//! Live content definitions: `<game dir>/claude-live/content.json`, read by the
//! companion mod while the game runs. Field names match `LiveContent.java`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::instances::{self, Instance};
use crate::{http, live, projects, Launcher};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drop {
    pub item: String,
    #[serde(default = "one")]
    pub min: u32,
    #[serde(default = "one")]
    pub max: u32,
}
fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Behaviour {
    pub classes_dir: String,
    pub class_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDef {
    pub slot: i32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    pub hardness: f32,
    pub resistance: f32,
    pub light: u8,
    pub requires_tool: bool,
    #[serde(default)]
    pub drops: Vec<Drop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behaviour: Option<Behaviour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register_as: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDef {
    pub slot: i32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    pub max_stack: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behaviour: Option<Behaviour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub register_as: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Content {
    pub version: u64,
    #[serde(default)]
    pub blocks: Vec<BlockDef>,
    #[serde(default)]
    pub items: Vec<ItemDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BehaviourInput {
    /// Mod project whose compiled classes hold the behaviour.
    pub project_id: String,
    /// Fully qualified class implementing LiveBlockBehaviour / LiveItemBehaviour.
    pub class_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockInput {
    pub id: String,
    pub name: String,
    /// Absolute path to a PNG (16×16 recommended).
    pub texture_path: Option<String>,
    pub hardness: Option<f32>,
    pub resistance: Option<f32>,
    pub light: Option<u8>,
    pub requires_tool: Option<bool>,
    pub drops: Option<Vec<Drop>>,
    pub behaviour: Option<BehaviourInput>,
    /// Route 4: register as a real block instead of using a slot.
    #[serde(default)]
    pub register: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemInput {
    pub id: String,
    pub name: String,
    pub texture_path: Option<String>,
    pub max_stack: Option<u32>,
    pub behaviour: Option<BehaviourInput>,
    #[serde(default)]
    pub register: bool,
}

fn file(l: &Launcher, inst: &Instance) -> PathBuf {
    live::live_dir(l, inst).join("content.json")
}

pub async fn load(l: &Launcher, inst: &Instance) -> Content {
    http::read_json(&file(l, inst)).await.unwrap_or_default()
}

async fn save(l: &Launcher, inst: &Instance, mut c: Content) -> Result<Content> {
    c.version += 1;
    http::write_json(&file(l, inst), &c).await?;
    Ok(c)
}

fn check_id(id: &str) -> Result<()> {
    if id.len() < 2 || id.len() > 64 || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        bail!("live content ids must be 2–64 characters of a-z, 0-9 and _ (got {id:?})");
    }
    Ok(())
}

async fn copy_texture(l: &Launcher, inst: &Instance, kind: &str, id: &str, src: &str) -> Result<String> {
    let bytes = tokio::fs::read(src).await.map_err(|_| anyhow!("can't read texture {src}"))?;
    if bytes.len() > 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bail!("textures must be PNG files under 1 MB");
    }
    let rel = format!("textures/{kind}_{id}.png");
    let dst = live::live_dir(l, inst).join(&rel);
    tokio::fs::create_dir_all(dst.parent().unwrap()).await?;
    // Write a new file each time so the companion sees a new modification time.
    let tmp = dst.with_extension("png.tmp");
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, &dst).await?;
    Ok(rel)
}

async fn behaviour(l: &Launcher, b: Option<BehaviourInput>) -> Result<Option<Behaviour>> {
    let Some(b) = b else { return Ok(None) };
    let p = projects::get(l, &b.project_id).await?;
    Ok(Some(Behaviour {
        classes_dir: p.path.join("build").join("classes").join("java").join("main").to_string_lossy().into(),
        class_name: b.class_name,
    }))
}

fn free_slot(used: impl Iterator<Item = i32>, total: u32) -> Option<i32> {
    let used: std::collections::HashSet<i32> = used.collect();
    (0..total as i32).find(|s| !used.contains(s))
}

fn require(inst: &Instance, register: bool) -> Result<()> {
    if let Some(why) = live::unsupported_reason(inst) {
        bail!(why);
    }
    if register && !inst.live.unfreeze_registries {
        bail!("Registering real content needs the experimental \"Unfreeze registries\" live mode setting.");
    }
    if !register && !inst.live.content_slots {
        bail!("Turn on \"Content slots\" in the instance's live mode settings first.");
    }
    Ok(())
}

/// Namespace for runtime-registered content: the behaviour's project, or claude_live.
fn namespace(l: &Launcher, b: &Option<Behaviour>) -> String {
    b.as_ref()
        .and_then(|b| Path::new(&b.classes_dir).ancestors().nth(4).and_then(|p| p.file_name()).map(|f| f.to_string_lossy().to_string()))
        .filter(|ns| ns.starts_with(|c: char| c.is_ascii_lowercase()) && ns.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or_else(|| {
            let _ = l;
            "claude_live".into()
        })
}

/// Add or update a live block (matched by id).
pub async fn define_block(l: &Launcher, instance_id: &str, b: BlockInput) -> Result<BlockDef> {
    let inst = instances::get(l, instance_id).await?;
    check_id(&b.id)?;
    require(&inst, b.register)?;
    let mut c = load(l, &inst).await;
    let existing = c.blocks.iter().position(|d| d.id == b.id);
    let beh = behaviour(l, b.behaviour).await?;
    let texture = match &b.texture_path {
        Some(p) => Some(copy_texture(l, &inst, "block", &b.id, p).await?),
        None => existing.and_then(|i| c.blocks[i].texture.clone()),
    };
    let prev = existing.map(|i| c.blocks[i].clone());
    if let Some(p) = &prev {
        if p.register_as.is_some() != b.register {
            bail!("{} already exists as a {}; remove it first", b.id, if p.register_as.is_some() { "registered block" } else { "slot block" });
        }
    }
    let (slot, register_as) = if b.register {
        let reg = prev.as_ref().and_then(|p| p.register_as.clone()).unwrap_or_else(|| format!("{}:{}", namespace(l, &beh), b.id));
        (-1, Some(reg))
    } else {
        let slot = match &prev {
            Some(p) => p.slot,
            None => free_slot(c.blocks.iter().filter(|d| d.register_as.is_none()).map(|d| d.slot), inst.live.slot_count)
                .ok_or_else(|| anyhow!("all {} block slots are used; remove one or raise the slot count", inst.live.slot_count))?,
        };
        (slot, None)
    };
    let def = BlockDef {
        slot,
        id: b.id.clone(),
        name: b.name,
        texture,
        hardness: b.hardness.or(prev.as_ref().map(|p| p.hardness)).unwrap_or(1.5),
        resistance: b.resistance.or(prev.as_ref().map(|p| p.resistance)).unwrap_or(6.0),
        light: b.light.or(prev.as_ref().map(|p| p.light)).unwrap_or(0).min(15),
        requires_tool: b.requires_tool.or(prev.as_ref().map(|p| p.requires_tool)).unwrap_or(false),
        drops: b.drops.or(prev.as_ref().map(|p| p.drops.clone())).unwrap_or_default(),
        behaviour: beh.or(prev.as_ref().and_then(|p| p.behaviour.clone())),
        register_as,
    };
    match existing {
        Some(i) => c.blocks[i] = def.clone(),
        None => c.blocks.push(def.clone()),
    }
    if def.register_as.is_some() {
        write_registered_assets(l, &inst, &c).await?;
    }
    save(l, &inst, c).await?;
    Ok(def)
}

/// Add or update a live item (matched by id).
pub async fn define_item(l: &Launcher, instance_id: &str, it: ItemInput) -> Result<ItemDef> {
    let inst = instances::get(l, instance_id).await?;
    check_id(&it.id)?;
    require(&inst, it.register)?;
    let mut c = load(l, &inst).await;
    let existing = c.items.iter().position(|d| d.id == it.id);
    let prev = existing.map(|i| c.items[i].clone());
    if let Some(p) = &prev {
        if p.register_as.is_some() != it.register {
            bail!("{} already exists with a different kind; remove it first", it.id);
        }
    }
    let beh = behaviour(l, it.behaviour).await?;
    let texture = match &it.texture_path {
        Some(p) => Some(copy_texture(l, &inst, "item", &it.id, p).await?),
        None => prev.as_ref().and_then(|p| p.texture.clone()),
    };
    let (slot, register_as) = if it.register {
        (-1, Some(prev.as_ref().and_then(|p| p.register_as.clone()).unwrap_or_else(|| format!("{}:{}", namespace(l, &beh), it.id))))
    } else {
        let slot = match &prev {
            Some(p) => p.slot,
            None => free_slot(c.items.iter().filter(|d| d.register_as.is_none()).map(|d| d.slot), inst.live.slot_count)
                .ok_or_else(|| anyhow!("all {} item slots are used", inst.live.slot_count))?,
        };
        (slot, None)
    };
    let def = ItemDef {
        slot,
        id: it.id,
        name: it.name,
        texture,
        max_stack: it.max_stack.or(prev.as_ref().map(|p| p.max_stack)).unwrap_or(64).clamp(1, 99),
        behaviour: beh.or(prev.as_ref().and_then(|p| p.behaviour.clone())),
        register_as,
    };
    match existing {
        Some(i) => c.items[i] = def.clone(),
        None => c.items.push(def.clone()),
    }
    if def.register_as.is_some() {
        write_registered_assets(l, &inst, &c).await?;
    }
    save(l, &inst, c).await?;
    Ok(def)
}

pub async fn remove(l: &Launcher, instance_id: &str, kind: &str, id: &str) -> Result<()> {
    let inst = instances::get(l, instance_id).await?;
    let mut c = load(l, &inst).await;
    let before = c.blocks.len() + c.items.len();
    match kind {
        "block" => c.blocks.retain(|d| d.id != id),
        "item" => c.items.retain(|d| d.id != id),
        _ => bail!("kind must be block or item"),
    }
    if c.blocks.len() + c.items.len() == before {
        bail!("no live {kind} called {id}");
    }
    save(l, &inst, c).await?;
    Ok(())
}

/// The registry id the game knows this content by.
pub fn game_id(kind: &str, slot: i32, register_as: &Option<String>) -> String {
    match register_as {
        Some(r) => r.clone(),
        None => format!("claude_live:slot_{kind}_{slot}"),
    }
}

/// Queue a command for the running singleplayer game (run by the companion).
pub async fn queue_command(l: &Launcher, instance_id: &str, command: &str) -> Result<()> {
    let inst = instances::get(l, instance_id).await?;
    if !inst.live.needs_companion() {
        bail!("Game commands need live mode content slots (or registry unfreezing) on, so the companion mod is loaded.");
    }
    if !l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        bail!("{} isn't running", inst.name);
    }
    let cmd = command.trim().replace(['\r', '\n'], " ");
    if cmd.is_empty() {
        bail!("empty command");
    }
    use tokio::io::AsyncWriteExt;
    let path = live::live_dir(l, &inst).join("commands.txt");
    let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await?;
    f.write_all(format!("{cmd}\n").as_bytes()).await?;
    Ok(())
}

pub async fn give(l: &Launcher, instance_id: &str, kind: &str, id: &str, count: u32) -> Result<String> {
    let inst = instances::get(l, instance_id).await?;
    let c = load(l, &inst).await;
    let gid = match kind {
        "block" => c.blocks.iter().find(|d| d.id == id).map(|d| game_id("block", d.slot, &d.register_as)),
        "item" => c.items.iter().find(|d| d.id == id).map(|d| game_id("item", d.slot, &d.register_as)),
        _ => bail!("kind must be block or item"),
    }
    .ok_or_else(|| anyhow!("no live {kind} called {id}"))?;
    queue_command(l, instance_id, &format!("give @p {gid} {}", count.clamp(1, 6400))).await?;
    Ok(gid)
}

async fn put(pack: &Path, rel: String, body: String) -> Result<()> {
    let p = crate::paths::safe_join(pack, &rel)?;
    tokio::fs::create_dir_all(p.parent().unwrap()).await?;
    tokio::fs::write(p, body).await?;
    Ok(())
}

/// Route 4 content needs its own models: write them into the claude-live resource pack.
async fn write_registered_assets(l: &Launcher, inst: &Instance, c: &Content) -> Result<()> {
    let pack = l.paths.game_dir(&inst.id).join("resourcepacks").join("claude-live");
    let live_dir = live::live_dir(l, inst);
    tokio::fs::create_dir_all(&pack).await?;
    let meta = pack.join("pack.mcmeta");
    if !meta.exists() {
        tokio::fs::write(&meta, r#"{"pack":{"description":"Claude Live textures","pack_format":64,"min_format":1,"max_format":10000}}"#).await?;
    }
    let mut langs: std::collections::HashMap<String, serde_json::Map<String, serde_json::Value>> = Default::default();
    for d in c.blocks.iter() {
        let Some(reg) = &d.register_as else { continue };
        let (ns, path) = reg.split_once(':').ok_or_else(|| anyhow!("bad id {reg}"))?;
        put(&pack, format!("assets/{ns}/blockstates/{path}.json"), format!(r#"{{"multipart":[{{"apply":{{"model":"{ns}:block/{path}"}}}}]}}"#)).await?;
        put(&pack, format!("assets/{ns}/models/block/{path}.json"), format!(r#"{{"parent":"minecraft:block/cube_all","textures":{{"all":"{ns}:block/{path}"}}}}"#)).await?;
        put(&pack, format!("assets/{ns}/items/{path}.json"), format!(r#"{{"model":{{"type":"minecraft:model","model":"{ns}:block/{path}"}}}}"#)).await?;
        if let Some(t) = &d.texture {
            let dst = crate::paths::safe_join(&pack, &format!("assets/{ns}/textures/block/{path}.png"))?;
            tokio::fs::create_dir_all(dst.parent().unwrap()).await?;
            tokio::fs::copy(live_dir.join(t), dst).await?;
        }
        langs.entry(ns.to_string()).or_default().insert(format!("block.{ns}.{path}"), d.name.clone().into());
    }
    for d in c.items.iter() {
        let Some(reg) = &d.register_as else { continue };
        let (ns, path) = reg.split_once(':').ok_or_else(|| anyhow!("bad id {reg}"))?;
        put(&pack, format!("assets/{ns}/models/item/{path}.json"), format!(r#"{{"parent":"minecraft:item/generated","textures":{{"layer0":"{ns}:item/{path}"}}}}"#)).await?;
        put(&pack, format!("assets/{ns}/items/{path}.json"), format!(r#"{{"model":{{"type":"minecraft:model","model":"{ns}:item/{path}"}}}}"#)).await?;
        if let Some(t) = &d.texture {
            let dst = crate::paths::safe_join(&pack, &format!("assets/{ns}/textures/item/{path}.png"))?;
            tokio::fs::create_dir_all(dst.parent().unwrap()).await?;
            tokio::fs::copy(live_dir.join(t), dst).await?;
        }
        langs.entry(ns.to_string()).or_default().insert(format!("item.{ns}.{path}"), d.name.clone().into());
    }
    for (ns, map) in langs {
        put(&pack, format!("assets/{ns}/lang/en_us.json"), serde_json::to_string_pretty(&map)?).await?;
    }
    // Tell the companion to reload resources.
    let version = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    tokio::fs::write(pack.join("pack.version"), version.to_string()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::Loader;

    async fn setup() -> (tempfile::TempDir, Launcher, Instance) {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(dir.path())).await.unwrap();
        let mut inst = instances::create(&l, instances::NewInstance {
            name: Some("Live".into()), mc_version: "26.1".into(), loader: Loader::Fabric,
            loader_version: Some("0.17.2".into()), ram_mb: None,
        }).await.unwrap();
        inst.live = live::LiveSettings { content_slots: true, unfreeze_registries: true, slot_count: 2, ..Default::default() };
        instances::save(&l, &inst).await.unwrap();
        (dir, l, inst)
    }

    fn block(id: &str) -> BlockInput {
        BlockInput { id: id.into(), name: "Ruby Ore".into(), texture_path: None, hardness: Some(3.0), resistance: None,
            light: Some(20), requires_tool: Some(true), drops: None, behaviour: None, register: false }
    }

    #[tokio::test]
    async fn slots_are_assigned_and_reused() {
        let (dir, l, inst) = setup().await;
        let png = dir.path().join("t.png");
        std::fs::write(&png, crate::companion::placeholder_png(false)).unwrap();

        let a = define_block(&l, &inst.id, BlockInput { texture_path: Some(png.to_string_lossy().into()), ..block("ruby_ore") }).await.unwrap();
        assert_eq!(a.slot, 0);
        assert_eq!(a.light, 15, "clamped");
        assert_eq!(a.texture.as_deref(), Some("textures/block_ruby_ore.png"));
        let b = define_block(&l, &inst.id, block("sapphire_ore")).await.unwrap();
        assert_eq!(b.slot, 1);
        assert!(define_block(&l, &inst.id, block("third")).await.is_err(), "only 2 slots");

        // Updating keeps the slot and the texture.
        let a2 = define_block(&l, &inst.id, BlockInput { name: "Ruby Ore (shiny)".into(), hardness: None, ..block("ruby_ore") }).await.unwrap();
        assert_eq!((a2.slot, a2.hardness, a2.texture.is_some()), (0, 3.0, true));

        let c = load(&l, &inst).await;
        assert_eq!(c.version, 3);
        // The JSON must match what LiveContent.java expects.
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(file(&l, &inst)).unwrap()).unwrap();
        assert_eq!(raw["blocks"][0]["requires_tool"], true);
        assert_eq!(raw["blocks"][0]["slot"], 0);

        remove(&l, &inst.id, "block", "ruby_ore").await.unwrap();
        let reuse = define_block(&l, &inst.id, block("emerald_ore")).await.unwrap();
        assert_eq!(reuse.slot, 0, "freed slot is reused");
        assert!(define_block(&l, &inst.id, block("Bad Id")).await.is_err());
    }

    #[tokio::test]
    async fn registered_content_writes_pack_assets() {
        let (dir, l, inst) = setup().await;
        let png = dir.path().join("t.png");
        std::fs::write(&png, crate::companion::placeholder_png(true)).unwrap();
        let d = define_block(&l, &inst.id, BlockInput { register: true, texture_path: Some(png.to_string_lossy().into()), ..block("ruby_block") }).await.unwrap();
        assert_eq!(d.register_as.as_deref(), Some("claude_live:ruby_block"));
        assert_eq!(d.slot, -1);
        let pack = l.paths.game_dir(&inst.id).join("resourcepacks/claude-live");
        assert!(pack.join("assets/claude_live/blockstates/ruby_block.json").exists());
        assert!(pack.join("assets/claude_live/textures/block/ruby_block.png").exists());
        assert!(pack.join("pack.version").exists());
        let lang: serde_json::Value = serde_json::from_slice(&std::fs::read(pack.join("assets/claude_live/lang/en_us.json")).unwrap()).unwrap();
        assert_eq!(lang["block.claude_live.ruby_block"], "Ruby Ore");
        assert!(define_block(&l, &inst.id, block("ruby_block")).await.is_err(), "can't switch kind");
    }

    #[tokio::test]
    async fn needs_the_right_toggles() {
        let (_d, l, mut inst) = setup().await;
        inst.live = live::LiveSettings::default();
        instances::save(&l, &inst).await.unwrap();
        assert!(define_block(&l, &inst.id, block("x_ore")).await.unwrap_err().to_string().contains("Content slots"));
        assert!(define_block(&l, &inst.id, BlockInput { register: true, ..block("x_ore") }).await.unwrap_err().to_string().contains("Unfreeze"));
    }
}
