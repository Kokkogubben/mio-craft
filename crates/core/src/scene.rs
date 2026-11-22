//! Block textures for the home screen's scene, read from the player's own
//! Minecraft client jar.
//!
//! Nothing from the game ships with the launcher. The jar comes from Mojang's
//! servers onto the player's computer, the same download as installing that
//! version (and reused when they do), and only once an account is signed in:
//! signing in checks that the account owns the game. Only the textures the
//! scene asks for are copied out, into the launcher's cache.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::http::{self, Download};
use crate::{auth, mojang, Launcher};

/// The most textures one request may ask for.
const MAX_NAMES: usize = 256;

#[derive(Debug, Serialize)]
pub struct SceneTextures {
    /// The Minecraft version the textures were read from.
    pub version: String,
    /// Texture name (`block/dirt`) → PNG, base64. Names the jar doesn't have are left out.
    pub textures: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheIndex {
    version: String,
    /// Names asked for before that the jar doesn't have, so they aren't looked for again.
    #[serde(default)]
    missing: Vec<String>,
}

/// `block/dirt`, `block/grass_block_side_overlay` and so on: block textures only.
fn valid_name(n: &str) -> bool {
    n.strip_prefix("block/")
        .is_some_and(|rest| !rest.is_empty() && rest.len() <= 64 && rest.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
}

fn cache_dir(l: &Launcher) -> PathBuf {
    l.paths.cache().join("scene-textures")
}

/// The textures the scene needs, from the cache or the client jar.
pub async fn textures(l: &Launcher, names: &[String]) -> Result<SceneTextures> {
    if names.len() > MAX_NAMES {
        bail!("Too many textures asked for ({}; at most {MAX_NAMES})", names.len());
    }
    if let Some(bad) = names.iter().find(|n| !valid_name(n)) {
        bail!("Not a block texture name: {bad}");
    }
    if auth::load_accounts(l).await.is_empty() {
        bail!("Sign in to load the scene's textures.");
    }

    let dir = cache_dir(l);
    let index_path = dir.join("index.json");
    let mut index: CacheIndex = http::read_json(&index_path).await.unwrap_or_default();
    let cached = |index: &CacheIndex, n: &String| index.missing.contains(n) || dir.join(format!("{n}.png")).is_file();

    if index.version.is_empty() || !names.iter().all(|n| cached(&index, n)) {
        let (version, jar) = client_jar(l).await?;
        if version != index.version {
            // Textures from another version: start over, so versions never mix.
            let _ = tokio::fs::remove_dir_all(&dir).await;
            index = CacheIndex { version: version.clone(), missing: vec![] };
        }
        let wanted: Vec<String> = names.iter().filter(|n| !cached(&index, n)).cloned().collect();
        let out = dir.clone();
        let missing = tokio::task::spawn_blocking(move || extract(&jar, &wanted, &out)).await??;
        index.missing.extend(missing);
        index.missing.sort();
        index.missing.dedup();
        http::write_json(&index_path, &index).await?;
    }

    let mut textures = BTreeMap::new();
    for n in names {
        if let Ok(png) = tokio::fs::read(dir.join(format!("{n}.png"))).await {
            textures.insert(n.clone(), base64(&png));
        }
    }
    Ok(SceneTextures { version: index.version, textures })
}

/// Copy the named textures out of a client jar into `out`. Returns the names it doesn't have.
fn extract(jar: &Path, names: &[String], out: &Path) -> Result<Vec<String>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(jar)?).with_context(|| format!("opening {}", jar.display()))?;
    let mut missing = vec![];
    for n in names {
        let mut entry = match zip.by_name(&format!("assets/minecraft/textures/{n}.png")) {
            Ok(e) => e,
            Err(_) => {
                missing.push(n.clone());
                continue;
            }
        };
        let mut png = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut png)?;
        let path = out.join(format!("{n}.png"));
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, png)?;
    }
    Ok(missing)
}

/// A vanilla client jar to read from: the newest installed release, or else
/// the latest release, downloaded the same way installing it would.
async fn client_jar(l: &Launcher) -> Result<(String, PathBuf)> {
    if let Some(found) = newest_installed(l).await {
        return Ok(found);
    }
    let m = mojang::manifest(l).await?;
    let id = m.latest.release;
    let v = mojang::fetch_version_json(l, &id).await?;
    let client = v
        .downloads
        .as_ref()
        .and_then(|d| d.get("client"))
        .ok_or_else(|| anyhow!("Minecraft {id} has no client download"))?;
    let path = l.paths.version_jar(&id);
    http::download(&l.http, &Download { url: client.url.clone(), path: path.clone(), sha1: client.sha1.clone(), size: client.size }).await?;
    Ok((id, path))
}

#[derive(Deserialize)]
struct VersionHead {
    id: String,
    #[serde(rename = "inheritsFrom")]
    inherits_from: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "releaseTime")]
    release_time: Option<String>,
}

/// The newest vanilla release whose client jar is on disk.
async fn newest_installed(l: &Launcher) -> Option<(String, PathBuf)> {
    let mut rd = tokio::fs::read_dir(l.paths.versions()).await.ok()?;
    let mut best: Option<(String, String, PathBuf)> = None; // (release time, id, jar)
    while let Ok(Some(e)) = rd.next_entry().await {
        let id = e.file_name().to_string_lossy().to_string();
        let jar = l.paths.version_jar(&id);
        if !jar.is_file() {
            continue;
        }
        let Ok(head) = http::read_json::<VersionHead>(&l.paths.version_json(&id)).await else { continue };
        if head.inherits_from.is_some() || head.kind.as_deref() != Some("release") || head.id != id {
            continue;
        }
        let time = head.release_time.unwrap_or_default();
        if best.as_ref().is_none_or(|(t, _, _)| time > *t) {
            best = Some((time, id, jar));
        }
    }
    best.map(|(_, id, jar)| (id, jar))
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                s.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_block_textures_only() {
        assert!(valid_name("block/dirt"));
        assert!(valid_name("block/grass_block_side_overlay"));
        assert!(!valid_name("block/"));
        assert!(!valid_name("item/apple"));
        assert!(!valid_name("block/../../secret"));
        assert!(!valid_name("block/Dirt"));
        assert!(!valid_name("block/a/b"));
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn extracts_only_what_the_jar_has() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("client.jar");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
            w.start_file("assets/minecraft/textures/block/dirt.png", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"png").unwrap();
            w.finish().unwrap();
        }
        let out = tmp.path().join("out");
        let missing = extract(&jar, &["block/dirt".into(), "block/nope".into()], &out).unwrap();
        assert_eq!(missing, vec!["block/nope".to_string()]);
        assert_eq!(std::fs::read(out.join("block/dirt.png")).unwrap(), b"png");
    }
}
