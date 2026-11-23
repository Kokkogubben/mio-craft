//! Files dragged onto the launcher: mod jars, shader packs and resource packs,
//! each put in the right folder of an instance. (.mrpack modpacks go through
//! `modpack::import` instead, since they make a new instance.)

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::instances::Instance;
use crate::loaders::Loader;
use crate::{paths, Event, Launcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Mod,
    Shader,
    ResourcePack,
}

#[derive(Debug, Serialize)]
pub struct Added {
    pub file_name: String,
    pub kind: Kind,
    /// Set when the file was added but probably won't work (a mod for another loader).
    pub warning: Option<String>,
}

const UNSUPPORTED: &str =
    "MioCraft can add mods (.jar), shader and resource packs (.zip) and modpacks (.mrpack)";

/// What a dropped file is, from its name and (for zips) what's inside.
pub fn classify(name: &str, bytes: &[u8]) -> Result<Kind> {
    let lower = name.to_ascii_lowercase();
    if !bytes.starts_with(b"PK") {
        bail!("{name} isn't a zip or jar file. {UNSUPPORTED}.");
    }
    if lower.ends_with(".jar") {
        return Ok(Kind::Mod);
    }
    if !lower.ends_with(".zip") {
        bail!("{UNSUPPORTED}, not {name}.");
    }
    let z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).with_context(|| format!("reading {name}"))?;
    let names: Vec<String> = z.file_names().map(|n| n.replace('\\', "/")).collect();
    // Shader packs keep their programs in shaders/ (sometimes one folder down).
    let shader = names.iter().any(|n| {
        let mut parts = n.split('/');
        let first = parts.next().unwrap_or("");
        first == "shaders" || parts.next() == Some("shaders")
    });
    if shader {
        return Ok(Kind::Shader);
    }
    if names.iter().any(|n| n == "pack.mcmeta") {
        return Ok(Kind::ResourcePack);
    }
    // A mod shipped as a zip (old Forge did this).
    const MOD_META: [&str; 5] = ["fabric.mod.json", "quilt.mod.json", "META-INF/mods.toml", "META-INF/neoforge.mods.toml", "mcmod.info"];
    if names.iter().any(|n| MOD_META.contains(&n.as_str())) {
        return Ok(Kind::Mod);
    }
    bail!("{name} doesn't look like a shader pack or a resource pack.")
}

/// Whether an instance's loader runs mods made for `mod_loader` (as `mods::read_metadata` names it).
/// Unknown jars are given the benefit of the doubt.
pub fn loader_runs(instance: Loader, mod_loader: &str) -> bool {
    match (instance, mod_loader) {
        (_, "unknown") => true,
        (Loader::Fabric | Loader::LegacyFabric, l) => l == "fabric",
        (Loader::Quilt, l) => l == "fabric" || l == "quilt",
        (Loader::Forge, l) => l == "forge",
        // Early NeoForge (1.20.1 to 1.20.4) used Forge's mods.toml.
        (Loader::Neoforge, l) => l == "neoforge" || l == "forge",
        (Loader::Vanilla, _) => false,
    }
}

fn folder(l: &Launcher, instance_id: &str, kind: Kind) -> PathBuf {
    match kind {
        Kind::Mod => l.paths.mods_dir(instance_id),
        Kind::Shader => crate::shaders::dir(l, instance_id),
        Kind::ResourcePack => l.paths.game_dir(instance_id).join("resourcepacks"),
    }
}

/// Put a dropped file into the instance (replacing a file of the same name).
pub async fn add(l: &Launcher, inst: &Instance, name: &str, bytes: Vec<u8>) -> Result<Added> {
    let instance_id = inst.id.as_str();
    let file_name = std::path::Path::new(name)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .filter(|f| !f.is_empty() && f == name)
        .ok_or_else(|| anyhow::anyhow!("bad file name {name:?}"))?;
    let kind = classify(&file_name, &bytes)?;
    if kind == Kind::Mod && inst.loader == Loader::Vanilla {
        bail!("Vanilla instances can't load mods. Make an instance with a mod loader for {file_name}.");
    }
    let dir = folder(l, instance_id, kind);
    tokio::fs::create_dir_all(&dir).await?;
    let dest = paths::safe_join(&dir, &file_name)?;
    let part = paths::safe_join(&dir, &format!("{file_name}.part"))?;
    tokio::fs::write(&part, &bytes).await?;
    tokio::fs::rename(&part, &dest).await?;
    let mut warning = None;
    if kind == Kind::Mod {
        // A disabled copy of the same mod would otherwise sit next to the new one.
        let _ = tokio::fs::remove_file(dir.join(format!("{file_name}.disabled"))).await;
        let d = dest.clone();
        if let Ok(Ok(meta)) = tokio::task::spawn_blocking(move || crate::mods::read_metadata(&d)).await {
            if !loader_runs(inst.loader, &meta.loader) {
                warning = Some(format!(
                    "{} looks like a {} mod, so it probably won't load in this {} instance",
                    meta.name.as_deref().unwrap_or(&file_name),
                    match meta.loader.as_str() { "neoforge" => "NeoForge", "forge" => "Forge", "quilt" => "Quilt", _ => "Fabric" },
                    inst.loader.display_name()
                ));
            }
        }
    }
    l.emit(Event::ModsChanged { instance_id: instance_id.into() });
    Ok(Added { file_name, kind, warning })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(files: &[&str]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for f in files {
                z.start_file(*f, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(b"{}").unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn classifies() {
        assert_eq!(classify("a.jar", &zip_with(&["fabric.mod.json"])).unwrap(), Kind::Mod);
        assert_eq!(classify("BSL.zip", &zip_with(&["shaders/final.fsh"])).unwrap(), Kind::Shader);
        assert_eq!(classify("Nested.zip", &zip_with(&["Nested/shaders/final.fsh"])).unwrap(), Kind::Shader);
        assert_eq!(classify("Faithful.zip", &zip_with(&["pack.mcmeta", "assets/x.png"])).unwrap(), Kind::ResourcePack);
        assert_eq!(classify("old.zip", &zip_with(&["mcmod.info"])).unwrap(), Kind::Mod);
        assert!(classify("x.zip", &zip_with(&["readme.txt"])).is_err());
        assert!(classify("x.txt", b"hello").is_err());
        assert!(classify("x.exe", &zip_with(&["a"])).is_err());
    }

    #[test]
    fn loaders() {
        assert!(loader_runs(Loader::Quilt, "fabric"));
        assert!(!loader_runs(Loader::Fabric, "forge"));
        assert!(loader_runs(Loader::Neoforge, "forge"));
        assert!(!loader_runs(Loader::Forge, "neoforge"));
        assert!(loader_runs(Loader::Forge, "unknown"));
    }
}
