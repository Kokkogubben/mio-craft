//! Minecraft Forge: the version list and installs.
//!
//! Forge installers come in three shapes. They are told apart by what is in
//! the installer jar, not by the Minecraft version:
//!
//! - **Legacy** (1.6.4 to early 1.12.2): `install_profile.json` carries the
//!   version JSON (`versionInfo`) and names the Forge jar packed in the
//!   installer. Nothing has to run.
//! - **Embedded** (late 1.12.2): a `version.json` plus the Forge jar under
//!   `maven/`. Nothing has to run either.
//! - **Processors** (1.13 and newer): the Minecraft jar is patched by tools
//!   that the installer runs, so the official installer does the install.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::http::{self, Download};
use crate::loaders::LoaderVersion;
use crate::{java, maven, mojang, Launcher};

pub const FORGE_MAVEN: &str = "https://maven.minecraftforge.net";
const METADATA: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const PROMOTIONS: &str = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";

/// The Forge part of a full Maven version, which is what instances store and
/// what modpacks name: `1.20.1-47.4.28` → `47.4.28`,
/// `1.7.10-10.13.4.1614-1.7.10` → `10.13.4.1614`. `None` if `full` isn't for `mc`.
pub fn short_version(full: &str, mc: &str) -> Option<String> {
    let rest = full.strip_prefix(mc)?.strip_prefix('-')?;
    let v = rest.split('-').next()?;
    (!v.is_empty()).then(|| v.to_string())
}

/// The full Maven version for a Forge build. Old builds carry suffixes that
/// can't be guessed (`-1.7.10`, `-mc172`), so it is looked up.
pub fn full_version(all: &[String], mc: &str, short: &str) -> Option<String> {
    all.iter().find(|f| short_version(f, mc).as_deref() == Some(short)).cloned()
}

/// Forge builds for `mc`, newest first, with the one Forge recommends marked.
pub fn filter_versions(all: &[String], mc: &str, promos: &HashMap<String, String>) -> Vec<LoaderVersion> {
    let recommended = promos.get(&format!("{mc}-recommended"));
    let mut seen = HashSet::new();
    let mut out: Vec<LoaderVersion> = all
        .iter()
        .filter_map(|f| short_version(f, mc))
        .filter(|v| seen.insert(v.clone()))
        .map(|v| LoaderVersion { recommended: Some(&v) == recommended, stable: true, version: v })
        .collect();
    out.sort_by(|a, b| maven::cmp_versions(&b.version, &a.version));
    out
}

async fn all_versions(l: &Launcher) -> Result<Vec<String>> {
    let xml = http::get_text(&l.http, METADATA).await.context("getting Forge's version list")?;
    Ok(maven::metadata_versions(&xml))
}

pub async fn versions(l: &Launcher, mc: &str) -> Result<Vec<LoaderVersion>> {
    let all = all_versions(l).await?;
    // Only used to mark the recommended build, so a failure isn't fatal.
    let promos = http::get_json::<Value>(&l.http, PROMOTIONS)
        .await
        .ok()
        .and_then(|p| serde_json::from_value::<HashMap<String, String>>(p["promos"].clone()).ok())
        .unwrap_or_default();
    Ok(filter_versions(&all, mc, &promos))
}

/// What an installer jar needs done.
#[derive(Debug)]
enum Shape {
    Legacy { version: Value, universal: String, coord: String },
    Embedded { version: Value },
    Processors { id: String },
}

fn read_entry(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<Vec<u8>> {
    let mut f = zip.by_name(name.trim_start_matches('/')).with_context(|| format!("the installer has no {name}"))?;
    let mut buf = vec![];
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_shape(installer: &Path) -> Result<Shape> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(installer)?).context("opening the Forge installer")?;
    let profile: Value = serde_json::from_slice(&read_entry(&mut zip, "install_profile.json")?)
        .context("reading the Forge installer's install_profile.json")?;
    shape_of(profile, |name| read_entry(&mut zip, name))
}

fn shape_of(profile: Value, mut read: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<Shape> {
    if profile["versionInfo"].is_object() {
        let install = &profile["install"];
        let universal = install["filePath"].as_str().ok_or_else(|| anyhow!("the Forge installer doesn't name its jar"))?;
        let coord = install["path"].as_str().ok_or_else(|| anyhow!("the Forge installer doesn't name its library"))?;
        return Ok(Shape::Legacy { version: profile["versionInfo"].clone(), universal: universal.into(), coord: coord.into() });
    }
    let json_path = profile["json"].as_str().unwrap_or("/version.json");
    let version: Value = serde_json::from_slice(&read(json_path)?).context("reading the Forge version JSON")?;
    let id = version["id"].as_str().ok_or_else(|| anyhow!("the Forge version JSON has no id"))?.to_string();
    let client_processors = profile["processors"].as_array().map_or(0, |p| {
        p.iter()
            .filter(|p| p["sides"].as_array().map_or(true, |s| s.iter().any(|s| s == "client")))
            .count()
    });
    Ok(if client_processors == 0 { Shape::Embedded { version } } else { Shape::Processors { id } })
}

/// The legacy version JSON, with the Forge library pointed at the jar taken
/// out of the installer (the Maven only has it as `-universal.jar`).
fn legacy_version_json(mut v: Value, coord: &str, rel: &str, sha1: &str, size: u64) -> Value {
    if let Some(libs) = v["libraries"].as_array_mut() {
        for lib in libs {
            if lib["name"] == coord {
                *lib = json!({
                    "name": coord,
                    "downloads": { "artifact": { "path": rel, "url": "", "sha1": sha1, "size": size } },
                });
            } else if let Some(url) = lib["url"].as_str() {
                // Forge's old Maven address; the files moved.
                if url.contains("files.minecraftforge.net/maven") || url.starts_with("http://maven.minecraftforge.net") {
                    lib["url"] = json!(format!("{FORGE_MAVEN}/"));
                }
            }
        }
    }
    v
}

fn extract_entry(installer: &Path, name: &str, to: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(installer)?)?;
    let bytes = read_entry(&mut zip, name)?;
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(to, bytes)?;
    Ok(())
}

fn id_of(v: &Value) -> Result<String> {
    Ok(v["id"].as_str().ok_or_else(|| anyhow!("the Forge version JSON has no id"))?.to_string())
}

/// Install Forge `short` (e.g. `47.4.28`) for Minecraft `mc` and return the
/// version id to launch.
pub async fn install(l: &Launcher, mc: &str, short: &str) -> Result<String> {
    let all = all_versions(l).await?;
    let full = full_version(&all, mc, short).ok_or_else(|| anyhow!("Forge {short} doesn't exist for Minecraft {mc}"))?;

    // The vanilla files come first: the installer patches the client jar.
    let vanilla = mojang::fetch_version_json(l, mc).await?;
    mojang::install_files(l, mc).await?;

    let url = format!("{FORGE_MAVEN}/net/minecraftforge/forge/{full}/forge-{full}-installer.jar");
    let installer: PathBuf = l.paths.cache().join(format!("forge-{full}-installer.jar"));
    let sha1 = http::get_text(&l.http, &format!("{url}.sha1")).await.ok().map(|s| s.trim().to_string()).filter(|s| s.len() == 40);
    l.progress("forge", 0, 1, format!("Downloading Forge {short}"));
    http::download(&l.http, &Download { url, path: installer.clone(), sha1, size: None })
        .await
        .with_context(|| format!("Forge {short} for Minecraft {mc} has no installer that MioCraft can use"))?;

    let shape = {
        let p = installer.clone();
        tokio::task::spawn_blocking(move || read_shape(&p)).await??
    };
    let libs = l.paths.libraries();
    match shape {
        Shape::Legacy { version, universal, coord } => {
            let id = id_of(&version)?;
            let rel = maven::coord_to_path(&coord)?;
            let jar = libs.join(&rel);
            let (src, dst) = (installer.clone(), jar.clone());
            tokio::task::spawn_blocking(move || extract_entry(&src, &universal, &dst)).await??;
            let sha1 = {
                let p = jar.clone();
                tokio::task::spawn_blocking(move || http::sha1_file_sync(&p)).await??
            };
            let size = tokio::fs::metadata(&jar).await?.len();
            http::write_json(&l.paths.version_json(&id), &legacy_version_json(version, &coord, &rel, &sha1, size)).await?;
            l.progress("forge", 1, 1, format!("Forge {short} installed"));
            Ok(id)
        }
        Shape::Embedded { version } => {
            let id = id_of(&version)?;
            let (src, dst) = (installer.clone(), libs.clone());
            tokio::task::spawn_blocking(move || http::extract_zip(&src, &dst, |n| n.starts_with("maven/"), true)).await??;
            http::write_json(&l.paths.version_json(&id), &version).await?;
            l.progress("forge", 1, 1, format!("Forge {short} installed"));
            Ok(id)
        }
        Shape::Processors { id } => {
            if l.paths.version_json(&id).exists() {
                // The installer writes the version JSON last, after patching.
                return Ok(id);
            }
            let jdk = java::ensure_jdk(l, java::required_major(&vanilla)).await?;
            crate::loaders::run_installer(l, &format!("Forge {short}"), &installer, &jdk, &id, &["--installClient"]).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn short_and_full_versions() {
        assert_eq!(short_version("1.20.1-47.4.28", "1.20.1").as_deref(), Some("47.4.28"));
        assert_eq!(short_version("1.7.10-10.13.4.1614-1.7.10", "1.7.10").as_deref(), Some("10.13.4.1614"));
        assert_eq!(short_version("1.7.2-10.12.2.1161-mc172", "1.7.2").as_deref(), Some("10.12.2.1161"));
        assert_eq!(short_version("1.7.10-10.13.4.1614-1.7.10", "1.7.1"), None);
        assert_eq!(short_version("1.20.1-47.4.28", "1.20"), None);

        let all = strings(&["1.7.10-10.13.4.1614-1.7.10", "1.7.2-10.12.2.1161-mc172", "1.20.1-47.4.28"]);
        assert_eq!(full_version(&all, "1.7.10", "10.13.4.1614").as_deref(), Some("1.7.10-10.13.4.1614-1.7.10"));
        assert_eq!(full_version(&all, "1.7.2", "10.12.2.1161").as_deref(), Some("1.7.2-10.12.2.1161-mc172"));
        assert_eq!(full_version(&all, "1.20.1", "47.4.27"), None);
    }

    #[test]
    fn version_list() {
        let all = strings(&["1.20.1-47.4.9", "1.20.1-47.4.28", "1.20.1-47.4.29", "1.20-46.0.14", "1.12.2-14.23.5.2859"]);
        let promos = HashMap::from([("1.20.1-recommended".to_string(), "47.4.28".to_string())]);
        let v = filter_versions(&all, "1.20.1", &promos);
        assert_eq!(v.iter().map(|x| x.version.as_str()).collect::<Vec<_>>(), vec!["47.4.29", "47.4.28", "47.4.9"]);
        assert_eq!(v.iter().filter(|x| x.recommended).map(|x| x.version.as_str()).collect::<Vec<_>>(), vec!["47.4.28"]);
        assert!(filter_versions(&all, "1.8.9", &promos).is_empty());
    }

    #[test]
    fn installer_shapes() {
        // 1.7.10: everything in install_profile.json.
        let legacy = json!({
            "install": { "path": "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10", "filePath": "forge-universal.jar" },
            "versionInfo": { "id": "1.7.10-Forge10.13.4.1614-1.7.10", "libraries": [] },
        });
        match shape_of(legacy, |_| anyhow::bail!("not read")).unwrap() {
            Shape::Legacy { universal, coord, .. } => {
                assert_eq!(universal, "forge-universal.jar");
                assert_eq!(coord, "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10");
            }
            s => panic!("{s:?}"),
        }

        let version = br#"{"id":"1.20.1-forge-47.4.28","libraries":[]}"#;
        // Late 1.12.2: a version.json and no processors.
        let embedded = json!({ "json": "/version.json", "processors": [], "libraries": [] });
        assert!(matches!(shape_of(embedded, |n| { assert_eq!(n, "/version.json"); Ok(version.to_vec()) }).unwrap(), Shape::Embedded { .. }));
        // Server-only processors don't count.
        let server_only = json!({ "json": "/version.json", "processors": [{ "sides": ["server"], "jar": "x:y:1" }] });
        assert!(matches!(shape_of(server_only, |_| Ok(version.to_vec())).unwrap(), Shape::Embedded { .. }));
        // 1.13+: client processors.
        let modern = json!({ "json": "/version.json", "processors": [{ "sides": ["server"] }, { "jar": "x:y:1" }] });
        match shape_of(modern, |_| Ok(version.to_vec())).unwrap() {
            Shape::Processors { id } => assert_eq!(id, "1.20.1-forge-47.4.28"),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn legacy_json_points_at_the_extracted_jar() {
        let coord = "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10";
        let v = json!({ "id": "x", "libraries": [
            { "name": coord, "url": "https://maven.minecraftforge.net/" },
            { "name": "org.scala-lang:scala-library:2.11.1", "url": "http://files.minecraftforge.net/maven/" },
            { "name": "net.minecraft:launchwrapper:1.12", "serverreq": true },
        ]});
        let rel = maven::coord_to_path(coord).unwrap();
        let out = legacy_version_json(v, coord, &rel, "abc", 5);
        let libs: Vec<mojang::Library> = serde_json::from_value(out["libraries"].clone()).unwrap();
        let dir = std::path::Path::new("L");

        let forge = libs[0].artifact_download(dir).unwrap().unwrap();
        assert_eq!(forge.url, "", "the Forge jar comes from the installer");
        assert_eq!(forge.path, dir.join(&rel));
        assert_eq!(forge.sha1.as_deref(), Some("abc"));

        let scala = libs[1].artifact_download(dir).unwrap().unwrap();
        assert!(scala.url.starts_with("https://maven.minecraftforge.net/org/scala-lang/"), "{}", scala.url);
        // Libraries marked server-only in old profiles are still needed by the client.
        let lw = libs[2].artifact_download(dir).unwrap().unwrap();
        assert!(lw.url.starts_with("https://libraries.minecraft.net/"), "{}", lw.url);
    }
}
