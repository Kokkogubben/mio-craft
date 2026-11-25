//! Resource packs: the instance's `resourcepacks/` folder, which of them the game
//! has turned on (`resourcePacks` in `options.txt`), and installing them from the
//! browser (Modrinth / CurseForge, through `shaders::install_pack`).

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;

use crate::browse::{InstallReport, InstallRequest, Kind, OnlineMod};
use crate::game_options::{read_options, version_named_keys, Options};
use crate::{instances, paths, Event, Launcher};

const MAX_ICON: u64 = 256 * 1024;

pub fn dir(l: &Launcher, instance_id: &str) -> PathBuf {
    l.paths.game_dir(instance_id).join("resourcepacks")
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourcePack {
    pub file_name: String,
    /// The pack's own description (`pack.mcmeta`), as plain text.
    pub description: Option<String>,
    /// `pack.png` as a data URL.
    pub icon: Option<String>,
    pub size: u64,
    /// Turned on in the game's options.
    pub enabled: bool,
    pub online: Option<OnlineMod>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackList {
    pub packs: Vec<ResourcePack>,
    pub folder: PathBuf,
}

/// Text from a chat component (a string, `{"text", "extra"}` or a list), without § codes.
fn plain(v: &serde_json::Value) -> String {
    let raw = match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(plain).collect(),
        serde_json::Value::Object(o) => {
            let mut s = o.get("text").or_else(|| o.get("translate")).and_then(|t| t.as_str()).unwrap_or("").to_string();
            if let Some(extra) = o.get("extra") {
                s.push_str(&plain(extra));
            }
            s
        }
        _ => String::new(),
    };
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Description and icon from a zipped or unpacked pack.
fn read_pack(path: &Path) -> (Option<String>, Option<String>) {
    let (meta, icon) = if path.is_dir() {
        let icon = std::fs::metadata(path.join("pack.png")).ok().filter(|m| m.len() <= MAX_ICON).and_then(|_| std::fs::read(path.join("pack.png")).ok());
        (std::fs::read_to_string(path.join("pack.mcmeta")).ok(), icon)
    } else {
        let Ok(mut z) = std::fs::File::open(path).map_err(anyhow::Error::from).and_then(|f| zip::ZipArchive::new(f).map_err(Into::into)) else {
            return (None, None);
        };
        let mut meta = None;
        if let Ok(mut f) = z.by_name("pack.mcmeta") {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                meta = Some(s);
            }
        }
        let mut icon = None;
        if let Ok(mut f) = z.by_name("pack.png") {
            if f.size() <= MAX_ICON {
                let mut b = vec![];
                if f.read_to_end(&mut b).is_ok() {
                    icon = Some(b);
                }
            }
        }
        (meta, icon)
    };
    let desc = meta
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m.trim_start_matches('\u{feff}')).ok())
        .map(|v| plain(&v["pack"]["description"]).trim().to_string())
        .filter(|d| !d.is_empty());
    let icon = icon.filter(|b| b.starts_with(b"\x89PNG")).map(|b| format!("data:image/png;base64,{}", crate::scene::base64(&b)));
    (desc, icon)
}

/// How the game names a pack in `options.txt`: `file/<name>` since 1.13, the bare name before.
fn option_name(mc: &str, file: &str) -> String {
    if version_named_keys(mc) {
        format!("file/{file}")
    } else {
        file.to_string()
    }
}

fn pack_list(o: Option<&Options>, key: &str) -> Result<Vec<String>> {
    match o.and_then(|o| o.get(key)).map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(vec![]),
        Some(v) => serde_json::from_str(v).map_err(|_| anyhow!("Couldn't read the {key} setting in options.txt")),
    }
}

pub async fn list(l: &Launcher, instance_id: &str) -> Result<PackList> {
    let inst = instances::get(l, instance_id).await?;
    let d = dir(l, &inst.id);
    let tags = crate::shaders::load_pack_tags(l, &inst.id, Kind::ResourcePack).await;
    let options = read_options(&l.paths.game_dir(&inst.id).join("options.txt")).await.ok().flatten();
    let on = pack_list(options.as_ref(), "resourcePacks").unwrap_or_default();
    let mut found = vec![];
    if let Ok(mut rd) = tokio::fs::read_dir(&d).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = e.metadata().await else { continue };
            let is_pack = if meta.is_dir() { e.path().join("pack.mcmeta").is_file() } else { name.to_lowercase().ends_with(".zip") };
            if is_pack {
                found.push((name, e.path(), if meta.is_file() { meta.len() } else { 0 }));
            }
        }
    }
    let mc = inst.mc_version.clone();
    let mut packs = tokio::task::spawn_blocking(move || {
        found
            .into_iter()
            .map(|(name, path, size)| {
                let (description, icon) = read_pack(&path);
                let enabled = on.contains(&option_name(&mc, &name));
                ResourcePack { description, icon, size, enabled, online: None, file_name: name }
            })
            .collect::<Vec<_>>()
    })
    .await?;
    for p in &mut packs {
        p.online = tags.get(&p.file_name).cloned();
    }
    packs.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));
    Ok(PackList { packs, folder: d })
}

/// Turn a pack on or off for the game's next start, in `options.txt`.
pub async fn set_enabled(l: &Launcher, instance_id: &str, file: &str, enabled: bool) -> Result<()> {
    let inst = instances::get(l, instance_id).await?;
    if l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        bail!("Close {} first: the game saves which packs are on when it quits", inst.name);
    }
    let path = paths::safe_join(&dir(l, &inst.id), file)?;
    if !path.exists() {
        bail!("There's no resource pack called {file}");
    }
    let opt_path = l.paths.game_dir(&inst.id).join("options.txt");
    let existing = read_options(&opt_path).await?;
    let mut on = pack_list(existing.as_ref(), "resourcePacks")?;
    let mut incompatible = pack_list(existing.as_ref(), "incompatibleResourcePacks")?;
    let name = option_name(&inst.mc_version, file);
    let modern = version_named_keys(&inst.mc_version);
    on.retain(|p| p != &name);
    incompatible.retain(|p| p != &name);
    if enabled {
        if modern && !on.iter().any(|p| p == "vanilla") {
            on.insert(0, "vanilla".into());
        }
        // Last in the list is on top. Also listed as accepted-if-incompatible, or the
        // game would drop a pack made for another version without asking.
        on.push(name.clone());
        if modern {
            incompatible.push(name);
        }
    }
    let mut o = existing.unwrap_or_default();
    o.set("resourcePacks", serde_json::to_string(&on)?);
    if modern || !incompatible.is_empty() {
        o.set("incompatibleResourcePacks", serde_json::to_string(&incompatible)?);
    }
    tokio::fs::create_dir_all(opt_path.parent().unwrap()).await?;
    let part = opt_path.with_extension("txt.part");
    tokio::fs::write(&part, o.render()).await?;
    tokio::fs::rename(&part, &opt_path).await?;
    l.emit(Event::ModsChanged { instance_id: inst.id.clone() });
    Ok(())
}

pub async fn install(l: &Launcher, instance_id: &str, req: InstallRequest) -> Result<InstallReport> {
    crate::shaders::install_pack(l, instance_id, req, Kind::ResourcePack).await
}

pub async fn delete(l: &Launcher, instance_id: &str, file: &str) -> Result<()> {
    let inst = instances::get(l, instance_id).await?;
    let path = paths::safe_join(&dir(l, &inst.id), file)?;
    let meta = tokio::fs::metadata(&path).await.map_err(|_| anyhow!("There's no resource pack called {file}"))?;
    // Take it out of the game's list first (when the game isn't running to overwrite it).
    if !l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        let _ = set_enabled(l, &inst.id, file, false).await;
    }
    if meta.is_dir() {
        tokio::fs::remove_dir_all(&path).await?;
    } else {
        tokio::fs::remove_file(&path).await?;
    }
    crate::shaders::load_pack_tags(l, &inst.id, Kind::ResourcePack).await; // drops the tag
    l.emit(Event::ModsChanged { instance_id: inst.id.clone() });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::NewInstance;
    use crate::loaders::Loader;
    use std::io::Write;

    fn pack_zip(path: &Path, desc: &str) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("pack.mcmeta", o).unwrap();
        z.write_all(format!(r#"{{"pack":{{"pack_format":34,"description":{desc}}}}}"#).as_bytes()).unwrap();
        z.start_file("pack.png", o).unwrap();
        z.write_all(b"\x89PNG\r\n\x1a\nfake").unwrap();
        z.finish().unwrap();
    }

    #[tokio::test]
    async fn lists_and_turns_packs_on_and_off() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(tmp.path())).await.unwrap();
        let inst = instances::create(&l, NewInstance { name: Some("RP".into()), mc_version: "1.21.1".into(), loader: Loader::Vanilla, loader_version: None, ram_mb: None })
            .await
            .unwrap();
        let d = dir(&l, &inst.id);
        std::fs::create_dir_all(d.join("Folder Pack")).unwrap();
        std::fs::write(d.join("Folder Pack/pack.mcmeta"), r#"{"pack":{"pack_format":34,"description":"Unzipped"}}"#).unwrap();
        std::fs::create_dir_all(d.join("not a pack")).unwrap();
        pack_zip(&d.join("Faithful 32x.zip"), r#"[{"text":"§6Faithful"},{"text":" 32x","extra":[" for 1.21"]}]"#);
        // The game's own options: other settings and a built-in pack stay as they are.
        let opt = l.paths.game_dir(&inst.id).join("options.txt");
        std::fs::write(&opt, "fov:0.5\nresourcePacks:[\"vanilla\",\"fabric\"]\nincompatibleResourcePacks:[]\n").unwrap();

        let ls = list(&l, &inst.id).await.unwrap();
        assert_eq!(ls.packs.iter().map(|p| p.file_name.as_str()).collect::<Vec<_>>(), vec!["Faithful 32x.zip", "Folder Pack"]);
        assert_eq!(ls.packs[0].description.as_deref(), Some("Faithful 32x for 1.21"));
        assert!(ls.packs[0].icon.as_deref().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(ls.packs[1].description.as_deref(), Some("Unzipped"));
        assert!(!ls.packs[0].enabled);

        set_enabled(&l, &inst.id, "Faithful 32x.zip", true).await.unwrap();
        let text = std::fs::read_to_string(&opt).unwrap();
        assert!(text.contains("fov:0.5\n"), "{text}");
        assert!(text.contains(r#"resourcePacks:["vanilla","fabric","file/Faithful 32x.zip"]"#), "{text}");
        assert!(text.contains(r#"incompatibleResourcePacks:["file/Faithful 32x.zip"]"#), "{text}");
        assert!(list(&l, &inst.id).await.unwrap().packs[0].enabled);

        set_enabled(&l, &inst.id, "Faithful 32x.zip", false).await.unwrap();
        let text = std::fs::read_to_string(&opt).unwrap();
        assert!(text.contains(r#"resourcePacks:["vanilla","fabric"]"#), "{text}");
        assert!(set_enabled(&l, &inst.id, "../x.zip", true).await.is_err());
        assert!(set_enabled(&l, &inst.id, "Missing.zip", true).await.is_err());

        // Not while the game runs.
        let _stop = l.processes.claim(&crate::launch::game_source(&inst.id)).await.unwrap();
        assert!(set_enabled(&l, &inst.id, "Folder Pack", true).await.is_err());
        l.processes.release(&crate::launch::game_source(&inst.id)).await;

        set_enabled(&l, &inst.id, "Folder Pack", true).await.unwrap();
        delete(&l, &inst.id, "Folder Pack").await.unwrap();
        assert!(!std::fs::read_to_string(&opt).unwrap().contains("Folder Pack"));
        assert_eq!(list(&l, &inst.id).await.unwrap().packs.len(), 1);
    }

    #[tokio::test]
    async fn old_versions_use_bare_names() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(tmp.path())).await.unwrap();
        let inst = instances::create(&l, NewInstance { name: Some("Old".into()), mc_version: "1.8.9".into(), loader: Loader::Vanilla, loader_version: None, ram_mb: None })
            .await
            .unwrap();
        pack_zip(&{
            std::fs::create_dir_all(dir(&l, &inst.id)).unwrap();
            dir(&l, &inst.id).join("Old.zip")
        }, "\"old\"");
        set_enabled(&l, &inst.id, "Old.zip", true).await.unwrap();
        let text = std::fs::read_to_string(l.paths.game_dir(&inst.id).join("options.txt")).unwrap();
        assert_eq!(text, "resourcePacks:[\"Old.zip\"]\n");
    }
}
