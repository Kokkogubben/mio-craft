//! Instances: a Minecraft version + loader with its own game folder.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::loaders::{self, Loader};
use crate::{http, java, mojang, Event, Launcher};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: Loader,
    /// Fabric loader or NeoForge version. Resolved to latest stable if empty at creation.
    pub loader_version: Option<String>,
    /// Version id to launch (e.g. `fabric-loader-0.16.9-1.21.1`). Set once installed.
    pub version_id: Option<String>,
    pub ram_mb: Option<u32>,
    pub created_at: u64,
    #[serde(default)]
    pub last_played: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct NewInstance {
    pub name: Option<String>,
    pub mc_version: String,
    pub loader: Loader,
    pub loader_version: Option<String>,
    pub ram_mb: Option<u32>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "instance".into()
    } else {
        out
    }
}

fn file(l: &Launcher, id: &str) -> std::path::PathBuf {
    l.paths.instance_dir(id).join("instance.json")
}

pub async fn list(l: &Launcher) -> Result<Vec<Instance>> {
    let mut out = vec![];
    let mut rd = tokio::fs::read_dir(l.paths.instances()).await?;
    while let Some(e) = rd.next_entry().await? {
        if let Ok(i) = http::read_json::<Instance>(&e.path().join("instance.json")).await {
            out.push(i);
        }
    }
    out.sort_by(|a, b| b.last_played.unwrap_or(b.created_at).cmp(&a.last_played.unwrap_or(a.created_at)));
    Ok(out)
}

pub async fn get(l: &Launcher, id: &str) -> Result<Instance> {
    crate::paths::safe_join(&l.paths.instances(), id)?;
    http::read_json(&file(l, id))
        .await
        .map_err(|_| anyhow!("no instance with id {id:?}"))
}

pub async fn save(l: &Launcher, i: &Instance) -> Result<()> {
    http::write_json(&file(l, &i.id), i).await
}

pub async fn create(l: &Launcher, req: NewInstance) -> Result<Instance> {
    let name = req
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("{} {}", req.mc_version, req.loader.as_str()));
    let base = slug(&name);
    let mut id = base.clone();
    let mut n = 2;
    while l.paths.instance_dir(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    let mut loader_version = req.loader_version.filter(|v| !v.is_empty());
    if loader_version.is_none() {
        loader_version = match req.loader {
            Loader::Vanilla => None,
            Loader::Fabric => {
                let v = loaders::fabric_versions(l, &req.mc_version).await?;
                let pick = v.iter().find(|x| x.stable).or(v.first());
                Some(pick.ok_or_else(|| anyhow!("Fabric has no loader for Minecraft {}", req.mc_version))?.version.clone())
            }
            Loader::Neoforge => {
                if loaders::neoforge_prefix(&req.mc_version).is_none() {
                    bail!("NeoForge supports Minecraft 1.20.2 and newer");
                }
                let v = loaders::neoforge_versions(l, &req.mc_version).await?;
                let pick = v.iter().find(|x| x.stable).or(v.first());
                Some(pick.ok_or_else(|| anyhow!("NeoForge has no build for Minecraft {} yet", req.mc_version))?.version.clone())
            }
        };
    }
    let inst = Instance {
        id,
        name,
        mc_version: req.mc_version,
        loader: req.loader,
        loader_version,
        version_id: None,
        ram_mb: req.ram_mb,
        created_at: now(),
        last_played: None,
    };
    tokio::fs::create_dir_all(l.paths.mods_dir(&inst.id)).await?;
    save(l, &inst).await?;
    l.emit(Event::InstancesChanged);
    Ok(inst)
}

/// Download everything the instance needs (version, loader, libraries, assets, Java).
pub async fn install(l: &Launcher, id: &str) -> Result<Instance> {
    let _guard = l.install_lock.lock().await;
    let mut inst = get(l, id).await?;
    l.progress("install", 0, 4, format!("Installing Minecraft {}", inst.mc_version));
    let vanilla = mojang::fetch_version_json(l, &inst.mc_version).await?;
    l.progress("install", 1, 4, "Installing mod loader");
    let version_id = match inst.loader {
        Loader::Vanilla => inst.mc_version.clone(),
        Loader::Fabric => {
            let v = inst.loader_version.clone().ok_or_else(|| anyhow!("no Fabric version set"))?;
            loaders::install_fabric(l, &inst.mc_version, &v).await?
        }
        Loader::Neoforge => {
            let v = inst.loader_version.clone().ok_or_else(|| anyhow!("no NeoForge version set"))?;
            loaders::install_neoforge(l, &inst.mc_version, &v).await?
        }
    };
    l.progress("install", 2, 4, "Downloading libraries and assets");
    mojang::install_files(l, &version_id).await?;
    l.progress("install", 3, 4, "Getting Java");
    java::ensure_jdk(l, java::required_major(&vanilla)).await?;
    inst.version_id = Some(version_id);
    save(l, &inst).await?;
    l.progress("install", 4, 4, format!("{} is ready", inst.name));
    l.emit(Event::InstancesChanged);
    Ok(inst)
}

pub async fn delete(l: &Launcher, id: &str) -> Result<()> {
    let inst = get(l, id).await?;
    if l.processes.is_running(&format!("game:{}", inst.id)).await {
        bail!("stop the game before deleting this instance");
    }
    tokio::fs::remove_dir_all(l.paths.instance_dir(&inst.id)).await?;
    l.emit(Event::InstancesChanged);
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct InstancePatch {
    pub name: Option<String>,
    pub ram_mb: Option<u32>,
}

pub async fn update(l: &Launcher, id: &str, p: InstancePatch) -> Result<Instance> {
    let mut inst = get(l, id).await?;
    if let Some(n) = p.name.filter(|n| !n.trim().is_empty()) {
        inst.name = n;
    }
    if let Some(r) = p.ram_mb {
        inst.ram_mb = Some(r.clamp(512, 65536));
    }
    save(l, &inst).await?;
    l.emit(Event::InstancesChanged);
    Ok(inst)
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugs() {
        assert_eq!(super::slug("My Cool  Pack!"), "my-cool-pack");
        assert_eq!(super::slug("1.21.1 fabric"), "1-21-1-fabric");
        assert_eq!(super::slug("!!!"), "instance");
    }
}
