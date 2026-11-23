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
    /// Maximum memory for the game. None: the launcher-wide default from before memory was per instance.
    pub ram_mb: Option<u32>,
    /// Extra JVM arguments for this instance. None: the launcher-wide ones from before they were per instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jvm_args: Option<Vec<String>>,
    pub created_at: u64,
    #[serde(default)]
    pub last_played: Option<u64>,
    /// How the game last exited (set when it stops).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<LastExit>,
    /// A crash at or before this time has been dismissed in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crash_dismissed_at: Option<u64>,
    /// Live mode toggles; applied on the next launch.
    #[serde(default)]
    pub live: crate::live::LiveSettings,
    /// Icon tile colour (`#rrggbb`). None: the loader's colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    /// One of the UI's built-in pixel icons. None: the loader's first letter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_glyph: Option<String>,
    /// A picture as a small PNG data URL; shown instead of the tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastExit {
    /// None when the launcher stopped the game.
    pub code: Option<i32>,
    pub at: u64,
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

/// A free instance id made from a name.
fn free_id(l: &Launcher, name: &str) -> String {
    let base = slug(name);
    let mut id = base.clone();
    let mut n = 2;
    while l.paths.instance_dir(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
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
        .unwrap_or_else(|| format!("{} {}", req.mc_version, req.loader.display_name()));
    let id = free_id(l, &name);
    let mut loader_version = req.loader_version.filter(|v| !v.is_empty());
    if loader_version.is_none() {
        loader_version = match req.loader {
            Loader::Vanilla => None,
            loader => {
                let v = loaders::versions(l, loader, &req.mc_version).await?;
                let pick = loaders::default_version(&v)
                    .ok_or_else(|| anyhow!(loaders::unsupported_message(loader, &req.mc_version)))?;
                Some(pick.version.clone())
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
        jvm_args: None,
        created_at: now(),
        last_played: None,
        last_exit: None,
        crash_dismissed_at: None,
        live: Default::default(),
        icon_color: None,
        icon_glyph: None,
        icon_image: None,
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
    let version_id = loaders::install(l, inst.loader, &inst.mc_version, inst.loader_version.as_deref()).await?;
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

/// Copy an instance with everything in it (worlds, mods, settings) under a new name.
pub async fn duplicate(l: &Launcher, id: &str, name: Option<String>) -> Result<Instance> {
    let src = get(l, id).await?;
    if l.processes.is_running(&format!("game:{}", src.id)).await {
        bail!("stop the game before copying this instance");
    }
    let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        let taken = |n: &str| std::fs::read_dir(l.paths.instances()).into_iter().flatten().flatten().any(|e| {
            std::fs::read_to_string(e.path().join("instance.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Instance>(&t).ok())
                .is_some_and(|i| i.name == n)
        });
        let mut n = format!("{} (copy)", src.name);
        let mut k = 2;
        while taken(&n) {
            n = format!("{} (copy {k})", src.name);
            k += 1;
        }
        n
    });
    let new_id = free_id(l, &name);
    let from = l.paths.instance_dir(&src.id);
    let to = l.paths.instance_dir(&new_id);
    let to2 = to.clone();
    let copied = tokio::task::spawn_blocking(move || copy_dir(&from, &to2)).await?;
    if let Err(e) = copied {
        let _ = tokio::fs::remove_dir_all(&to).await;
        return Err(e.context("copying the instance"));
    }
    let inst = Instance {
        id: new_id,
        name,
        created_at: now(),
        last_played: None,
        last_exit: None,
        crash_dismissed_at: None,
        ..src
    };
    save(l, &inst).await?;
    l.emit(Event::InstancesChanged);
    Ok(inst)
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let ty = e.file_type()?;
        let dest = to.join(e.file_name());
        if ty.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else if ty.is_file() {
            std::fs::copy(e.path(), &dest)?;
        }
        // Links are skipped: a copy shouldn't point back into the original.
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct InstancePatch {
    pub name: Option<String>,
    pub ram_mb: Option<u32>,
    pub jvm_args: Option<Vec<String>>,
    pub live: Option<crate::live::LiveSettings>,
    /// For the icon fields an empty string clears the value.
    pub icon_color: Option<String>,
    pub icon_glyph: Option<String>,
    pub icon_image: Option<String>,
}

/// Largest icon picture accepted (the UI sends 64×64 PNGs, far smaller).
const ICON_IMAGE_MAX: usize = 200_000;

fn icon_value(v: String, ok: impl Fn(&str) -> bool, what: &str) -> Result<Option<String>> {
    let v = v.trim().to_string();
    if v.is_empty() {
        return Ok(None);
    }
    if !ok(&v) {
        bail!("that {what} can't be used as an instance icon");
    }
    Ok(Some(v))
}

pub async fn update(l: &Launcher, id: &str, p: InstancePatch) -> Result<Instance> {
    let mut inst = get(l, id).await?;
    if let Some(n) = p.name.filter(|n| !n.trim().is_empty()) {
        inst.name = n;
    }
    if let Some(r) = p.ram_mb {
        inst.ram_mb = Some(r.clamp(512, 65536));
    }
    if let Some(a) = p.jvm_args {
        inst.jvm_args = Some(a.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
    }
    if let Some(c) = p.icon_color {
        inst.icon_color = icon_value(c, |c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()), "colour")?;
    }
    if let Some(g) = p.icon_glyph {
        inst.icon_glyph = icon_value(g, |g| g.len() <= 24 && g.chars().all(|c| c.is_ascii_lowercase() || c == '-'), "icon")?;
    }
    if let Some(img) = p.icon_image {
        inst.icon_image = icon_value(
            img,
            |i| i.len() <= ICON_IMAGE_MAX && i.starts_with("data:image/png;base64,") && i[22..].chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)),
            "picture",
        )?;
    }
    if let Some(live) = p.live {
        let live = live.normalized();
        if live.any() {
            if let Some(why) = crate::live::unsupported_reason(&inst) {
                anyhow::bail!(why);
            }
        }
        inst.live = live;
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
