//! Mod sets: named selections of which mods are on in an instance ("Full", "Light").
//!
//! A set remembers mods by what they are, not by file name, since updates rename
//! jars: the Modrinth or CurseForge project, the Claude project, or the mod id,
//! and only as a last resort the file name.
//!
//! The mods folder itself is always the active set. Switching first saves the
//! folder into the active set, then turns mods on and off to match the other one.
//! So a mod added while in one set belongs to that set only, and is off elsewhere.

use std::collections::{BTreeSet, HashSet};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::mods::ModEntry;
use crate::{http, instances, Event, Launcher};

const MAX_SETS: usize = 20;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModSets {
    /// The set the mods folder is in now, if any.
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub sets: Vec<ModSet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModSet {
    pub name: String,
    /// Keys of the mods that are on in this set.
    #[serde(default)]
    pub on: BTreeSet<String>,
}

/// What the UI gets: names, and how many mods each set turns on.
#[derive(Debug, Serialize)]
pub struct SetsView {
    pub active: Option<String>,
    pub sets: Vec<SetSummary>,
}

#[derive(Debug, Serialize)]
pub struct SetSummary {
    pub name: String,
    pub enabled: usize,
}

fn file(l: &Launcher, id: &str) -> std::path::PathBuf {
    l.paths.instance_dir(id).join("mod-sets.json")
}

async fn load(l: &Launcher, id: &str) -> ModSets {
    http::read_json(&file(l, id)).await.unwrap_or_default()
}

async fn save(l: &Launcher, id: &str, s: &ModSets) -> Result<()> {
    http::write_json(&file(l, id), s).await
}

/// What a set remembers a mod by.
pub fn key(m: &ModEntry) -> String {
    if let Some(o) = &m.online {
        let src = serde_json::to_value(o.source).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        return format!("{src}:{}", o.project_id);
    }
    if let Some(p) = &m.claude_project {
        return format!("claude:{p}");
    }
    if let Some(id) = m.mod_id.as_deref().filter(|s| !s.is_empty()) {
        return format!("mod:{id}");
    }
    format!("file:{}", m.file_name.strip_suffix(".disabled").unwrap_or(&m.file_name))
}

fn enabled_keys(mods: &[ModEntry]) -> BTreeSet<String> {
    mods.iter().filter(|m| m.enabled).map(key).collect()
}

fn view(s: &ModSets, mods: &[ModEntry]) -> SetsView {
    let present: HashSet<String> = mods.iter().map(key).collect();
    SetsView {
        active: s.active.clone(),
        sets: s
            .sets
            .iter()
            .map(|set| SetSummary {
                name: set.name.clone(),
                enabled: if s.active.as_deref() == Some(&set.name) {
                    mods.iter().filter(|m| m.enabled).count()
                } else {
                    mods.iter().filter(|m| set.on.contains(&key(m)) && present.contains(&key(m))).count()
                },
            })
            .collect(),
    }
}

fn clean_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 40 || n.chars().any(char::is_control) {
        bail!("Give the set a name of up to 40 characters");
    }
    Ok(n.to_string())
}

fn find(s: &ModSets, name: &str) -> Option<usize> {
    s.sets.iter().position(|x| x.name.eq_ignore_ascii_case(name))
}

/// Mods may only change while the game is closed.
async fn check_closed(l: &Launcher, inst: &instances::Instance) -> Result<()> {
    if l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        bail!("Close {} before switching mod sets", inst.name);
    }
    Ok(())
}

pub async fn list(l: &Launcher, id: &str) -> Result<SetsView> {
    instances::get(l, id).await?;
    let mods = crate::mods::list(l, id).await?;
    Ok(view(&load(l, id).await, &mods))
}

/// Save the mods that are on now as a new set, and make it the active one.
pub async fn create(l: &Launcher, id: &str, name: &str) -> Result<SetsView> {
    instances::get(l, id).await?;
    let name = clean_name(name)?;
    let mut s = load(l, id).await;
    if find(&s, &name).is_some() {
        bail!("There's already a set called {name}");
    }
    if s.sets.len() >= MAX_SETS {
        bail!("An instance can have at most {MAX_SETS} mod sets");
    }
    let mods = crate::mods::list(l, id).await?;
    let on = enabled_keys(&mods);
    // The folder is the active set; keep that set's saved copy current too.
    if let Some(i) = s.active.as_deref().and_then(|a| find(&s, a)) {
        s.sets[i].on = on.clone();
    }
    s.sets.push(ModSet { name: name.clone(), on });
    s.active = Some(name);
    save(l, id, &s).await?;
    Ok(view(&s, &mods))
}

/// Turn mods on and off to match a set.
pub async fn switch(l: &Launcher, id: &str, name: &str) -> Result<SetsView> {
    let inst = instances::get(l, id).await?;
    check_closed(l, &inst).await?;
    let Ok(_guard) = l.install_lock.try_lock() else { bail!("Wait for the install to finish, then switch") };
    let mut s = load(l, id).await;
    let Some(target) = find(&s, name) else { bail!("There's no mod set called {name}") };
    let mods = crate::mods::list(l, id).await?;
    if let Some(i) = s.active.as_deref().and_then(|a| find(&s, a)) {
        s.sets[i].on = enabled_keys(&mods);
    }
    let want = s.sets[target].on.clone();
    let dir = l.paths.mods_dir(id);
    let mut failed = vec![];
    for m in &mods {
        let on = want.contains(&key(m));
        if on == m.enabled {
            continue;
        }
        let base = m.file_name.strip_suffix(".disabled").unwrap_or(&m.file_name);
        let to = if on { base.to_string() } else { format!("{base}.disabled") };
        // Never replace a file: both x.jar and x.jar.disabled can exist.
        let to_path = dir.join(&to);
        if to_path.exists() {
            failed.push(m.file_name.clone());
            continue;
        }
        if tokio::fs::rename(dir.join(&m.file_name), &to_path).await.is_err() {
            failed.push(m.file_name.clone());
        }
    }
    s.active = Some(s.sets[target].name.clone());
    save(l, id, &s).await?;
    l.emit(Event::ModsChanged { instance_id: id.into() });
    if !failed.is_empty() {
        bail!("Switched, but these mods couldn't be changed: {}", failed.join(", "));
    }
    let mods = crate::mods::list(l, id).await?;
    Ok(view(&s, &mods))
}

pub async fn rename(l: &Launcher, id: &str, name: &str, new_name: &str) -> Result<SetsView> {
    instances::get(l, id).await?;
    let new_name = clean_name(new_name)?;
    let mut s = load(l, id).await;
    let Some(i) = find(&s, name) else { bail!("There's no mod set called {name}") };
    if find(&s, &new_name).is_some_and(|j| j != i) {
        bail!("There's already a set called {new_name}");
    }
    if s.active.as_deref() == Some(&s.sets[i].name) {
        s.active = Some(new_name.clone());
    }
    s.sets[i].name = new_name;
    save(l, id, &s).await?;
    Ok(view(&s, &crate::mods::list(l, id).await?))
}

/// Forget a set. The mods stay as they are.
pub async fn remove(l: &Launcher, id: &str, name: &str) -> Result<SetsView> {
    instances::get(l, id).await?;
    let mut s = load(l, id).await;
    let Some(i) = find(&s, name) else { bail!("There's no mod set called {name}") };
    let gone = s.sets.remove(i);
    if s.active.as_deref() == Some(&gone.name) {
        s.active = None;
    }
    save(l, id, &s).await?;
    Ok(view(&s, &crate::mods::list(l, id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::{OnlineMod, Source};
    use std::io::Write;

    fn jar(dir: &std::path::Path, name: &str, id: &str) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join(name)).unwrap());
        z.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(format!(r#"{{"schemaVersion":1,"id":"{id}","name":"{id}","version":"1"}}"#).as_bytes()).unwrap();
        z.finish().unwrap();
    }

    fn on(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|f| f.ends_with(".jar")).collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn switches_between_sets() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(dir.path())).await.unwrap();
        let inst = instances::create(&l, instances::NewInstance { name: Some("Sets".into()), mc_version: "1.21.1".into(), loader: crate::loaders::Loader::Fabric, loader_version: Some("0.16.9".into()), ram_mb: None })
            .await
            .unwrap();
        let md = l.paths.mods_dir(&inst.id);
        jar(&md, "sodium-0.6.jar", "sodium");
        jar(&md, "shaders.jar", "iris");
        jar(&md, "big.jar", "bigmod");
        let mut tags = crate::browse::OnlineTags::new();
        tags.insert("sodium-0.6.jar".into(), OnlineMod {
            source: Source::Modrinth, project_id: "AANobbMI".into(), version_id: "v".into(), title: "Sodium".into(),
            page_url: String::new(), as_dependency: false, download_url: None,
        });
        crate::browse::save_tags(&l, &inst.id, &tags).await.unwrap();

        let v = create(&l, &inst.id, " Full ").await.unwrap();
        assert_eq!(v.active.as_deref(), Some("Full"));
        assert_eq!(v.sets[0].enabled, 3);
        assert!(create(&l, &inst.id, "full").await.is_err());

        // "Light" starts as a copy; turning a mod off while in it changes only Light.
        create(&l, &inst.id, "Light").await.unwrap();
        crate::mods::set_enabled(&l, &inst.id, "big.jar", false).await.unwrap();
        let v = list(&l, &inst.id).await.unwrap();
        assert_eq!(v.sets.iter().map(|s| s.enabled).collect::<Vec<_>>(), vec![3, 2]);

        // Full turns it back on.
        switch(&l, &inst.id, "Full").await.unwrap();
        assert_eq!(on(&md), vec!["big.jar", "shaders.jar", "sodium-0.6.jar"]);

        // An update renames the Sodium jar; a mod added while in Full stays in Full only.
        std::fs::rename(md.join("sodium-0.6.jar"), md.join("sodium-0.7.jar")).unwrap();
        let mut tags = crate::browse::load_tags(&l, &inst.id).await;
        let t = tags.remove("sodium-0.6.jar").unwrap();
        tags.insert("sodium-0.7.jar".into(), t);
        crate::browse::save_tags(&l, &inst.id, &tags).await.unwrap();
        jar(&md, "extra.jar", "extra");

        switch(&l, &inst.id, "Light").await.unwrap();
        assert_eq!(on(&md), vec!["shaders.jar", "sodium-0.7.jar"]);
        assert!(md.join("extra.jar.disabled").exists());
        switch(&l, &inst.id, "Full").await.unwrap();
        assert_eq!(on(&md), vec!["big.jar", "extra.jar", "shaders.jar", "sodium-0.7.jar"]);

        // Not while the game runs (or its command before launch).
        let _stop = l.processes.claim(&crate::launch::game_source(&inst.id)).await.unwrap();
        let e = switch(&l, &inst.id, "Light").await.unwrap_err();
        assert!(e.to_string().contains("Close"), "{e}");
        l.processes.release(&crate::launch::game_source(&inst.id)).await;

        let v = rename(&l, &inst.id, "Full", "Everything").await.unwrap();
        assert_eq!(v.active.as_deref(), Some("Everything"));
        let v = remove(&l, &inst.id, "Everything").await.unwrap();
        assert_eq!(v.active, None);
        assert_eq!(on(&md).len(), 4, "removing a set leaves the mods alone");
        assert!(switch(&l, &inst.id, "Nope").await.is_err());
    }

    #[test]
    fn keys_survive_renames() {
        let m = |file: &str, id: Option<&str>| ModEntry {
            file_name: file.into(), enabled: true, mod_id: id.map(String::from), name: None, version: None, description: None,
            authors: vec![], loader: "fabric".into(), size: 0, claude_project: None, online: None,
        };
        assert_eq!(key(&m("a-1.jar", Some("a"))), key(&m("a-2.jar.disabled", Some("a"))));
        assert_eq!(key(&m("plain.jar.disabled", None)), "file:plain.jar");
        let mut c = m("glow-3.jar", Some("glow"));
        c.claude_project = Some("glow".into());
        assert_eq!(key(&c), "claude:glow");
    }
}
