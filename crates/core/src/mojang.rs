//! Mojang version manifest, version JSON format, libraries and assets.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::http::{self, Download};
use crate::{maven, platform, Launcher};

pub const MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const RESOURCES_URL: &str = "https://resources.download.minecraft.net";
pub const LIBRARIES_URL: &str = "https://libraries.minecraft.net/";

// ---------- manifest ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub latest: Latest,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
    pub sha1: Option<String>,
}

pub async fn manifest(l: &Launcher) -> Result<Manifest> {
    let cache = l.paths.cache().join("version_manifest_v2.json");
    match http::get_json::<Manifest>(&l.http, MANIFEST_URL).await {
        Ok(m) => {
            let _ = http::write_json(&cache, &m).await;
            Ok(m)
        }
        // Offline: fall back to the last copy we saw.
        Err(e) => http::read_json(&cache)
            .await
            .map_err(|_| e.context("could not fetch the Minecraft version list")),
    }
}

// ---------- version JSON ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "inheritsFrom", skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    #[serde(rename = "mainClass", skip_serializing_if = "Option::is_none")]
    pub main_class: Option<String>,
    #[serde(rename = "minecraftArguments", skip_serializing_if = "Option::is_none")]
    pub minecraft_arguments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(rename = "assetIndex", skip_serializing_if = "Option::is_none")]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<HashMap<String, Artifact>>,
    #[serde(rename = "javaVersion", skip_serializing_if = "Option::is_none")]
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Ruled { rules: Vec<Rule>, value: StringOrList },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s.clone()],
            Self::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Library {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<LibDownloads>,
    /// Base maven URL (used by Fabric/NeoForge libraries without `downloads`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<Rule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub natives: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibDownloads {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

// ---------- rules ----------

/// Launch features we advertise. We support none of the optional ones
/// (demo mode, custom resolution, quick play) so all are false.
pub fn rules_allow(rules: &Option<Vec<Rule>>) -> bool {
    let Some(rules) = rules else { return true };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for r in rules {
        if rule_matches(r) {
            allowed = r.action == "allow";
        }
    }
    allowed
}

fn rule_matches(r: &Rule) -> bool {
    if let Some(os) = &r.os {
        if let Some(name) = &os.name {
            if name != platform::mojang_os() {
                return false;
            }
        }
        if let Some(arch) = &os.arch {
            let ours = platform::mojang_arch();
            let ok = match arch.as_str() {
                "x86" => ours == "x86",
                "arm64" => ours == "arm64",
                "x86_64" => ours == "x86_64",
                _ => false,
            };
            if !ok {
                return false;
            }
        }
    }
    if let Some(features) = &r.features {
        // A rule that requires any feature to be true never matches.
        if features.values().any(|v| *v) {
            return false;
        }
    }
    true
}

impl Library {
    /// The main jar of this library, if it has one for this OS.
    pub fn artifact_download(&self, libs_dir: &std::path::Path) -> Result<Option<Download>> {
        if !rules_allow(&self.rules) {
            return Ok(None);
        }
        if let Some(d) = &self.downloads {
            if let Some(a) = &d.artifact {
                if a.url.is_empty() {
                    // Locally generated (e.g. by the NeoForge installer); nothing to fetch.
                    let rel = match &a.path {
                        Some(p) => p.clone(),
                        None => maven::coord_to_path(&self.name)?,
                    };
                    return Ok(Some(Download {
                        url: String::new(),
                        path: libs_dir.join(rel),
                        sha1: a.sha1.clone(),
                        size: a.size,
                    }));
                }
                let rel = match &a.path {
                    Some(p) => p.clone(),
                    None => maven::coord_to_path(&self.name)?,
                };
                return Ok(Some(Download {
                    url: a.url.clone(),
                    path: libs_dir.join(rel),
                    sha1: a.sha1.clone(),
                    size: a.size,
                }));
            }
            if d.classifiers.is_some() && self.natives.is_some() {
                // Natives-only library (pre-1.19 LWJGL).
                return Ok(None);
            }
        }
        let rel = maven::coord_to_path(&self.name)?;
        let base = self.url.clone().unwrap_or_else(|| LIBRARIES_URL.to_string());
        let base = if base.ends_with('/') { base } else { format!("{base}/") };
        Ok(Some(Download {
            url: format!("{base}{rel}"),
            path: libs_dir.join(&rel),
            sha1: self.sha1.clone(),
            size: self.size,
        }))
    }

    /// Old-style natives jar (to be extracted) for this OS, if any.
    pub fn natives_download(&self, libs_dir: &std::path::Path) -> Option<Download> {
        if !rules_allow(&self.rules) {
            return None;
        }
        let natives = self.natives.as_ref()?;
        let key = natives.get(platform::mojang_os())?;
        let bits = if cfg!(target_pointer_width = "64") { "64" } else { "32" };
        let key = key.replace("${arch}", bits);
        let a = self.downloads.as_ref()?.classifiers.as_ref()?.get(&key)?;
        let rel = a.path.clone().unwrap_or_else(|| {
            maven::coord_to_path(&format!("{}:{key}", self.name)).unwrap_or_default()
        });
        Some(Download {
            url: a.url.clone(),
            path: libs_dir.join(rel),
            sha1: a.sha1.clone(),
            size: a.size,
        })
    }
}

// ---------- loading and merging ----------

pub async fn load_version(l: &Launcher, id: &str) -> Result<VersionJson> {
    http::read_json(&l.paths.version_json(id))
        .await
        .with_context(|| format!("version {id} is not installed"))
}

/// Load a version and everything it inherits from, merged into one.
/// Returns the merged JSON and the id of the root (vanilla) version.
pub async fn resolve(l: &Launcher, id: &str) -> Result<(VersionJson, String)> {
    let mut chain = vec![load_version(l, id).await?];
    while let Some(parent) = chain.last().unwrap().inherits_from.clone() {
        if chain.len() > 8 {
            bail!("version inheritance is too deep for {id}");
        }
        chain.push(load_version(l, &parent).await?);
    }
    let root_id = chain.last().unwrap().id.clone();
    let mut merged = chain.pop().unwrap();
    while let Some(child) = chain.pop() {
        merged = merge(child, merged);
    }
    Ok((merged, root_id))
}

pub fn merge(child: VersionJson, parent: VersionJson) -> VersionJson {
    // Child libraries take precedence over parent ones with the same identity.
    let mut seen = std::collections::HashSet::new();
    let mut libraries = vec![];
    for lib in child.libraries.into_iter().chain(parent.libraries) {
        if seen.insert(maven::coord_key(&lib.name)) {
            libraries.push(lib);
        }
    }
    let arguments = match (parent.arguments, child.arguments) {
        (Some(mut p), Some(c)) => {
            p.game.extend(c.game);
            p.jvm.extend(c.jvm);
            Some(p)
        }
        (p, c) => c.or(p),
    };
    VersionJson {
        id: child.id,
        inherits_from: None,
        main_class: child.main_class.or(parent.main_class),
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        arguments,
        libraries,
        asset_index: child.asset_index.or(parent.asset_index),
        assets: child.assets.or(parent.assets),
        downloads: child.downloads.or(parent.downloads),
        java_version: child.java_version.or(parent.java_version),
        kind: child.kind.or(parent.kind),
        logging: child.logging.or(parent.logging),
    }
}

// ---------- installing ----------

/// Download a vanilla version's JSON (if missing) and return it.
pub async fn fetch_version_json(l: &Launcher, id: &str) -> Result<VersionJson> {
    let path = l.paths.version_json(id);
    if let Ok(v) = http::read_json::<VersionJson>(&path).await {
        return Ok(v);
    }
    let m = manifest(l).await?;
    let entry = m
        .versions
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| anyhow!("Minecraft version {id} does not exist"))?;
    http::download(
        &l.http,
        &Download {
            url: entry.url.clone(),
            path: path.clone(),
            sha1: entry.sha1.clone(),
            size: None,
        },
    )
    .await?;
    http::read_json(&path).await
}

/// Make sure every file a (possibly modded) version needs is on disk.
pub async fn install_files(l: &Launcher, version_id: &str) -> Result<()> {
    let (v, root_id) = resolve(l, version_id).await?;
    let libs = l.paths.libraries();

    let mut downloads = vec![];
    if let Some(client) = v.downloads.as_ref().and_then(|d| d.get("client")) {
        downloads.push(Download {
            url: client.url.clone(),
            path: l.paths.version_jar(&root_id),
            sha1: client.sha1.clone(),
            size: client.size,
        });
    }
    for lib in &v.libraries {
        if let Some(d) = lib.artifact_download(&libs)? {
            if !d.url.is_empty() {
                downloads.push(d);
            }
        }
        if let Some(d) = lib.natives_download(&libs) {
            downloads.push(d);
        }
    }
    dedupe(&mut downloads);
    http::download_many(l, "libraries", downloads).await?;

    if let Some(index) = &v.asset_index {
        install_assets(l, index).await?;
    }
    Ok(())
}

fn dedupe(d: &mut Vec<Download>) {
    let mut seen = std::collections::HashSet::new();
    d.retain(|x| seen.insert(x.path.clone()));
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

pub fn asset_object_path(assets: &std::path::Path, hash: &str) -> PathBuf {
    assets.join("objects").join(&hash[..2]).join(hash)
}

async fn install_assets(l: &Launcher, index: &AssetIndexRef) -> Result<()> {
    let assets = l.paths.assets();
    let index_path = assets.join("indexes").join(format!("{}.json", index.id));
    http::download(
        &l.http,
        &Download {
            url: index.url.clone(),
            path: index_path.clone(),
            sha1: index.sha1.clone(),
            size: index.size,
        },
    )
    .await?;
    let idx: AssetIndex = http::read_json(&index_path).await?;
    let downloads: Vec<Download> = idx
        .objects
        .values()
        .map(|o| Download {
            url: format!("{RESOURCES_URL}/{}/{}", &o.hash[..2], o.hash),
            path: asset_object_path(&assets, &o.hash),
            // Size check only: hashing ~4000 small files every launch is slow.
            sha1: None,
            size: Some(o.size),
        })
        .collect();
    http::download_many(l, "assets", downloads).await?;

    if idx.is_virtual || idx.map_to_resources {
        // Very old versions read loose files instead of the object store.
        let virt = assets.join("virtual").join(&index.id);
        for (name, o) in &idx.objects {
            let dst = crate::paths::safe_join(&virt, name)?;
            if tokio::fs::metadata(&dst).await.is_err() {
                if let Some(p) = dst.parent() {
                    tokio::fs::create_dir_all(p).await?;
                }
                tokio::fs::copy(asset_object_path(&assets, &o.hash), &dst).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_disallow_other_os() {
        let other = if platform::mojang_os() == "windows" { "osx" } else { "windows" };
        let rules = Some(vec![Rule {
            action: "allow".into(),
            os: Some(OsRule { name: Some(other.into()), arch: None, version: None }),
            features: None,
        }]);
        assert!(!rules_allow(&rules));
        let rules = Some(vec![
            Rule { action: "allow".into(), os: None, features: None },
            Rule {
                action: "disallow".into(),
                os: Some(OsRule { name: Some(other.into()), arch: None, version: None }),
                features: None,
            },
        ]);
        assert!(rules_allow(&rules));
    }

    #[test]
    fn feature_rules_off() {
        let mut f = HashMap::new();
        f.insert("is_demo_user".to_string(), true);
        let rules = Some(vec![Rule { action: "allow".into(), os: None, features: Some(f) }]);
        assert!(!rules_allow(&rules));
    }

    #[test]
    fn parses_modern_and_fabric_json() {
        let vanilla = r#"{"id":"1.21.1","mainClass":"net.minecraft.client.main.Main",
          "arguments":{"game":["--username","${auth_player_name}",{"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"}],
          "jvm":[{"rules":[{"action":"allow","os":{"name":"windows"}}],"value":"-XX:HeapDumpPath=x"},"-cp","${classpath}"]},
          "libraries":[{"name":"org.ow2.asm:asm:9.3","downloads":{"artifact":{"path":"org/ow2/asm/asm/9.3/asm-9.3.jar","url":"https://x/asm.jar","sha1":"a","size":1}}}],
          "javaVersion":{"component":"java-runtime-delta","majorVersion":21},"type":"release"}"#;
        let fabric = r#"{"id":"fabric-loader-0.16.0-1.21.1","inheritsFrom":"1.21.1","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
          "arguments":{"game":[],"jvm":["-DFabricMcEmu= net.minecraft.client.main.Main "]},
          "libraries":[{"name":"org.ow2.asm:asm:9.7","url":"https://maven.fabricmc.net/"},{"name":"net.fabricmc:fabric-loader:0.16.0","url":"https://maven.fabricmc.net/"}]}"#;
        let p: VersionJson = serde_json::from_str(vanilla).unwrap();
        let c: VersionJson = serde_json::from_str(fabric).unwrap();
        let m = merge(c, p);
        assert_eq!(m.main_class.unwrap(), "net.fabricmc.loader.impl.launch.knot.KnotClient");
        // Fabric's newer ASM replaces vanilla's.
        let asm: Vec<_> = m.libraries.iter().filter(|l| l.name.starts_with("org.ow2.asm:asm:")).collect();
        assert_eq!(asm.len(), 1);
        assert_eq!(asm[0].name, "org.ow2.asm:asm:9.7");
        assert_eq!(m.java_version.unwrap().major_version, 21);
        assert_eq!(m.arguments.as_ref().unwrap().jvm.len(), 4);
        let d = m.libraries[0].artifact_download(std::path::Path::new("/libs")).unwrap().unwrap();
        assert_eq!(d.url, "https://maven.fabricmc.net/org/ow2/asm/asm/9.7/asm-9.7.jar");
    }
}
