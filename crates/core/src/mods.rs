//! The mod list of an instance.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::{http, paths, Event, Launcher};

#[derive(Debug, Clone, Serialize)]
pub struct ModEntry {
    pub file_name: String,
    pub enabled: bool,
    pub mod_id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
    /// `fabric`, `neoforge`, `forge` or `unknown`.
    pub loader: String,
    pub size: u64,
    /// Set when the jar was built by Claude from a mod project in this launcher.
    pub claude_project: Option<String>,
    /// Set when the jar came from the built-in browser (Modrinth / CurseForge).
    pub online: Option<crate::browse::OnlineMod>,
}

/// `instances/<id>/claude-mods.json`: installed jar file name → project id.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ClaudeTags {
    #[serde(flatten)]
    pub files: HashMap<String, String>,
}

fn tags_file(l: &Launcher, instance_id: &str) -> std::path::PathBuf {
    l.paths.instance_dir(instance_id).join("claude-mods.json")
}

pub async fn load_tags(l: &Launcher, instance_id: &str) -> ClaudeTags {
    http::read_json(&tags_file(l, instance_id)).await.unwrap_or_default()
}

pub async fn save_tags(l: &Launcher, instance_id: &str, t: &ClaudeTags) -> Result<()> {
    http::write_json(&tags_file(l, instance_id), t).await
}

fn base_name(file: &str) -> &str {
    file.strip_suffix(".disabled").unwrap_or(file)
}

pub async fn list(l: &Launcher, instance_id: &str) -> Result<Vec<ModEntry>> {
    let dir = l.paths.mods_dir(instance_id);
    tokio::fs::create_dir_all(&dir).await?;
    let tags = load_tags(l, instance_id).await;
    let online = crate::browse::prune_tags(l, instance_id).await;
    let mut out = vec![];
    let mut rd = tokio::fs::read_dir(&dir).await?;
    while let Some(e) = rd.next_entry().await? {
        let file_name = e.file_name().to_string_lossy().to_string();
        let enabled = file_name.ends_with(".jar");
        if !enabled && !file_name.ends_with(".jar.disabled") {
            continue;
        }
        let path = e.path();
        let size = e.metadata().await.map(|m| m.len()).unwrap_or(0);
        let meta = tokio::task::spawn_blocking(move || read_metadata(&path))
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        out.push(ModEntry {
            claude_project: tags.files.get(base_name(&file_name)).cloned(),
            online: online.get(base_name(&file_name)).cloned(),
            file_name,
            enabled,
            mod_id: meta.mod_id,
            name: meta.name,
            version: meta.version,
            description: meta.description,
            authors: meta.authors,
            loader: meta.loader,
            size,
        });
    }
    out.sort_by(|a, b| {
        let ka = a.name.clone().unwrap_or_else(|| a.file_name.clone()).to_lowercase();
        let kb = b.name.clone().unwrap_or_else(|| b.file_name.clone()).to_lowercase();
        ka.cmp(&kb)
    });
    Ok(out)
}

#[derive(Default)]
pub struct ModMeta {
    pub mod_id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub loader: String,
}

fn read_entry(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = zip.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

pub fn read_metadata(path: &Path) -> Result<ModMeta> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
    if let Some(json) = read_entry(&mut zip, "fabric.mod.json") {
        return Ok(parse_fabric(&json));
    }
    for (file, loader) in [
        ("META-INF/neoforge.mods.toml", "neoforge"),
        ("META-INF/mods.toml", "forge"),
    ] {
        if let Some(t) = read_entry(&mut zip, file) {
            let mut m = parse_mods_toml(&t, loader);
            if m.version.as_deref().map(|v| v.contains("${")).unwrap_or(false) {
                m.version = read_entry(&mut zip, "META-INF/MANIFEST.MF").and_then(|mf| {
                    mf.lines()
                        .find_map(|l| l.strip_prefix("Implementation-Version:"))
                        .map(|v| v.trim().to_string())
                });
            }
            return Ok(m);
        }
    }
    Ok(ModMeta { loader: "unknown".into(), ..Default::default() })
}

pub fn parse_fabric(json: &str) -> ModMeta {
    // Fabric allows comments-free JSON only, but be lenient about failures.
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let authors = v["authors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from).or_else(|| x["name"].as_str().map(String::from)))
                .collect()
        })
        .unwrap_or_default();
    ModMeta {
        mod_id: v["id"].as_str().map(String::from),
        name: v["name"].as_str().map(String::from),
        version: v["version"].as_str().map(String::from),
        description: v["description"].as_str().map(String::from),
        authors,
        loader: "fabric".into(),
    }
}

pub fn parse_mods_toml(text: &str, loader: &str) -> ModMeta {
    let v: toml::Value = toml::from_str(text).unwrap_or(toml::Value::Table(Default::default()));
    let m = v.get("mods").and_then(|m| m.as_array()).and_then(|a| a.first());
    let get = |k: &str| m.and_then(|m| m.get(k)).and_then(|x| x.as_str()).map(|s| s.trim().to_string());
    let authors = get("authors")
        .or_else(|| v.get("authors").and_then(|x| x.as_str()).map(String::from))
        .map(|a| a.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    ModMeta {
        mod_id: get("modId"),
        name: get("displayName"),
        version: get("version"),
        description: get("description"),
        authors,
        loader: loader.into(),
    }
}

fn mod_path(l: &Launcher, instance_id: &str, file: &str) -> Result<std::path::PathBuf> {
    if !(file.ends_with(".jar") || file.ends_with(".jar.disabled")) || file.contains('/') || file.contains('\\') {
        bail!("not a mod file name: {file:?}");
    }
    paths::safe_join(&l.paths.mods_dir(instance_id), file)
}

/// Enable or disable a mod by renaming `x.jar` ↔ `x.jar.disabled`.
pub async fn set_enabled(l: &Launcher, instance_id: &str, file: &str, enabled: bool) -> Result<String> {
    let path = mod_path(l, instance_id, file)?;
    if !path.exists() {
        bail!("mod {file} not found");
    }
    let base = base_name(file).to_string();
    let target = if enabled { base.clone() } else { format!("{base}.disabled") };
    if target != file {
        tokio::fs::rename(&path, l.paths.mods_dir(instance_id).join(&target)).await?;
    }
    l.emit(Event::ModsChanged { instance_id: instance_id.into() });
    Ok(target)
}

pub async fn delete(l: &Launcher, instance_id: &str, file: &str) -> Result<()> {
    let path = mod_path(l, instance_id, file)?;
    tokio::fs::remove_file(&path).await?;
    let mut tags = load_tags(l, instance_id).await;
    if tags.files.remove(base_name(file)).is_some() {
        save_tags(l, instance_id, &tags).await?;
    }
    l.emit(Event::ModsChanged { instance_id: instance_id.into() });
    Ok(())
}

/// Copy a jar into an instance's mods folder (replacing older builds of the
/// same project) and tag it as Claude-made when `project_id` is given.
pub async fn install_jar(
    l: &Launcher,
    instance_id: &str,
    jar: &Path,
    project_id: Option<&str>,
) -> Result<String> {
    let file_name = jar
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .ok_or_else(|| anyhow::anyhow!("bad jar path"))?;
    let dir = l.paths.mods_dir(instance_id);
    tokio::fs::create_dir_all(&dir).await?;
    let mut tags = load_tags(l, instance_id).await;
    if let Some(pid) = project_id {
        // Remove previous builds of this project (they may have other versions in the name).
        let old: Vec<String> = tags.files.iter().filter(|(_, p)| p.as_str() == pid).map(|(f, _)| f.clone()).collect();
        for f in old {
            let _ = tokio::fs::remove_file(dir.join(&f)).await;
            let _ = tokio::fs::remove_file(dir.join(format!("{f}.disabled"))).await;
            tags.files.remove(&f);
        }
        tags.files.insert(file_name.clone(), pid.to_string());
    }
    tokio::fs::copy(jar, dir.join(&file_name)).await?;
    let _ = tokio::fs::remove_file(dir.join(format!("{file_name}.disabled"))).await;
    save_tags(l, instance_id, &tags).await?;
    l.emit(Event::ModsChanged { instance_id: instance_id.into() });
    Ok(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fabric_meta() {
        let m = parse_fabric(r#"{"schemaVersion":1,"id":"glow","name":"Glow","version":"1.0.0","authors":["Mio",{"name":"Claude"}]}"#);
        assert_eq!(m.mod_id.as_deref(), Some("glow"));
        assert_eq!(m.authors, vec!["Mio", "Claude"]);
    }

    #[test]
    fn neo_meta() {
        let t = r#"
modLoader="javafml"
loaderVersion="[1,)"
license="MIT"
[[mods]]
modId="glow"
version="1.2.0"
displayName="Glow"
authors="Mio, Claude"
description='''
Makes things glow.
'''
"#;
        let m = parse_mods_toml(t, "neoforge");
        assert_eq!(m.mod_id.as_deref(), Some("glow"));
        assert_eq!(m.name.as_deref(), Some("Glow"));
        assert_eq!(m.description.as_deref(), Some("Makes things glow."));
        assert_eq!(m.authors.len(), 2);
    }
}
