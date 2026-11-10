//! Modrinth modpacks (`.mrpack`): import into a new instance, and export an
//! instance to share.
//!
//! Format: https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha1::Digest;

use crate::browse::{self, OnlineMod, Source};
use crate::http::{self, Download};
use crate::instances::{self, Instance, NewInstance};
use crate::loaders::Loader;
use crate::{mods, Event, Launcher};

/// Hosts a pack may download from, per the .mrpack spec.
pub const ALLOWED_HOSTS: &[&str] = &["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub format_version: u32,
    pub game: String,
    pub version_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<PackFile>,
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub hashes: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    pub downloads: Vec<String>,
    pub file_size: u64,
}

pub fn host_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host); // never trust userinfo
    let host = host.split(':').next().unwrap_or(host);
    ALLOWED_HOSTS.contains(&host)
}

/// Loader and versions from a pack's `dependencies`.
pub fn pack_target(deps: &HashMap<String, String>) -> Result<(String, Loader, Option<String>)> {
    let mc = deps.get("minecraft").cloned().ok_or_else(|| anyhow!("the pack doesn't say which Minecraft version it's for"))?;
    if deps.contains_key("forge") {
        bail!("this pack uses Forge; MioCraft supports Fabric and NeoForge packs");
    }
    if deps.contains_key("quilt-loader") {
        bail!("this pack uses Quilt; MioCraft supports Fabric and NeoForge packs");
    }
    if let Some(v) = deps.get("fabric-loader") {
        return Ok((mc, Loader::Fabric, Some(v.clone())));
    }
    if let Some(v) = deps.get("neoforge") {
        return Ok((mc, Loader::Neoforge, Some(v.clone())));
    }
    Ok((mc, Loader::Vanilla, None))
}

fn client_side(f: &PackFile) -> bool {
    f.env.as_ref().and_then(|e| e.get("client")).map(|c| c != "unsupported").unwrap_or(true)
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub instance: Instance,
    pub downloaded: usize,
    pub override_files: usize,
    pub warnings: Vec<String>,
}

/// Create a new instance from an .mrpack file's bytes.
pub async fn import(l: &Launcher, bytes: Vec<u8>) -> Result<ImportReport> {
    let tmp = l.paths.cache().join(format!("import-{}.mrpack", uuid::Uuid::new_v4()));
    tokio::fs::write(&tmp, &bytes).await?;
    drop(bytes);
    let r = import_file(l, &tmp).await;
    let _ = tokio::fs::remove_file(&tmp).await;
    r
}

pub async fn import_file(l: &Launcher, pack: &Path) -> Result<ImportReport> {
    let p = pack.to_path_buf();
    let index: Index = tokio::task::spawn_blocking(move || -> Result<Index> {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&p)?).context("that file isn't a valid .mrpack")?;
        let mut f = zip.by_name("modrinth.index.json").map_err(|_| anyhow!("that file isn't a Modrinth modpack (no modrinth.index.json)"))?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        serde_json::from_str(&s).context("couldn't read the modpack index")
    })
    .await??;
    if index.game != "minecraft" {
        bail!("this pack is for {:?}, not Minecraft", index.game);
    }
    let (mc, loader, loader_version) = pack_target(&index.dependencies)?;

    let inst = instances::create(
        l,
        NewInstance { name: Some(index.name.clone()), mc_version: mc, loader, loader_version, ram_mb: None },
    )
    .await?;
    let result = fill_instance(l, &inst, &index, pack).await;
    match result {
        Ok((downloaded, override_files, warnings)) => {
            l.emit(Event::ModsChanged { instance_id: inst.id.clone() });
            Ok(ImportReport { instance: inst, downloaded, override_files, warnings })
        }
        Err(e) => {
            // Don't leave a half-imported instance behind.
            let _ = instances::delete(l, &inst.id).await;
            Err(e)
        }
    }
}

async fn fill_instance(l: &Launcher, inst: &Instance, index: &Index, pack: &Path) -> Result<(usize, usize, Vec<String>)> {
    let game = l.paths.game_dir(&inst.id);
    let mut warnings = vec![];
    let mut downloads = vec![];
    let mut mod_files: Vec<(String, String, String)> = vec![]; // (file name, sha1, url)
    for f in index.files.iter().filter(|f| client_side(f)) {
        let path = crate::paths::safe_join(&game, &f.path).with_context(|| format!("unsafe path in pack: {:?}", f.path))?;
        let Some(url) = f.downloads.iter().find(|u| host_allowed(u)) else {
            warnings.push(format!("Skipped {}: its download isn't from a host modpacks may use", f.path));
            continue;
        };
        let sha1 = f.hashes.get("sha1").cloned();
        if sha1.is_none() {
            warnings.push(format!("Skipped {}: no sha1 hash", f.path));
            continue;
        }
        if let Some(name) = f.path.strip_prefix("mods/") {
            if !name.contains('/') {
                mod_files.push((name.to_string(), sha1.clone().unwrap(), url.clone()));
            }
        }
        downloads.push(Download { url: url.clone(), path, sha1, size: Some(f.file_size) });
    }
    let downloaded = downloads.len();
    http::download_many(l, "modpack", downloads).await?;

    let (p, g) = (pack.to_path_buf(), game.clone());
    let override_files = tokio::task::spawn_blocking(move || -> Result<usize> {
        let mut n = 0;
        for prefix in ["overrides/", "client-overrides/"] {
            let count = std::cell::Cell::new(0usize);
            http::extract_zip(&p, &g, |name| {
                let keep = name.starts_with(prefix) && name.len() > prefix.len();
                if keep && !name.ends_with('/') {
                    count.set(count.get() + 1);
                }
                keep
            }, true)?;
            n += count.get();
        }
        Ok(n)
    })
    .await??;

    // Best effort: recognise the pack's mods on Modrinth so they get badges and update checks.
    if let Err(e) = tag_from_modrinth(l, &inst.id, &mod_files).await {
        warnings.push(format!("Couldn't match mods to Modrinth for update checks: {e:#}"));
    }
    Ok((downloaded, override_files, warnings))
}

async fn tag_from_modrinth(l: &Launcher, instance_id: &str, files: &[(String, String, String)]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let hashes: Vec<&str> = files.iter().map(|f| f.1.as_str()).collect();
    let resp = l
        .http
        .post(format!("{}/version_files", browse::modrinth_api()))
        .json(&serde_json::json!({ "hashes": hashes, "algorithm": "sha1" }))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("Modrinth returned {}", resp.status());
    }
    let by_hash: HashMap<String, serde_json::Value> = resp.json().await?;
    let project_ids: Vec<String> = by_hash.values().filter_map(|v| v["project_id"].as_str().map(String::from)).collect();
    let mut titles: HashMap<String, (String, String)> = HashMap::new();
    if !project_ids.is_empty() {
        let url = format!(
            "{}/projects?ids={}",
            browse::modrinth_api(),
            browse::urlencode(&serde_json::to_string(&project_ids)?)
        );
        if let Ok(v) = l.http.get(url).send().await?.json::<Vec<serde_json::Value>>().await {
            for p in v {
                if let (Some(id), Some(t), Some(s)) = (p["id"].as_str(), p["title"].as_str(), p["slug"].as_str()) {
                    titles.insert(id.into(), (t.into(), s.into()));
                }
            }
        }
    }
    let mut tags = browse::load_tags(l, instance_id).await;
    for (file, sha1, url) in files {
        let Some(v) = by_hash.get(sha1) else { continue };
        let (Some(pid), Some(vid)) = (v["project_id"].as_str(), v["id"].as_str()) else { continue };
        let (title, slug) = titles.get(pid).cloned().unwrap_or_else(|| (file.clone(), pid.to_string()));
        tags.insert(
            file.clone(),
            OnlineMod {
                source: Source::Modrinth,
                project_id: pid.into(),
                version_id: vid.into(),
                title,
                page_url: format!("https://modrinth.com/mod/{slug}"),
                as_dependency: false,
                download_url: Some(url.clone()),
            },
        );
    }
    browse::save_tags(l, instance_id, &tags).await
}

// ---------- export ----------

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExportOptions {
    /// Pack version, e.g. "1.0.0".
    pub version: Option<String>,
    /// Include the instance's `config/` folder.
    #[serde(default)]
    pub include_config: bool,
    /// Embed mods that aren't from Modrinth and weren't made by Claude
    /// (CurseForge downloads, jars added by hand). Only do this if their
    /// licences allow sharing.
    #[serde(default)]
    pub include_other_jars: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub path: PathBuf,
    /// Mods referenced by download link (Modrinth).
    pub linked: Vec<String>,
    /// Jars stored inside the pack (Claude's mods, plus others if asked).
    pub embedded: Vec<String>,
    /// Enabled mods that were not included, with the reason.
    pub left_out: Vec<String>,
}

fn sha_both(path: &Path) -> Result<(String, String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let (mut s1, mut s512) = (sha1::Sha1::new(), sha2::Sha512::new());
    let mut buf = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        s1.update(&buf[..n]);
        s512.update(&buf[..n]);
    }
    Ok((hex::encode(s1.finalize()), hex::encode(s512.finalize()), size))
}

pub async fn export(l: &Launcher, instance_id: &str, opts: ExportOptions) -> Result<ExportReport> {
    let inst = instances::get(l, instance_id).await?;
    let game = l.paths.game_dir(&inst.id);
    let mods_dir = l.paths.mods_dir(&inst.id);
    let list = mods::list(l, &inst.id).await?;
    let version = opts.version.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "1.0.0".into());

    let mut deps = HashMap::new();
    deps.insert("minecraft".to_string(), inst.mc_version.clone());
    match inst.loader {
        Loader::Fabric => {
            deps.insert("fabric-loader".into(), inst.loader_version.clone().unwrap_or_default());
        }
        Loader::Neoforge => {
            deps.insert("neoforge".into(), inst.loader_version.clone().unwrap_or_default());
        }
        Loader::Vanilla => {}
    }

    let mut files = vec![];
    let mut report = ExportReport { path: PathBuf::new(), linked: vec![], embedded: vec![], left_out: vec![] };
    let mut embed: Vec<(PathBuf, String)> = vec![]; // (source, path inside zip)
    for m in list.iter().filter(|m| m.enabled) {
        let label = m.name.clone().unwrap_or_else(|| m.file_name.clone());
        let src = mods_dir.join(&m.file_name);
        let from_modrinth = m.online.as_ref().filter(|o| o.source == Source::Modrinth);
        let mut modrinth_url = from_modrinth.and_then(|o| o.download_url.clone()).filter(|u| host_allowed(u));
        if modrinth_url.is_none() {
            if let Some(o) = from_modrinth {
                // Installed before links were recorded: ask Modrinth.
                modrinth_url = modrinth_file_url(l, &o.version_id, &m.file_name).await.filter(|u| host_allowed(u));
            }
        }
        if let Some(url) = modrinth_url {
            let p = src.clone();
            let (s1, s512, size) = tokio::task::spawn_blocking(move || sha_both(&p)).await??;
            let mut hashes = HashMap::new();
            hashes.insert("sha1".into(), s1);
            hashes.insert("sha512".into(), s512);
            files.push(PackFile { path: format!("mods/{}", m.file_name), hashes, env: None, downloads: vec![url], file_size: size });
            report.linked.push(label);
        } else if m.claude_project.is_some() || opts.include_other_jars {
            embed.push((src, format!("overrides/mods/{}", m.file_name)));
            report.embedded.push(label);
        } else {
            let why = match &m.online {
                Some(o) if o.source == Source::Curseforge => "from CurseForge, which doesn't allow linking in .mrpack files",
                Some(_) => "from Modrinth, but its download link couldn't be found; reinstall it from Browse",
                None => "added by hand; it's unknown whether it may be shared",
            };
            report.left_out.push(format!("{label} ({why})"));
        }
    }
    if opts.include_config {
        let cfg = game.join("config");
        if cfg.exists() {
            for path in walk(&cfg)? {
                let rel = path.strip_prefix(&game)?.to_string_lossy().replace('\\', "/");
                embed.push((path, format!("overrides/{rel}")));
            }
        }
    }

    let index = Index {
        format_version: 1,
        game: "minecraft".into(),
        version_id: version.clone(),
        name: inst.name.clone(),
        summary: Some(format!("Exported from MioCraft ({} {})", inst.mc_version, inst.loader.as_str())),
        files,
        dependencies: deps,
    };
    let out_dir = l.paths.root.join("exports");
    tokio::fs::create_dir_all(&out_dir).await?;
    let out = out_dir.join(format!("{}-{}.mrpack", instances::slug(&inst.name), instances::slug(&version)));
    let o = out.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let tmp = o.with_extension("mrpack.tmp");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&tmp)?);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("modrinth.index.json", opt)?;
        zip.write_all(&serde_json::to_vec_pretty(&index)?)?;
        for (src, name) in embed {
            zip.start_file(name, opt)?;
            std::io::copy(&mut std::fs::File::open(&src)?, &mut zip)?;
        }
        zip.finish()?;
        std::fs::rename(&tmp, &o)?;
        Ok(())
    })
    .await??;
    report.path = out;
    Ok(report)
}

/// Download URL of a Modrinth version's file (matching `file_name` if it has several).
async fn modrinth_file_url(l: &Launcher, version_id: &str, file_name: &str) -> Option<String> {
    let url = format!("{}/version/{}", browse::modrinth_api(), browse::urlencode(version_id));
    let v: serde_json::Value = l.http.get(url).send().await.ok()?.json().await.ok()?;
    let files = v["files"].as_array()?;
    files
        .iter()
        .find(|f| f["filename"].as_str() == Some(file_name))
        .or_else(|| files.iter().find(|f| f["primary"].as_bool() == Some(true)))
        .and_then(|f| f["url"].as_str().map(String::from))
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let ft = e.file_type()?;
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                out.push(e.path());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;

    #[test]
    fn hosts() {
        assert!(host_allowed("https://cdn.modrinth.com/data/x/y.jar"));
        assert!(host_allowed("https://github.com/a/b/releases/x.jar"));
        assert!(!host_allowed("http://cdn.modrinth.com/x.jar"));
        assert!(!host_allowed("https://cdn.modrinth.com.evil.net/x.jar"));
        assert!(!host_allowed("https://cdn.modrinth.com@evil.net/x.jar"));
        assert!(!host_allowed("https://evil.net/cdn.modrinth.com/x.jar"));
    }

    #[test]
    fn targets() {
        let mut d = HashMap::new();
        d.insert("minecraft".to_string(), "1.21.1".to_string());
        d.insert("fabric-loader".to_string(), "0.16.9".to_string());
        assert_eq!(pack_target(&d).unwrap().1, Loader::Fabric);
        d.remove("fabric-loader");
        d.insert("forge".into(), "52".into());
        assert!(pack_target(&d).is_err());
    }

    fn test_launcher_without_proxy(dir: &Path) -> Launcher {
        let rt = tokio::runtime::Handle::current();
        let l = tokio::task::block_in_place(|| rt.block_on(Launcher::new(Paths::new(dir)))).unwrap();
        Launcher(std::sync::Arc::new(crate::Inner {
            paths: l.paths.clone(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            events: l.events.clone(),
            config: tokio::sync::RwLock::new(Default::default()),
            processes: Default::default(),
            install_lock: tokio::sync::Mutex::new(()),
        }))
    }

    /// Export a Fabric instance (one Modrinth mod, one Claude mod, one hand-added
    /// jar), check the index, then import the pack into a new instance.
    #[tokio::test(flavor = "multi_thread")]
    async fn export_then_import_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let l = test_launcher_without_proxy(dir.path());
        let inst = instances::create(&l, NewInstance {
            name: Some("Round Trip".into()), mc_version: "1.21.1".into(), loader: Loader::Fabric,
            loader_version: Some("0.16.9".into()), ram_mb: None,
        }).await.unwrap();
        let md = l.paths.mods_dir(&inst.id);
        let jar = |name: &str, id: &str| {
            let mut z = zip::ZipWriter::new(std::fs::File::create(md.join(name)).unwrap());
            z.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(format!(r#"{{"id":"{id}","name":"{id}","version":"1"}}"#).as_bytes()).unwrap();
            z.finish().unwrap();
        };
        jar("sodium.jar", "sodium");
        jar("glow.jar", "glow");
        jar("manual.jar", "manual");
        let mut tags = browse::OnlineTags::new();
        tags.insert("sodium.jar".into(), OnlineMod {
            source: Source::Modrinth, project_id: "AANobbMI".into(), version_id: "v".into(), title: "Sodium".into(),
            page_url: String::new(), as_dependency: false,
            download_url: Some("https://cdn.modrinth.com/data/AANobbMI/versions/v/sodium.jar".into()),
        });
        browse::save_tags(&l, &inst.id, &tags).await.unwrap();
        let mut ct = mods::ClaudeTags::default();
        ct.files.insert("glow.jar".into(), "glow".into());
        mods::save_tags(&l, &inst.id, &ct).await.unwrap();
        std::fs::create_dir_all(l.paths.game_dir(&inst.id).join("config")).unwrap();
        std::fs::write(l.paths.game_dir(&inst.id).join("config/glow.toml"), "brightness = 7").unwrap();

        let r = export(&l, &inst.id, ExportOptions { version: Some("2.0".into()), include_config: true, include_other_jars: false }).await.unwrap();
        assert_eq!(r.linked, vec!["sodium"]);
        assert_eq!(r.embedded, vec!["glow"]);
        assert_eq!(r.left_out.len(), 1);
        assert!(r.path.ends_with("round-trip-2-0.mrpack"));

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&r.path).unwrap()).unwrap();
        let mut idx = String::new();
        zip.by_name("modrinth.index.json").unwrap().read_to_string(&mut idx).unwrap();
        let idx: Index = serde_json::from_str(&idx).unwrap();
        assert_eq!(idx.dependencies["fabric-loader"], "0.16.9");
        assert_eq!(idx.files.len(), 1);
        assert_eq!(idx.files[0].hashes["sha512"].len(), 128);
        assert!(zip.by_name("overrides/mods/glow.jar").is_ok());
        assert!(zip.by_name("overrides/config/glow.toml").is_ok());
        assert!(zip.by_name("overrides/mods/manual.jar").is_err());
        drop(zip);

        // Import it back. Strip the remote file so no network is needed.
        let mut idx2 = idx.clone();
        idx2.files.clear();
        let pack2 = dir.path().join("p2.mrpack");
        {
            let mut src = zip::ZipArchive::new(std::fs::File::open(&r.path).unwrap()).unwrap();
            let mut z = zip::ZipWriter::new(std::fs::File::create(&pack2).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("modrinth.index.json", o).unwrap();
            z.write_all(&serde_json::to_vec(&idx2).unwrap()).unwrap();
            for name in ["overrides/mods/glow.jar", "overrides/config/glow.toml"] {
                let mut buf = vec![];
                src.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
                z.start_file(name, o).unwrap();
                z.write_all(&buf).unwrap();
            }
            z.finish().unwrap();
        }
        let imp = import_file(&l, &pack2).await.unwrap();
        assert_eq!(imp.instance.name, "Round Trip");
        assert_ne!(imp.instance.id, inst.id);
        assert_eq!(imp.instance.loader_version.as_deref(), Some("0.16.9"));
        let g = l.paths.game_dir(&imp.instance.id);
        assert!(g.join("mods/glow.jar").exists());
        assert_eq!(std::fs::read_to_string(g.join("config/glow.toml")).unwrap(), "brightness = 7");
        assert_eq!(imp.override_files, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rejects_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let l = test_launcher_without_proxy(dir.path());
        let pack = dir.path().join("bad.mrpack");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&pack).unwrap());
        z.start_file("modrinth.index.json", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(br#"{"formatVersion":1,"game":"minecraft","versionId":"1","name":"Bad","dependencies":{"minecraft":"1.21.1","fabric-loader":"0.16.9"},
            "files":[{"path":"../../evil.jar","hashes":{"sha1":"x"},"downloads":["https://cdn.modrinth.com/x.jar"],"fileSize":1}]}"#).unwrap();
        z.finish().unwrap();
        assert!(import_file(&l, &pack).await.is_err());
        assert!(instances::list(&l).await.unwrap().is_empty(), "half-imported instance removed");
    }
}
