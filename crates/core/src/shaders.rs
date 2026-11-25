//! Shader packs: installed from the browser (Modrinth / CurseForge) into an
//! instance's `shaderpacks/` folder, listed and removed from the launcher.
//! Searching reuses `browse` with `Kind::Shader`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use crate::browse::{self, InstallReport, InstallRequest, Kind, OnlineMod, Source};
use crate::http::{self, Download};
use crate::{instances, mods, paths, Event, Launcher};

/// Mods that load shader packs. Iris needs Sodium, which the mod browser
/// installs with it.
const SHADER_LOADER_MOD_IDS: [&str; 3] = ["iris", "oculus", "optifine"];
/// Iris on Modrinth, offered when an instance has no shader loader yet.
pub const IRIS_MODRINTH_ID: &str = "YL57xq9U";

pub fn dir(l: &Launcher, instance_id: &str) -> PathBuf {
    l.paths.game_dir(instance_id).join("shaderpacks")
}

/// `instances/<id>/online-shaders.json` (or `online-resourcepacks.json`): pack file name → where it came from.
pub(crate) type Tags = HashMap<String, OnlineMod>;

/// The folder a kind of pack goes in.
pub(crate) fn pack_dir(l: &Launcher, instance_id: &str, kind: Kind) -> PathBuf {
    match kind {
        Kind::ResourcePack => crate::resource_packs::dir(l, instance_id),
        _ => dir(l, instance_id),
    }
}

fn tags_file(l: &Launcher, instance_id: &str, kind: Kind) -> PathBuf {
    let name = if kind == Kind::ResourcePack { "online-resourcepacks.json" } else { "online-shaders.json" };
    l.paths.instance_dir(instance_id).join(name)
}

/// Tags for packs that still exist (packs can be deleted by hand).
pub async fn load_tags(l: &Launcher, instance_id: &str) -> Tags {
    load_pack_tags(l, instance_id, Kind::Shader).await
}

pub(crate) async fn load_pack_tags(l: &Launcher, instance_id: &str, kind: Kind) -> Tags {
    let mut tags: Tags = http::read_json(&tags_file(l, instance_id, kind)).await.unwrap_or_default();
    let d = pack_dir(l, instance_id, kind);
    let before = tags.len();
    tags.retain(|f, _| d.join(f).exists());
    if tags.len() != before {
        let _ = http::write_json(&tags_file(l, instance_id, kind), &tags).await;
    }
    tags
}

#[derive(Debug, Clone, Serialize)]
pub struct ShaderPack {
    pub file_name: String,
    pub size: u64,
    /// Set for packs installed from the browser.
    pub online: Option<OnlineMod>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShaderList {
    pub packs: Vec<ShaderPack>,
    /// Whether the instance has a mod that can load shader packs (Iris).
    pub has_loader: bool,
    pub folder: PathBuf,
}

pub async fn list(l: &Launcher, instance_id: &str) -> Result<ShaderList> {
    let d = dir(l, instance_id);
    let tags = load_tags(l, instance_id).await;
    let mut packs = vec![];
    if let Ok(mut rd) = tokio::fs::read_dir(&d).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata().await else { continue };
            // Packs are zip files or unpacked folders; skip Iris' own settings files.
            if !(meta.is_dir() || name.to_lowercase().ends_with(".zip")) {
                continue;
            }
            packs.push(ShaderPack { online: tags.get(&name).cloned(), size: if meta.is_file() { meta.len() } else { 0 }, file_name: name });
        }
    }
    packs.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));
    let has_loader = mods::list(l, instance_id)
        .await
        .map(|m| {
            m.iter().any(|m| m.enabled && m.mod_id.as_deref().map(|id| SHADER_LOADER_MOD_IDS.contains(&id)).unwrap_or(false))
        })
        .unwrap_or(false);
    Ok(ShaderList { packs, has_loader, folder: d })
}

/// Download a shader pack into the instance's `shaderpacks/` folder.
pub async fn install(l: &Launcher, instance_id: &str, req: InstallRequest) -> Result<InstallReport> {
    let mut report = install_pack(l, instance_id, req, Kind::Shader).await?;
    if !list(l, instance_id).await?.has_loader {
        report.warnings.push("This instance has no shader loader yet. Install Iris from the mod browser to use shader packs.".to_string());
    }
    Ok(report)
}

/// Download a shader or resource pack from the browser into its folder.
pub(crate) async fn install_pack(l: &Launcher, instance_id: &str, req: InstallRequest, kind: Kind) -> Result<InstallReport> {
    let source = Source::parse(&req.source)?;
    let inst = instances::get(l, instance_id).await?;
    let (title, _slug, page_url) = browse::project_info(l, source, kind, &req.project_id).await?;
    let mut warnings = vec![];
    let version = match &req.version_id {
        Some(v) => browse::version_by_id(l, source, kind, &req.project_id, v, &inst).await?,
        None => {
            let all = browse::versions(l, source, kind, &req.project_id, &inst).await?;
            match browse::pick_version(&all).cloned() {
                Some(v) => v,
                // Resource packs for another version usually still work; the game just warns.
                None if kind == Kind::ResourcePack => {
                    let any = browse::versions_for(l, source, kind, &req.project_id, &inst, false).await?;
                    let v = browse::pick_version(&any).cloned().ok_or_else(|| anyhow!("{title} has no files to download"))?;
                    warnings.push(format!("{title} wasn't made for Minecraft {}, so the game may call it incompatible. It often works anyway.", inst.mc_version));
                    v
                }
                None => bail!("{title} has no version for Minecraft {}", inst.mc_version),
            }
        }
    };
    let folder = if kind == Kind::ResourcePack { "resourcepacks" } else { "shaderpacks" };
    let Some(url) = version.file.url.clone() else {
        bail!("The author of {title} doesn't allow downloads from other apps. Download it from {page_url} and drop it in the {folder} folder.");
    };
    let name = &version.file.filename;
    if !name.to_lowercase().ends_with(".zip") {
        bail!("{title} has an unexpected file name ({name})");
    }
    let d = pack_dir(l, &inst.id, kind);
    tokio::fs::create_dir_all(&d).await?;
    let path = paths::safe_join(&d, name)?;

    // Replace other versions of the same pack.
    let mut tags = load_pack_tags(l, &inst.id, kind).await;
    let old: Vec<String> = tags
        .iter()
        .filter(|(f, t)| t.source == source && t.project_id == req.project_id && *f != name)
        .map(|(f, _)| f.clone())
        .collect();
    for f in old {
        let _ = tokio::fs::remove_file(d.join(&f)).await;
        tags.remove(&f);
    }

    l.progress(folder, 0, 1, format!("Downloading {title}"));
    http::download(
        &l.http,
        &Download {
            url: url.clone(),
            path,
            sha1: version.file.sha1.clone(),
            size: if version.file.size > 0 { Some(version.file.size) } else { None },
        },
    )
    .await
    .with_context(|| format!("downloading {title}"))?;
    tags.insert(
        name.clone(),
        OnlineMod {
            source,
            project_id: req.project_id.clone(),
            version_id: version.version_id.clone(),
            title: title.clone(),
            page_url,
            as_dependency: false,
            download_url: Some(url),
        },
    );
    http::write_json(&tags_file(l, &inst.id, kind), &tags).await?;
    l.progress(folder, 1, 1, format!("{title} installed"));
    l.emit(Event::ModsChanged { instance_id: inst.id.clone() });
    Ok(InstallReport { installed: vec![name.clone()], already_installed: vec![], warnings })
}

pub async fn delete(l: &Launcher, instance_id: &str, file: &str) -> Result<()> {
    instances::get(l, instance_id).await?;
    let path = paths::safe_join(&dir(l, instance_id), file)?;
    let meta = tokio::fs::metadata(&path).await.map_err(|_| anyhow!("no shader pack named {file}"))?;
    if meta.is_dir() {
        tokio::fs::remove_dir_all(&path).await?;
    } else {
        tokio::fs::remove_file(&path).await?;
    }
    load_tags(l, instance_id).await; // drops the tag for the removed file
    l.emit(Event::ModsChanged { instance_id: instance_id.into() });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::NewInstance;
    use crate::loaders::Loader;
    use crate::Paths;

    #[tokio::test]
    async fn lists_and_deletes_packs() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(tmp.path())).await.unwrap();
        let inst = instances::create(
            &l,
            NewInstance { name: Some("S".into()), mc_version: "1.21.1".into(), loader: Loader::Fabric, loader_version: Some("0.16.0".into()), ram_mb: None },
        )
        .await
        .unwrap();
        let d = dir(&l, &inst.id);
        tokio::fs::create_dir_all(d.join("Unpacked")).await.unwrap();
        tokio::fs::write(d.join("Pretty.zip"), b"zip").await.unwrap();
        tokio::fs::write(d.join("Pretty.zip.txt"), b"iris settings").await.unwrap();

        let s = list(&l, &inst.id).await.unwrap();
        let names: Vec<_> = s.packs.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(names, vec!["Pretty.zip", "Unpacked"]);
        assert!(!s.has_loader);

        assert!(delete(&l, &inst.id, "../Pretty.zip").await.is_err());
        delete(&l, &inst.id, "Pretty.zip").await.unwrap();
        delete(&l, &inst.id, "Unpacked").await.unwrap();
        assert!(list(&l, &inst.id).await.unwrap().packs.is_empty());
    }

    #[test]
    fn shader_facets() {
        let f = browse::modrinth_shader_facets("26.3");
        assert!(f.contains("project_type:shader") && f.contains("categories:iris") && f.contains("versions:26.3"));
    }
}
