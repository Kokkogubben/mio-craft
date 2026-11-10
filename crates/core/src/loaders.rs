//! Mod loaders: Fabric and NeoForge.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::http::{self, Download};
use crate::{java, maven, mojang, Launcher};

pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
pub const FABRIC_MAVEN: &str = "https://maven.fabricmc.net";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    Fabric,
    Neoforge,
}

impl Loader {
    pub fn as_str(&self) -> &'static str {
        match self {
            Loader::Vanilla => "vanilla",
            Loader::Fabric => "fabric",
            Loader::Neoforge => "neoforge",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

// ---------- Fabric ----------

#[derive(Deserialize)]
struct FabricLoaderEntry {
    loader: FabricLoaderInfo,
}
#[derive(Deserialize)]
struct FabricLoaderInfo {
    version: String,
    stable: bool,
}

pub async fn fabric_versions(l: &Launcher, mc: &str) -> Result<Vec<LoaderVersion>> {
    let entries: Vec<FabricLoaderEntry> =
        http::get_json(&l.http, &format!("{FABRIC_META}/versions/loader/{mc}")).await?;
    Ok(entries
        .into_iter()
        .map(|e| LoaderVersion { version: e.loader.version, stable: e.loader.stable })
        .collect())
}

/// Installs the Fabric profile JSON and returns its version id.
pub async fn install_fabric(l: &Launcher, mc: &str, loader: &str) -> Result<String> {
    let url = format!("{FABRIC_META}/versions/loader/{mc}/{loader}/profile/json");
    let profile: mojang::VersionJson = http::get_json(&l.http, &url)
        .await
        .with_context(|| format!("Fabric {loader} is not available for Minecraft {mc}"))?;
    let id = profile.id.clone();
    http::write_json(&l.paths.version_json(&id), &profile).await?;
    Ok(id)
}

/// Latest Fabric API build for a Minecraft version (versions look like `0.110.0+1.21.1`).
pub async fn fabric_api_version(l: &Launcher, mc: &str) -> Result<Option<String>> {
    let xml = http::get_text(
        &l.http,
        &format!("{FABRIC_MAVEN}/net/fabricmc/fabric-api/fabric-api/maven-metadata.xml"),
    )
    .await?;
    Ok(pick_fabric_api(&maven::metadata_versions(&xml), mc))
}

pub fn pick_fabric_api(versions: &[String], mc: &str) -> Option<String> {
    let suffix = format!("+{mc}");
    versions
        .iter()
        .filter(|v| v.ends_with(&suffix))
        .max_by(|a, b| maven::cmp_versions(a, b))
        .cloned()
}

// ---------- NeoForge ----------

#[derive(Deserialize)]
struct NeoVersions {
    versions: Vec<String>,
}

/// The NeoForge version prefix for a Minecraft version, e.g.
/// `1.21.1` → `21.1.`, `1.20.4` → `20.4.`, `1.21` → `21.0.`, `26.1` → `26.1.`.
/// NeoForge only exists for Minecraft 1.20.2 and newer.
pub fn neoforge_prefix(mc: &str) -> Option<String> {
    let parts: Vec<&str> = mc.split('.').collect();
    if parts.first() == Some(&"1") {
        let minor: u32 = parts.get(1)?.parse().ok()?;
        let patch: u32 = parts.get(2).map(|p| p.parse().ok()).unwrap_or(Some(0))?;
        if minor < 20 || (minor == 20 && patch < 2) {
            return None;
        }
        Some(format!("{minor}.{patch}."))
    } else {
        // Year-based versions: 26.1 → `26.1.0.x`, 26.1.1 → `26.1.1.x`.
        let year: u32 = parts.first()?.parse().ok()?;
        if year < 25 || parts.len() < 2 {
            return None;
        }
        if parts.len() == 2 {
            Some(format!("{mc}.0."))
        } else {
            Some(format!("{mc}."))
        }
    }
}

pub async fn neoforge_versions(l: &Launcher, mc: &str) -> Result<Vec<LoaderVersion>> {
    let Some(prefix) = neoforge_prefix(mc) else {
        return Ok(vec![]);
    };
    let all: NeoVersions = http::get_json(
        &l.http,
        "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge",
    )
    .await?;
    Ok(filter_neoforge(&all.versions, &prefix))
}

pub fn filter_neoforge(all: &[String], prefix: &str) -> Vec<LoaderVersion> {
    let mut v: Vec<LoaderVersion> = all
        .iter()
        .filter(|v| v.starts_with(prefix))
        .map(|v| LoaderVersion { version: v.clone(), stable: !v.contains("beta") && !v.contains("alpha") })
        .collect();
    v.sort_by(|a, b| maven::cmp_versions(&b.version, &a.version));
    v
}

/// Runs the official NeoForge installer against our data directory and
/// returns the installed version id (`neoforge-<version>`).
pub async fn install_neoforge(l: &Launcher, mc: &str, neo: &str) -> Result<String> {
    let id = format!("neoforge-{neo}");
    if l.paths.version_json(&id).exists() {
        return Ok(id);
    }
    // The installer needs the vanilla JSON and client jar present.
    let vanilla = mojang::fetch_version_json(l, mc).await?;
    mojang::install_files(l, mc).await?;
    let jdk = java::ensure_jdk(l, java::required_major(&vanilla).max(17)).await?;

    let profiles = l.paths.root.join("launcher_profiles.json");
    if !profiles.exists() {
        tokio::fs::write(&profiles, br#"{"profiles":{}}"#).await?;
    }
    let installer = l.paths.cache().join(format!("neoforge-{neo}-installer.jar"));
    http::download(
        &l.http,
        &Download {
            url: format!("{NEOFORGE_MAVEN}/net/neoforged/neoforge/{neo}/neoforge-{neo}-installer.jar"),
            path: installer.clone(),
            sha1: None,
            size: None,
        },
    )
    .await?;

    l.progress("neoforge", 0, 1, format!("Running NeoForge {neo} installer"));
    let mut last_output = String::new();
    for flag in ["--install-client", "--installClient"] {
        let mut cmd = tokio::process::Command::new(java::java_bin(&jdk));
        cmd.arg("-jar").arg(&installer).arg(flag).arg(&l.paths.root);
        cmd.current_dir(l.paths.cache());
        crate::process::hide_window(&mut cmd);
        let out = cmd.output().await.context("running the NeoForge installer")?;
        if out.status.success() && l.paths.version_json(&id).exists() {
            l.progress("neoforge", 1, 1, format!("NeoForge {neo} installed"));
            return Ok(id);
        }
        last_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let tail: Vec<&str> = last_output.lines().rev().take(30).collect();
    bail!(
        "NeoForge installer failed:\n{}",
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neo_prefixes() {
        assert_eq!(neoforge_prefix("1.21.1").as_deref(), Some("21.1."));
        assert_eq!(neoforge_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(neoforge_prefix("1.20.4").as_deref(), Some("20.4."));
        assert_eq!(neoforge_prefix("1.20.1"), None);
        assert_eq!(neoforge_prefix("1.19.2"), None);
        assert_eq!(neoforge_prefix("26.1").as_deref(), Some("26.1.0."));
        assert_eq!(neoforge_prefix("26.1.1").as_deref(), Some("26.1.1."));
    }

    #[test]
    fn neo_filter() {
        let all: Vec<String> = ["21.1.1", "21.1.77", "21.10.3-beta", "21.0.5", "26.1.0.1-beta", "26.1.1.4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let v = filter_neoforge(&all, &neoforge_prefix("1.21.1").unwrap());
        assert_eq!(v.iter().map(|x| x.version.as_str()).collect::<Vec<_>>(), vec!["21.1.77", "21.1.1"]);
        let v = filter_neoforge(&all, &neoforge_prefix("26.1").unwrap());
        assert_eq!(v.iter().map(|x| x.version.as_str()).collect::<Vec<_>>(), vec!["26.1.0.1-beta"]);
        let v = filter_neoforge(&all, &neoforge_prefix("26.1.1").unwrap());
        assert_eq!(v.iter().map(|x| x.version.as_str()).collect::<Vec<_>>(), vec!["26.1.1.4"]);
    }

    #[test]
    fn fabric_api_pick() {
        let v: Vec<String> = ["0.100.0+1.21", "0.102.0+1.21.1", "0.110.0+1.21.1", "0.99.0+1.21.10"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(pick_fabric_api(&v, "1.21.1").as_deref(), Some("0.110.0+1.21.1"));
        assert_eq!(pick_fabric_api(&v, "1.21").as_deref(), Some("0.100.0+1.21"));
    }
}
