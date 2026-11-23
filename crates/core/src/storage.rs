//! The Storage page: what takes up space in the data folder, and removing what
//! nothing uses any more.
//!
//! Sizes never follow links or junctions. A delete names a part and a key; the
//! core scans again, checks the item is still unused, and works out the path
//! itself. Folders are renamed before they're removed, so a folder that's still
//! open somewhere fails as a whole instead of being half deleted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::Value;

use crate::{instances, java, mojang, projects, Launcher};

/// Held while Gradle runs, so Storage won't remove files a build is using.
pub(crate) struct GradleRun(Launcher);

impl GradleRun {
    pub(crate) fn start(l: &Launcher) -> Self {
        l.gradle_runs.fetch_add(1, Ordering::SeqCst);
        GradleRun(l.clone())
    }
}

impl Drop for GradleRun {
    fn drop(&mut self) {
        self.0.gradle_runs.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub key: String,
    pub label: String,
    pub size: u64,
    /// What uses it, or a short note.
    pub detail: String,
    /// It can be removed from the Storage page.
    pub removable: bool,
    /// The folder to open, if there's one worth showing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub total: u64,
    pub items: Vec<Item>,
    /// Why nothing here can be removed right now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub const PARTS: [&str; 7] = ["instances", "versions", "assets", "libraries", "java", "gradle", "other"];

/// A folder left behind by a delete that couldn't finish.
const DELETING: &str = ".deleting";

// ---------- sizes ----------

/// Bytes in a file or folder, not following links or junctions.
fn size_of(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if m.file_type().is_symlink() {
        return 0;
    }
    if !m.is_dir() {
        return if m.is_file() { m.len() } else { 0 };
    }
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

async fn size(p: PathBuf) -> u64 {
    tokio::task::spawn_blocking(move || size_of(&p)).await.unwrap_or(0)
}

/// Plain folders and files in a folder (no links), by name.
fn entries(dir: &Path) -> Vec<(String, PathBuf, bool)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            if ft.is_symlink() {
                return None;
            }
            Some((e.file_name().to_string_lossy().to_string(), e.path(), ft.is_dir()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn names(list: &[String]) -> String {
    match list.len() {
        0 => String::new(),
        1..=3 => list.join(", "),
        n => format!("{}, {} and {} more", list[0], list[1], n - 2),
    }
}

// ---------- what's in use ----------

/// Which versions the instances need, with the names of the instances that need them.
struct VersionUse {
    /// False when an instance or version file couldn't be read: then nothing is removed.
    sure: bool,
    used: HashMap<String, Vec<String>>,
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn version_use(l: &Launcher) -> VersionUse {
    let mut sure = true;
    let mut used: HashMap<String, Vec<String>> = HashMap::new();
    for (name, dir, is_dir) in entries(&l.paths.instances()) {
        if !is_dir || name.ends_with(DELETING) {
            continue;
        }
        let file = dir.join("instance.json");
        if !file.exists() {
            continue;
        }
        let Some(inst) = read_json(&file).and_then(|v| serde_json::from_value::<instances::Instance>(v).ok()) else {
            sure = false;
            continue;
        };
        let mut todo: Vec<String> = inst.version_id.iter().cloned().chain([inst.mc_version.clone()]).collect();
        let mut seen = HashSet::new();
        while let Some(id) = todo.pop() {
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            used.entry(id.clone()).or_default().push(inst.name.clone());
            if !l.paths.version_dir(&id).exists() {
                continue;
            }
            match read_json(&l.paths.version_json(&id)) {
                Some(v) => {
                    if let Some(parent) = v.get("inheritsFrom").and_then(Value::as_str) {
                        todo.push(parent.to_string());
                    }
                }
                None => sure = false,
            }
        }
    }
    for list in used.values_mut() {
        list.sort();
        list.dedup();
    }
    VersionUse { sure, used }
}

const UNSURE: &str = "Some instance or version files couldn't be read, so nothing here can be removed. Check the instances and try again.";

// ---------- the parts ----------

pub async fn part(l: &Launcher, name: &str) -> Result<Part> {
    let mut note = None;
    let items = match name {
        "instances" => instances_part(l).await?,
        "versions" => {
            let (items, sure) = versions_part(l).await;
            if !sure {
                note = Some(UNSURE.into());
            }
            items
        }
        "assets" => {
            let (items, sure) = assets_part(l).await;
            if !sure {
                note = Some(UNSURE.into());
            }
            items
        }
        "libraries" => vec![Item {
            key: "libraries".into(),
            label: "Libraries".into(),
            size: size(l.paths.libraries()).await,
            detail: "Shared by every Minecraft version and mod loader".into(),
            removable: false,
            path: Some(l.paths.libraries()),
        }],
        "java" => java_part(l).await,
        "gradle" => gradle_part(l).await,
        "other" => other_part(l).await,
        _ => bail!("no such part of Storage: {name}"),
    };
    Ok(Part { total: items.iter().map(|i| i.size).sum(), items, note })
}

async fn instances_part(l: &Launcher) -> Result<Vec<Item>> {
    let mut out = vec![];
    for inst in instances::list(l).await? {
        let dir = l.paths.instance_dir(&inst.id);
        let game = l.paths.game_dir(&inst.id);
        let (all, worlds, mods) = tokio::join!(size(dir.clone()), size(game.join("saves")), size(game.join("mods")));
        let mut parts = vec![];
        if worlds > 0 {
            parts.push(format!("worlds {}", human(worlds)));
        }
        if mods > 0 {
            parts.push(format!("mods {}", human(mods)));
        }
        out.push(Item { key: inst.id, label: inst.name, size: all, detail: parts.join(" · "), removable: false, path: Some(dir) });
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    Ok(out)
}

async fn versions_part(l: &Launcher) -> (Vec<Item>, bool) {
    let l2 = l.clone();
    let vu = tokio::task::spawn_blocking(move || version_use(&l2)).await.unwrap_or(VersionUse { sure: false, used: HashMap::new() });
    let mut out = vec![];
    for (name, path, is_dir) in entries(&l.paths.versions()) {
        if !is_dir {
            continue;
        }
        let leftover = name.ends_with(DELETING);
        let users = vu.used.get(&name).cloned().unwrap_or_default();
        let detail = if leftover {
            "Left over from an earlier clean-up".into()
        } else if users.is_empty() {
            "Not used by any instance".into()
        } else {
            format!("Used by {}", names(&users))
        };
        out.push(Item { key: name.clone(), label: name, size: size(path).await, detail, removable: leftover || (vu.sure && users.is_empty()), path: None });
    }
    (out, vu.sure)
}

/// The asset files no kept version needs: unused indexes, their loose copies and objects.
struct AssetPlan {
    sure: bool,
    used_indexes: HashSet<String>,
    unused: Vec<PathBuf>,
}

fn asset_plan(l: &Launcher) -> AssetPlan {
    let vu = version_use(l);
    let assets = l.paths.assets();
    let mut sure = vu.sure;
    let mut used_indexes = HashSet::new();
    for id in vu.used.keys() {
        if !l.paths.version_dir(id).exists() {
            continue;
        }
        let Some(v) = read_json(&l.paths.version_json(id)) else {
            sure = false;
            continue;
        };
        if let Some(ix) = v.pointer("/assetIndex/id").and_then(Value::as_str) {
            used_indexes.insert(ix.to_string());
        }
    }
    let mut keep = HashSet::new();
    for ix in &used_indexes {
        let p = assets.join("indexes").join(format!("{ix}.json"));
        if !p.exists() {
            continue; // downloaded again on the next launch, with its objects
        }
        match read_json(&p).and_then(|v| v.get("objects").cloned()) {
            Some(Value::Object(objs)) => {
                for o in objs.values() {
                    if let Some(h) = o.get("hash").and_then(Value::as_str) {
                        keep.insert(h.to_string());
                    }
                }
            }
            _ => sure = false,
        }
    }
    let mut unused = vec![];
    for (name, path, is_dir) in entries(&assets.join("indexes")) {
        if !is_dir && name.ends_with(".json") && !used_indexes.contains(name.trim_end_matches(".json")) {
            unused.push(path);
        }
    }
    for (name, path, is_dir) in entries(&assets.join("virtual")) {
        if is_dir && !used_indexes.contains(&name) {
            unused.push(path);
        }
    }
    for (_, sub, is_dir) in entries(&assets.join("objects")) {
        if !is_dir {
            continue;
        }
        for (hash, path, is_dir) in entries(&sub) {
            if !is_dir && !keep.contains(&hash) {
                unused.push(path);
            }
        }
    }
    AssetPlan { sure, used_indexes, unused }
}

async fn assets_part(l: &Launcher) -> (Vec<Item>, bool) {
    let l2 = l.clone();
    let Ok(plan) = tokio::task::spawn_blocking(move || asset_plan(&l2)).await else { return (vec![], false) };
    let all = size(l.paths.assets()).await;
    let unused_paths = plan.unused.clone();
    let unused = tokio::task::spawn_blocking(move || unused_paths.iter().map(|p| size_of(p)).sum::<u64>()).await.unwrap_or(0);
    let mut idx: Vec<_> = plan.used_indexes.into_iter().collect();
    idx.sort();
    let mut items = vec![Item {
        key: "used".into(),
        label: "Sounds, languages and other game assets".into(),
        size: all.saturating_sub(unused),
        detail: if idx.is_empty() { "Not used by any instance yet".into() } else { format!("Asset sets in use: {}", names(&idx)) },
        removable: false,
        path: None,
    }];
    if unused > 0 {
        items.push(Item {
            key: "unused".into(),
            label: "Unused game assets".into(),
            size: unused,
            detail: "From versions no instance uses any more".into(),
            removable: plan.sure,
            path: None,
        });
    }
    (items, plan.sure)
}

async fn java_part(l: &Launcher) -> Vec<Item> {
    // Which Java versions instances and mod projects need.
    let mut need: HashMap<u32, Vec<String>> = HashMap::new();
    let mut live = vec![];
    for inst in instances::list(l).await.unwrap_or_default() {
        // Not installed yet: the Minecraft version says which Java it will need.
        let vid = inst.version_id.as_deref().unwrap_or(&inst.mc_version);
        if let Ok((v, _)) = mojang::resolve(l, vid).await {
            need.entry(java::required_major(&v)).or_default().push(inst.name.clone());
        }
        if inst.live.hotswap {
            live.push(inst.name.clone());
        }
    }
    for p in projects::list(l).await.unwrap_or_default() {
        for major in [p.toolchain.java_target, p.toolchain.gradle_jdk] {
            let list = need.entry(major).or_default();
            if !list.contains(&p.name) {
                list.push(p.name.clone());
            }
        }
    }
    let mut out = vec![];
    for (name, path, is_dir) in entries(&l.paths.java()) {
        let leftover = name.ends_with(DELETING) || name.ends_with(".tmp");
        let users = if let Some(m) = name.strip_prefix("jdk-").and_then(|m| m.parse::<u32>().ok()) {
            need.get(&m).cloned().unwrap_or_default()
        } else if name.starts_with("jbr-") && !leftover {
            live.clone()
        } else {
            vec![]
        };
        let label = if let Some(m) = name.strip_prefix("jdk-").filter(|_| !leftover) {
            format!("Java {m}")
        } else if let Some(v) = name.strip_prefix("jbr-").filter(|_| !leftover) {
            format!("JetBrains Runtime {v} (live hotswap)")
        } else {
            name.clone()
        };
        let detail = if leftover {
            "Left over from an unfinished download or clean-up".into()
        } else if users.is_empty() {
            "Nothing needs it right now. Downloaded again if something does".into()
        } else {
            format!("Used by {}", names(&users))
        };
        out.push(Item { key: name, label, size: size(path).await, detail, removable: users.is_empty() && is_dir, path: None });
    }
    out
}

async fn gradle_part(l: &Launcher) -> Vec<Item> {
    let mut need: HashMap<String, Vec<String>> = HashMap::new();
    for p in projects::list(l).await.unwrap_or_default() {
        need.entry(p.toolchain.gradle_version.clone()).or_default().push(p.name.clone());
    }
    let mut out = vec![];
    for (name, path, is_dir) in entries(&l.paths.gradle()) {
        if !is_dir {
            continue;
        }
        let item = if name == "home" {
            Item {
                key: name,
                label: "Gradle cache".into(),
                size: size(path).await,
                detail: "Libraries and tools downloaded by mod project builds. The next build downloads what it needs again, which can take a while".into(),
                removable: true,
                path: None,
            }
        } else if let Some(v) = name.strip_prefix("gradle-").filter(|_| !name.ends_with(DELETING) && !name.ends_with(".tmp")) {
            let users = need.get(v).cloned().unwrap_or_default();
            Item {
                label: format!("Gradle {v}"),
                detail: if users.is_empty() { "No mod project uses it. Downloaded again if one does".into() } else { format!("Used by {}", names(&users)) },
                removable: users.is_empty(),
                size: size(path).await,
                key: name,
                path: None,
            }
        } else {
            Item { label: name.clone(), detail: "Left over from an unfinished download or clean-up".into(), removable: true, size: size(path).await, key: name, path: None }
        };
        out.push(item);
    }
    out
}

/// Downloads kept in the cache folder that nothing reads again: loader installers,
/// leftover archives and imports.
fn cache_leftovers(l: &Launcher) -> Vec<PathBuf> {
    entries(&l.paths.cache())
        .into_iter()
        .filter(|(name, _, is_dir)| {
            if *is_dir {
                return name.ends_with(DELETING) || name.ends_with(".tmp");
            }
            (name.ends_with("-installer.jar") || name.ends_with("-installer.jar.log"))
                || (name.starts_with("import-") && name.ends_with(".mrpack"))
                || ((name.starts_with("jdk-") || name.starts_with("jbr-")) && (name.ends_with(".zip") || name.ends_with(".tar.gz")))
                || (name.starts_with("gradle-") && name.ends_with("-bin.zip"))
                || name.ends_with(".part")
        })
        .map(|(_, p, _)| p)
        .collect()
}

/// Folders anywhere at the top of the data folder's sections that a delete left behind.
fn unfinished(l: &Launcher) -> Vec<PathBuf> {
    [l.paths.instances(), l.paths.projects()]
        .iter()
        .flat_map(|d| entries(d))
        .filter(|(name, _, is_dir)| *is_dir && name.ends_with(DELETING))
        .map(|(_, p, _)| p)
        .collect()
}

async fn other_part(l: &Launcher) -> Vec<Item> {
    let mut out = vec![];
    for p in projects::list(l).await.unwrap_or_default() {
        out.push(Item { key: format!("project:{}", p.id), label: format!("Mod project: {}", p.name), size: size(p.path.clone()).await, detail: "Source code and build output".into(), removable: false, path: Some(p.path) });
    }
    let exports = l.paths.root.join("exports");
    if exports.exists() {
        out.push(Item { key: "exports".into(), label: "Exported modpacks".into(), size: size(exports.clone()).await, detail: "Files you exported. Remove them yourself if you don't need them".into(), removable: false, path: Some(exports) });
    }
    let skins = l.paths.root.join("skins");
    if skins.exists() {
        out.push(Item { key: "skins".into(), label: "Saved skins".into(), size: size(skins).await, detail: "Your skin library".into(), removable: false, path: None });
    }
    let leftovers = cache_leftovers(l);
    let cache_all = size(l.paths.cache()).await;
    let leftover_size = {
        let ps = leftovers.clone();
        tokio::task::spawn_blocking(move || ps.iter().map(|p| size_of(p)).sum::<u64>()).await.unwrap_or(0)
    };
    out.push(Item { key: "cache".into(), label: "Skins, capes and textures cache".into(), size: cache_all.saturating_sub(leftover_size), detail: "Small, and used by the launcher".into(), removable: false, path: None });
    if leftover_size > 0 {
        out.push(Item { key: "downloads".into(), label: "Old downloads".into(), size: leftover_size, detail: "Mod loader installers and other downloads that are already installed".into(), removable: true, path: None });
    }
    let left = unfinished(l);
    if !left.is_empty() {
        let ps = left.clone();
        let s = tokio::task::spawn_blocking(move || ps.iter().map(|p| size_of(p)).sum::<u64>()).await.unwrap_or(0);
        out.push(Item { key: "unfinished".into(), label: "Unfinished deletes".into(), size: s, detail: "Folders an earlier delete couldn't finish".into(), removable: true, path: None });
    }
    out
}

pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

// ---------- removing ----------

/// Rename a folder out of the way, then remove it. Renaming fails as a whole if
/// anything inside is open, so a runtime is never left half deleted.
async fn remove_dir(dir: &Path) -> Result<()> {
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let target = if name.ends_with(DELETING) {
        dir.to_path_buf()
    } else {
        let mut t = dir.with_file_name(format!("{name}{DELETING}"));
        let mut n = 2;
        while t.exists() {
            t = dir.with_file_name(format!("{name}-{n}{DELETING}"));
            n += 1;
        }
        tokio::fs::rename(dir, &t).await?;
        t
    };
    // std's remove_dir_all removes links and junctions without following them.
    tokio::fs::remove_dir_all(&target).await?;
    Ok(())
}

async fn remove_paths(paths: Vec<PathBuf>) -> u64 {
    let mut freed = 0;
    for p in paths {
        let Ok(m) = tokio::fs::symlink_metadata(&p).await else { continue };
        let n = if m.is_dir() { size(p.clone()).await } else { m.len() };
        let ok = if m.is_dir() { remove_dir(&p).await.is_ok() } else { tokio::fs::remove_file(&p).await.is_ok() };
        if ok {
            freed += n;
        }
    }
    freed
}

/// Ask Gradle daemons left from earlier builds to stop, so their files can be removed.
async fn stop_gradle_daemons(l: &Launcher) {
    let home = l.paths.gradle().join("home");
    if !home.join("daemon").exists() {
        return;
    }
    let Some(jdk) = entries(&l.paths.java()).into_iter().map(|(_, p, _)| p).find(|p| java::java_bin(p).exists() && !p.to_string_lossy().ends_with(DELETING)) else {
        return;
    };
    for (name, dist, _) in entries(&l.paths.gradle()) {
        let bin = crate::gradle::gradle_bin(&dist);
        if !name.starts_with("gradle-") || !bin.exists() {
            continue;
        }
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("--stop").env("GRADLE_USER_HOME", &home).env("JAVA_HOME", &jdk).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        crate::process::hide_window(&mut cmd);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(60), cmd.status()).await;
    }
}

/// Remove one removable item. Returns the bytes freed.
pub async fn clean(l: &Launcher, part_name: &str, key: &str) -> Result<u64> {
    if !l.processes.running().await.is_empty() {
        bail!("Close the game and stop any test clients first.");
    }
    if l.gradle_runs.load(Ordering::SeqCst) > 0 {
        bail!("Wait for the mod build to finish first.");
    }
    let Ok(_guard) = l.install_lock.try_lock() else {
        bail!("Wait for the install to finish first.");
    };
    // Look again now, rather than trusting what the page showed.
    let fresh = part(l, part_name).await?;
    let Some(item) = fresh.items.iter().find(|i| i.key == key && i.removable) else {
        bail!("That can't be removed any more. The page has been refreshed.");
    };
    match (part_name, key) {
        ("versions", _) => {
            remove_dir(&l.paths.version_dir(&item.key)).await.map_err(|_| in_use(&item.label))?;
            Ok(item.size)
        }
        ("java", _) | ("gradle", _) => {
            let dir = if part_name == "java" { l.paths.java() } else { l.paths.gradle() }.join(&item.key);
            if remove_dir(&dir).await.is_err() {
                stop_gradle_daemons(l).await;
                remove_dir(&dir).await.map_err(|_| in_use(&item.label))?;
            }
            Ok(item.size)
        }
        ("assets", "unused") => {
            let l2 = l.clone();
            let plan = tokio::task::spawn_blocking(move || asset_plan(&l2)).await?;
            if !plan.sure {
                bail!(UNSURE);
            }
            Ok(remove_paths(plan.unused).await)
        }
        ("other", "downloads") => Ok(remove_paths(cache_leftovers(l)).await),
        ("other", "unfinished") => Ok(remove_paths(unfinished(l)).await),
        _ => bail!("That can't be removed from here."),
    }
}

fn in_use(label: &str) -> anyhow::Error {
    anyhow::anyhow!("Something is still using {label}, so it wasn't removed. Close other programs that might use it (or restart your computer) and try again.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Paths;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn inst_json(id: &str, mc: &str, vid: Option<&str>) -> String {
        serde_json::json!({ "id": id, "name": format!("Inst {id}"), "mc_version": mc, "loader": "fabric", "loader_version": null, "version_id": vid, "ram_mb": null, "created_at": 0 }).to_string()
    }

    async fn setup() -> (tempfile::TempDir, Launcher) {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let p = &l.paths;
        write(&p.instance_dir("a").join("instance.json"), &inst_json("a", "1.21.1", Some("fabric-loader-0.20.1-1.21.1")));
        write(&p.version_json("fabric-loader-0.20.1-1.21.1"), r#"{"id":"fabric-loader-0.20.1-1.21.1","inheritsFrom":"1.21.1"}"#);
        write(&p.version_json("1.21.1"), r#"{"id":"1.21.1","assetIndex":{"id":"17"},"javaVersion":{"majorVersion":21}}"#);
        write(&p.version_json("1.12.2"), r#"{"id":"1.12.2","assetIndex":{"id":"1.12"}}"#);
        write(&p.version_jar("1.12.2"), &"j".repeat(1000));
        write(&p.assets().join("indexes/17.json"), r#"{"objects":{"a":{"hash":"aa11","size":4}}}"#);
        write(&p.assets().join("indexes/1.12.json"), r#"{"objects":{"b":{"hash":"bb22","size":4}}}"#);
        write(&p.assets().join("objects/aa/aa11"), "keep");
        write(&p.assets().join("objects/bb/bb22"), "gone");
        write(&p.java().join("jdk-8/bin/java.exe"), "x");
        write(&p.cache().join("forge-1.12.2-14.23.5.2859-installer.jar"), &"i".repeat(500));
        write(&p.cache().join("version_manifest_v2.json"), "{}");
        (dir, l)
    }

    #[tokio::test]
    async fn finds_and_removes_what_nothing_uses() {
        let (_dir, l) = setup().await;
        let v = part(&l, "versions").await.unwrap();
        let get = |k: &str| v.items.iter().find(|i| i.key == k).unwrap().clone();
        assert!(!get("1.21.1").removable && get("1.21.1").detail.contains("Inst a"));
        assert!(!get("fabric-loader-0.20.1-1.21.1").removable);
        assert!(get("1.12.2").removable);

        assert!(clean(&l, "versions", "1.21.1").await.is_err());
        assert!(clean(&l, "versions", "../instances").await.is_err());
        assert!(clean(&l, "versions", "1.12.2").await.unwrap() >= 1000);
        assert!(!l.paths.version_dir("1.12.2").exists());

        let a = part(&l, "assets").await.unwrap();
        assert!(a.items.iter().any(|i| i.key == "unused" && i.removable));
        clean(&l, "assets", "unused").await.unwrap();
        assert!(l.paths.assets().join("objects/aa/aa11").exists());
        assert!(!l.paths.assets().join("objects/bb/bb22").exists());
        assert!(!l.paths.assets().join("indexes/1.12.json").exists());

        assert_eq!(clean(&l, "other", "downloads").await.unwrap(), 500);
        assert!(l.paths.cache().join("version_manifest_v2.json").exists());

        // Java 8 isn't needed by anything here (the version JSONs name no Java), but
        // Java 21 is the default for 1.21.1 and isn't installed.
        assert!(clean(&l, "java", "jdk-8").await.is_ok());
        assert!(!l.paths.java().join("jdk-8").exists());
    }

    #[tokio::test]
    async fn removes_nothing_when_unsure() {
        let (_dir, l) = setup().await;
        write(&l.paths.instance_dir("broken").join("instance.json"), "{ not json");
        let v = part(&l, "versions").await.unwrap();
        assert!(v.note.is_some());
        assert!(v.items.iter().all(|i| !i.removable));
        assert!(clean(&l, "versions", "1.12.2").await.is_err());
        assert!(clean(&l, "assets", "unused").await.is_err());
        assert!(l.paths.version_dir("1.12.2").exists());
    }

    #[tokio::test]
    async fn refuses_while_gradle_runs() {
        let (_dir, l) = setup().await;
        let run = GradleRun::start(&l);
        assert!(clean(&l, "other", "downloads").await.is_err());
        drop(run);
    }

    /// Read-only: how long each part takes on a real data folder (`MIO_STORAGE_ROOT`).
    #[tokio::test]
    #[ignore]
    async fn scans_a_real_data_folder() {
        let root = std::env::var("MIO_STORAGE_ROOT").expect("set MIO_STORAGE_ROOT");
        let l = Launcher::new(Paths::new(root)).await.unwrap();
        for name in PARTS {
            let t = std::time::Instant::now();
            let p = part(&l, name).await.unwrap();
            let rm: Vec<_> = p.items.iter().filter(|i| i.removable).collect();
            println!(
                "{name:10} {:>10} in {:>4} ms, {} items, {} removable ({}), unsure: {}",
                human(p.total),
                t.elapsed().as_millis(),
                p.items.len(),
                rm.len(),
                human(rm.iter().map(|i| i.size).sum()),
                p.note.is_some()
            );
        }
    }

    /// A folder with a file open inside (like a jar a running Java has open) isn't touched at all.
    #[cfg(windows)]
    #[tokio::test]
    async fn leaves_folders_in_use_alone() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_dir, l) = setup().await;
        let jar = l.paths.version_jar("1.12.2");
        // Read sharing only, the way Java opens jars.
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(&jar).unwrap();
        assert!(clean(&l, "versions", "1.12.2").await.is_err());
        assert!(jar.exists(), "the folder was renamed or emptied while in use");
        assert!(l.paths.version_json("1.12.2").exists(), "part of the folder was deleted");
        drop(held);
        clean(&l, "versions", "1.12.2").await.unwrap();
        assert!(!l.paths.version_dir("1.12.2").exists());
    }

    /// A junction inside a folder is neither counted nor followed when the folder is removed.
    #[cfg(windows)]
    #[tokio::test]
    async fn never_follows_junctions() {
        let (_dir, l) = setup().await;
        let outside = tempfile::tempdir().unwrap();
        let canary = outside.path().join("canary.txt");
        std::fs::write(&canary, "x".repeat(5000)).unwrap();
        let link = l.paths.version_dir("1.12.2").join("link");
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(outside.path()).output().unwrap().status.success();
        assert!(ok && link.join("canary.txt").exists());
        let v = part(&l, "versions").await.unwrap();
        let item = v.items.iter().find(|i| i.key == "1.12.2").unwrap();
        assert!(item.size < 5000, "counted the junction's target: {}", item.size);
        clean(&l, "versions", "1.12.2").await.unwrap();
        assert!(!l.paths.version_dir("1.12.2").exists());
        assert!(canary.exists(), "followed the junction");
    }
}
