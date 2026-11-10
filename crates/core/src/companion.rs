//! The live companion mod: generated from `live/companion`, built with the
//! same Gradle pipeline as Claude's mod projects, and installed into an
//! instance when its live mode needs it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};

use crate::instances::Instance;
use crate::loaders::Loader;
use crate::projects::ModProject;
use crate::templates::{self, ModSpec};
use crate::{browse, java, loaders, mods, mojang, Launcher};

pub const JAR_NAME: &str = "claude-live-companion.jar";
/// Slot assets are generated for this many slots; the launch setting picks how many are registered.
pub const MAX_SLOTS: u32 = 256;
const FABRIC_API_MODRINTH: &str = "P7dR8mSH";

macro_rules! sources {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../live/companion/src/main/java/dev/miocraft/live/", $path)))),*]
    };
}

const JAVA_SOURCES: &[(&str, &str)] = sources!(
    "ClaudeLive.java",
    "ClaudeLiveClient.java",
    "LiveContent.java",
    "LiveBlock.java",
    "LiveBlockItem.java",
    "LiveItem.java",
    "Behaviours.java",
    "Commands.java",
    "Textures.java",
    "RuntimeRegistrar.java",
    "api/LiveBlockBehaviour.java",
    "api/LiveItemBehaviour.java",
    "mixin/MappedRegistryAccessor.java",
    "mixin/HolderReferenceInvoker.java",
);

// ---------- tiny PNG writer (placeholder textures) ----------

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for b in data {
        c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Encode RGBA pixels as a PNG.
pub fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
    for y in 0..h as usize {
        raw.push(0); // filter: none
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&raw).unwrap();
    let idat = z.finish().unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = vec![];
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    out
}

/// A 16×16 checkerboard so unfilled slots are easy to spot.
pub fn placeholder_png(item: bool) -> Vec<u8> {
    let mut px = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let on = ((x / 4) + (y / 4)) % 2 == 0;
            let (r, g, b) = match (item, on) {
                (false, true) => (232, 130, 79),
                (false, false) => (40, 40, 44),
                (true, true) => (106, 190, 80),
                (true, false) => (40, 40, 44),
            };
            px.extend_from_slice(&[r, g, b, 255]);
        }
    }
    png_rgba(16, 16, &px)
}

// ---------- project files ----------

fn fabric_mod_json(spec: &ModSpec) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "id": "claude_live",
        "version": "1.0.0",
        "name": "Claude Live",
        "description": "MioCraft's live mode: content slots, live textures and hotswap support.",
        "authors": ["MioCraft"],
        "license": "MIT",
        "environment": "*",
        "entrypoints": {
            "main": ["dev.miocraft.live.ClaudeLive"],
            "client": ["dev.miocraft.live.ClaudeLiveClient"]
        },
        "mixins": ["claude_live.mixins.json"],
        "depends": {
            "fabricloader": format!(">={}", spec.loader_version),
            "minecraft": format!("~{}", spec.mc_version),
            "fabric-api": "*"
        }
    }))
    .unwrap()
}

const MIXINS_JSON: &str = r#"{
  "required": false,
  "package": "dev.miocraft.live.mixin",
  "compatibilityLevel": "JAVA_21",
  "mixins": ["MappedRegistryAccessor", "HolderReferenceInvoker"],
  "injectors": { "defaultRequire": 0 }
}
"#;

/// Every file of the companion project (text files only; PNGs separately).
pub fn render(spec: &ModSpec) -> (BTreeMap<String, String>, BTreeMap<String, Vec<u8>>) {
    let mut f = templates::render(spec);
    // Drop the template's example main class and metadata; ours replace them.
    f.retain(|k, _| !k.starts_with("src/main/java/") && k != "src/main/resources/fabric.mod.json" && k != "CLAUDE.md");
    let base = "src/main/java/dev/miocraft/live/";
    for (path, src) in JAVA_SOURCES {
        f.insert(format!("{base}{path}"), src.to_string());
    }
    f.insert("src/main/resources/fabric.mod.json".into(), fabric_mod_json(spec));
    f.insert("src/main/resources/claude_live.mixins.json".into(), MIXINS_JSON.into());

    let a = "src/main/resources/assets/claude_live";
    let mut lang = serde_json::Map::new();
    lang.insert("itemGroup.claude_live.live".into(), "Claude Live".into());
    let mut bin = BTreeMap::new();
    let (block_png, item_png) = (placeholder_png(false), placeholder_png(true));
    for i in 0..MAX_SLOTS {
        let b = format!("slot_block_{i}");
        let it = format!("slot_item_{i}");
        // Multipart with no condition: one model for every light level.
        f.insert(
            format!("{a}/blockstates/{b}.json"),
            format!(r#"{{"multipart":[{{"apply":{{"model":"claude_live:block/{b}"}}}}]}}"#),
        );
        f.insert(
            format!("{a}/models/block/{b}.json"),
            format!(r#"{{"parent":"minecraft:block/cube_all","textures":{{"all":"claude_live:block/{b}"}}}}"#),
        );
        f.insert(
            format!("{a}/items/{b}.json"),
            format!(r#"{{"model":{{"type":"minecraft:model","model":"claude_live:block/{b}"}}}}"#),
        );
        f.insert(
            format!("{a}/models/item/{it}.json"),
            format!(r#"{{"parent":"minecraft:item/generated","textures":{{"layer0":"claude_live:item/{it}"}}}}"#),
        );
        f.insert(
            format!("{a}/items/{it}.json"),
            format!(r#"{{"model":{{"type":"minecraft:model","model":"claude_live:item/{it}"}}}}"#),
        );
        bin.insert(format!("{a}/textures/block/{b}.png"), block_png.clone());
        bin.insert(format!("{a}/textures/item/{it}.png"), item_png.clone());
        lang.insert(format!("block.claude_live.{b}"), format!("Empty live block {i}").into());
        lang.insert(format!("item.claude_live.{it}"), format!("Empty live item {i}").into());
    }
    f.insert(format!("{a}/lang/en_us.json"), serde_json::to_string_pretty(&lang).unwrap());
    (f, bin)
}

fn hash_of(text: &BTreeMap<String, String>) -> String {
    use sha1::Digest;
    let mut h = sha1::Sha1::new();
    for (k, v) in text {
        h.update(k.as_bytes());
        h.update([0]);
        h.update(v.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

/// Path where the built companion is kept for projects to compile against
/// (its `api` package has the behaviour interfaces).
pub fn api_jar(l: &Launcher) -> PathBuf {
    l.paths.root.join("live").join("claude-live-api.jar")
}

/// Build (if needed) the companion for this instance's Minecraft and Fabric versions.
pub async fn ensure_built(l: &Launcher, inst: &Instance) -> Result<PathBuf> {
    if inst.loader != Loader::Fabric {
        bail!("the live companion is Fabric-only for now");
    }
    let loader_version = inst.loader_version.clone().ok_or_else(|| anyhow!("instance has no Fabric version"))?;
    let vanilla = mojang::fetch_version_json(l, &inst.mc_version).await?;
    let java_target = java::required_major(&vanilla);
    let toolchain = templates::toolchain(Loader::Fabric, &inst.mc_version, java_target, None)?;
    let spec = ModSpec {
        mod_id: "claude_live".into(),
        name: "Claude Live".into(),
        description: String::new(),
        author: "MioCraft".into(),
        mc_version: inst.mc_version.clone(),
        loader: Loader::Fabric,
        loader_version: loader_version.clone(),
        fabric_api_version: loaders::fabric_api_version(l, &inst.mc_version).await.ok().flatten(),
        toolchain: toolchain.clone(),
    };
    let (text, bin) = render(&spec);
    let hash = hash_of(&text);
    let dir = l.paths.root.join("live").join("companion").join(crate::instances::slug(&format!("{}-{}", inst.mc_version, loader_version)));
    let jar = dir.join("claude-live-companion.jar");
    let hash_file = dir.join("source.hash");
    if jar.exists() && tokio::fs::read_to_string(&hash_file).await.ok().as_deref() == Some(hash.as_str()) {
        return Ok(jar);
    }
    let src = dir.join("src");
    let _ = tokio::fs::remove_dir_all(&src).await;
    for (rel, contents) in &text {
        let p = crate::paths::safe_join(&src, rel)?;
        tokio::fs::create_dir_all(p.parent().unwrap()).await?;
        tokio::fs::write(&p, contents).await?;
    }
    for (rel, contents) in &bin {
        let p = crate::paths::safe_join(&src, rel)?;
        tokio::fs::create_dir_all(p.parent().unwrap()).await?;
        tokio::fs::write(&p, contents).await?;
    }
    l.progress("live", 0, 1, "Building the Claude Live companion mod (first time only)");
    let project = ModProject {
        id: "claude_live".into(),
        name: "Claude Live".into(),
        description: String::new(),
        instance_id: inst.id.clone(),
        loader: Loader::Fabric,
        mc_version: inst.mc_version.clone(),
        loader_version,
        path: src.clone(),
        toolchain,
        created_at: 0,
        last_build: None,
        pending_install: None,
    };
    let (status, lines) = crate::projects::run_gradle(l, &project, &["build", "-x", "test"], "build:claude_live").await?;
    let built = crate::projects::find_jar(&src.join("build").join("libs"));
    match (status.success(), built) {
        (true, Some(b)) => {
            tokio::fs::copy(&b, &jar).await?;
            tokio::fs::copy(&b, api_jar(l)).await?;
            tokio::fs::write(&hash_file, &hash).await?;
            l.progress("live", 1, 1, "Claude Live companion ready");
            Ok(jar)
        }
        _ => {
            let errors: Vec<String> = lines.iter().filter(|x| crate::projects::is_error_line(x)).take(30).cloned().collect();
            bail!(
                "Couldn't build the Claude Live companion mod. Its source is in {} (written for Minecraft 26.x). \
                 Errors:\n{}",
                src.display(),
                errors.join("\n")
            )
        }
    }
}

/// Install or remove the companion (and Fabric API, which it needs) to match
/// the instance's live settings. Called before every launch.
pub async fn sync(l: &Launcher, inst: &Instance) -> Result<()> {
    let target = l.paths.mods_dir(&inst.id).join(JAR_NAME);
    if !inst.live.needs_companion() {
        if target.exists() {
            tokio::fs::remove_file(&target).await?;
            l.emit(crate::Event::ModsChanged { instance_id: inst.id.clone() });
        }
        return Ok(());
    }
    let jar = ensure_built(l, inst).await?;
    tokio::fs::create_dir_all(target.parent().unwrap()).await?;
    tokio::fs::copy(&jar, &target).await?;
    let has_api = mods::list(l, &inst.id).await?.iter().any(|m| m.enabled && m.mod_id.as_deref() == Some("fabric-api"));
    if !has_api {
        l.progress("live", 0, 1, "Installing Fabric API (the companion mod needs it)");
        browse::install(
            l,
            &inst.id,
            browse::InstallRequest { source: "modrinth".into(), project_id: FABRIC_API_MODRINTH.into(), version_id: None },
        )
        .await?;
    }
    l.emit(crate::Event::ModsChanged { instance_id: inst.id.clone() });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_is_valid() {
        let p = placeholder_png(false);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn renders_companion_project() {
        let spec = ModSpec {
            mod_id: "claude_live".into(),
            name: "Claude Live".into(),
            description: String::new(),
            author: "x".into(),
            mc_version: "26.1".into(),
            loader: Loader::Fabric,
            loader_version: "0.17.2".into(),
            fabric_api_version: Some("0.140.0+26.1".into()),
            toolchain: templates::toolchain(Loader::Fabric, "26.1", 25, None).unwrap(),
        };
        let (text, bin) = render(&spec);
        assert!(text.contains_key("src/main/java/dev/miocraft/live/ClaudeLive.java"));
        assert!(!text.keys().any(|k| k.contains("com/claudemods")), "template example class removed");
        let fmj: serde_json::Value = serde_json::from_str(&text["src/main/resources/fabric.mod.json"]).unwrap();
        assert_eq!(fmj["entrypoints"]["client"][0], "dev.miocraft.live.ClaudeLiveClient");
        let bs: serde_json::Value = serde_json::from_str(&text["src/main/resources/assets/claude_live/blockstates/slot_block_255.json"]).unwrap();
        assert_eq!(bs["multipart"][0]["apply"]["model"], "claude_live:block/slot_block_255");
        assert_eq!(bin.len(), (MAX_SLOTS * 2) as usize);
        let lang: serde_json::Value = serde_json::from_str(&text["src/main/resources/assets/claude_live/lang/en_us.json"]).unwrap();
        assert_eq!(lang["item.claude_live.slot_item_3"], "Empty live item 3");
    }
}

