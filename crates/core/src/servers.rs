//! The servers in an instance's multiplayer list. They live in the game's own
//! `servers.dat`, so servers added here show up in the game and the other way
//! round. Tags the launcher doesn't know are kept as they are.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::nbt::{self, Tag};
use crate::{instances, Launcher};

#[derive(Debug, Clone, Serialize)]
pub struct Server {
    /// Position in `servers.dat` (hidden entries count too).
    pub index: usize,
    pub name: String,
    pub address: String,
    /// The server's icon as base64 PNG, saved by the game after it pinged it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerInput {
    pub name: String,
    pub address: String,
}

const MAX_NAME: usize = 64;
const MAX_ADDRESS: usize = 255;
const MAX_SERVERS: usize = 500;

fn file(l: &Launcher, id: &str) -> std::path::PathBuf {
    l.paths.game_dir(id).join("servers.dat")
}

/// Read `servers.dat`, or an empty list when there isn't one yet.
async fn load(l: &Launcher, id: &str) -> Result<Tag> {
    match tokio::fs::read(file(l, id)).await {
        Ok(b) => {
            let root = nbt::read(&b).map_err(|e| anyhow!("Couldn't read this instance's server list (servers.dat): {e}"))?;
            if !matches!(root, Tag::Compound(_)) {
                bail!("This instance's servers.dat isn't a server list");
            }
            Ok(root)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Tag::Compound(vec![("servers".into(), Tag::List(10, vec![]))])),
        Err(e) => Err(e.into()),
    }
}

fn entries(root: &Tag) -> &[Tag] {
    match root.get("servers") {
        Some(Tag::List(_, v)) => v,
        _ => &[],
    }
}

fn entries_mut(root: &mut Tag) -> &mut Vec<Tag> {
    if !matches!(root.get("servers"), Some(Tag::List(..))) {
        root.set("servers", Tag::List(10, vec![]));
    }
    match root.get_mut("servers") {
        Some(Tag::List(el, v)) => {
            *el = 10;
            v
        }
        _ => unreachable!(),
    }
}

fn to_server(index: usize, t: &Tag) -> Option<Server> {
    // The game hides servers it only joined through Quick Play.
    if t.get("hidden").and_then(Tag::as_i64) == Some(1) {
        return None;
    }
    let icon = t
        .get("icon")
        .and_then(Tag::as_str)
        .filter(|s| !s.is_empty() && s.len() < 200_000 && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c)))
        .map(String::from);
    Some(Server {
        index,
        name: t.get("name").and_then(Tag::as_str).unwrap_or("Minecraft Server").to_string(),
        address: t.get("ip").and_then(Tag::as_str).unwrap_or("").to_string(),
        icon,
    })
}

pub async fn list(l: &Launcher, id: &str) -> Result<Vec<Server>> {
    instances::get(l, id).await?;
    let root = load(l, id).await?;
    Ok(entries(&root).iter().enumerate().filter_map(|(i, t)| to_server(i, t)).collect())
}

/// Check a server address the way the game reads it: a host name or IP, and
/// an optional port.
pub fn check_address(a: &str) -> Result<()> {
    if a.is_empty() {
        bail!("Enter the server's address");
    }
    if a.len() > MAX_ADDRESS || a.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("That isn't a server address");
    }
    let (host, port) = split_address(a);
    if host.is_empty() || port == 0 {
        bail!("That isn't a server address");
    }
    Ok(())
}

/// Host and port, as `--server` and `--port` want them (25565 when no port is given).
pub fn split_address(a: &str) -> (String, u16) {
    let port = |p: &str| if p.is_empty() { Some(25565) } else { p.parse::<u16>().ok() };
    if let Some(rest) = a.strip_prefix('[') {
        // [IPv6]:port
        if let Some((h, p)) = rest.split_once(']') {
            let p = p.strip_prefix(':').unwrap_or(p);
            return (h.to_string(), port(p).unwrap_or(0));
        }
        return (String::new(), 0);
    }
    match a.matches(':').count() {
        1 => {
            let (h, p) = a.split_once(':').unwrap();
            (h.to_string(), port(p).unwrap_or(0))
        }
        // No port, or a bare IPv6 address.
        _ => (a.to_string(), 25565),
    }
}

fn clean(input: &ServerInput) -> Result<(String, String)> {
    let address = input.address.trim().to_string();
    check_address(&address)?;
    let name: String = input.name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    Ok((if name.is_empty() { "Minecraft Server".into() } else { name }, address))
}

async fn save(l: &Launcher, id: &str, root: &Tag) -> Result<()> {
    let inst = instances::get(l, id).await?;
    if l.processes.is_running(&crate::launch::game_source(id)).await {
        bail!("{} is running. Close the game first, since it keeps its own copy of the server list.", inst.name);
    }
    let bytes = nbt::write(root)?;
    let path = file(l, id);
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    // Keep the old list next to it, as the game does.
    if path.exists() {
        let _ = tokio::fs::copy(&path, path.with_file_name("servers.dat_old")).await;
    }
    let part = path.with_file_name("servers.dat.part");
    tokio::fs::write(&part, &bytes).await?;
    tokio::fs::rename(&part, &path).await?;
    Ok(())
}

pub async fn add(l: &Launcher, id: &str, input: &ServerInput) -> Result<Vec<Server>> {
    let (name, address) = clean(input)?;
    let mut root = load(l, id).await?;
    let list = entries_mut(&mut root);
    if list.len() >= MAX_SERVERS {
        bail!("The server list is full");
    }
    list.push(Tag::Compound(vec![("name".into(), Tag::String(name)), ("ip".into(), Tag::String(address))]));
    save(l, id, &root).await?;
    list_from(&root)
}

pub async fn update(l: &Launcher, id: &str, index: usize, input: &ServerInput) -> Result<Vec<Server>> {
    let (name, address) = clean(input)?;
    let mut root = load(l, id).await?;
    let entry = entries_mut(&mut root).get_mut(index).ok_or_else(|| anyhow!("That server isn't in the list any more"))?;
    let moved = entry.get("ip").and_then(Tag::as_str) != Some(address.as_str());
    entry.set("name", Tag::String(name));
    entry.set("ip", Tag::String(address));
    // The icon belongs to the old address.
    if moved {
        entry.remove("icon");
    }
    save(l, id, &root).await?;
    list_from(&root)
}

pub async fn remove(l: &Launcher, id: &str, index: usize) -> Result<Vec<Server>> {
    let mut root = load(l, id).await?;
    let list = entries_mut(&mut root);
    if index >= list.len() {
        bail!("That server isn't in the list any more");
    }
    list.remove(index);
    save(l, id, &root).await?;
    list_from(&root)
}

fn list_from(root: &Tag) -> Result<Vec<Server>> {
    Ok(entries(root).iter().enumerate().filter_map(|(i, t)| to_server(i, t)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::Loader;
    use crate::Paths;

    #[test]
    fn splits_addresses() {
        assert_eq!(split_address("play.example.net"), ("play.example.net".into(), 25565));
        assert_eq!(split_address("play.example.net:25570"), ("play.example.net".into(), 25570));
        assert_eq!(split_address("10.0.0.2:1"), ("10.0.0.2".into(), 1));
        assert_eq!(split_address("[::1]:25566"), ("::1".into(), 25566));
        assert_eq!(split_address("[::1]"), ("::1".into(), 25565));
        assert_eq!(split_address("fe80::1"), ("fe80::1".into(), 25565));
        assert!(check_address("host:99999").is_err());
        assert!(check_address("two words").is_err());
        assert!(check_address("").is_err());
        assert!(check_address(":25565").is_err());
        assert!(check_address("mc.example.org").is_ok());
    }

    #[tokio::test]
    async fn edits_keep_what_the_game_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let inst = instances::create(&l, instances::NewInstance { name: Some("s".into()), mc_version: "1.21.1".into(), loader: Loader::Vanilla, loader_version: None, ram_mb: None })
            .await
            .unwrap();
        assert!(list(&l, &inst.id).await.unwrap().is_empty());

        // What the game writes: an icon, a texture choice, and a hidden Quick Play entry.
        let game = Tag::Compound(vec![(
            "servers".into(),
            Tag::List(10, vec![
                Tag::Compound(vec![("ip".into(), Tag::String("a.example".into())), ("name".into(), Tag::String("A".into())), ("icon".into(), Tag::String("iVBORw0KGgo=".into())), ("acceptTextures".into(), Tag::Byte(1))]),
                Tag::Compound(vec![("ip".into(), Tag::String("q.example".into())), ("name".into(), Tag::String("Q".into())), ("hidden".into(), Tag::Byte(1))]),
            ]),
        )]);
        let path = l.paths.game_dir(&inst.id).join("servers.dat");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, nbt::write(&game).unwrap()).unwrap();

        let s = list(&l, &inst.id).await.unwrap();
        assert_eq!(s.len(), 1, "the hidden one isn't listed");
        assert_eq!(s[0].icon.as_deref(), Some("iVBORw0KGgo="));

        let s = add(&l, &inst.id, &ServerInput { name: "  ".into(), address: " b.example:25570 ".into() }).await.unwrap();
        assert_eq!((s[1].index, s[1].name.as_str(), s[1].address.as_str()), (2, "Minecraft Server", "b.example:25570"));

        // Renaming keeps the icon and the texture choice; a new address drops the icon.
        update(&l, &inst.id, 0, &ServerInput { name: "Alpha".into(), address: "a.example".into() }).await.unwrap();
        let root = nbt::read(&std::fs::read(&path).unwrap()).unwrap();
        let first = &entries(&root)[0];
        assert_eq!(first.get("acceptTextures"), Some(&Tag::Byte(1)));
        assert!(first.get("icon").is_some());
        update(&l, &inst.id, 0, &ServerInput { name: "Alpha".into(), address: "new.example".into() }).await.unwrap();
        let root = nbt::read(&std::fs::read(&path).unwrap()).unwrap();
        assert!(entries(&root)[0].get("icon").is_none());
        assert_eq!(entries(&root)[1].get("hidden"), Some(&Tag::Byte(1)), "hidden entries are kept");
        assert!(path.with_file_name("servers.dat_old").exists());

        let s = remove(&l, &inst.id, 0).await.unwrap();
        assert_eq!(s.iter().map(|x| x.address.as_str()).collect::<Vec<_>>(), vec!["b.example:25570"]);
        assert!(remove(&l, &inst.id, 9).await.is_err());
        assert!(add(&l, &inst.id, &ServerInput { name: "x".into(), address: "bad address".into() }).await.is_err());
    }
}
