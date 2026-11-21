//! Mod loaders: Fabric, Quilt, Legacy Fabric, NeoForge and Forge (in
//! `forge.rs`).

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::http::{self, Download};
use crate::{java, maven, mojang, Launcher};

pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
pub const QUILT_META: &str = "https://meta.quiltmc.org/v3";
/// Fabric for Minecraft 1.3 to 1.13.2, by a separate project.
pub const LEGACY_FABRIC_META: &str = "https://meta.legacyfabric.net/v2";
pub const FABRIC_MAVEN: &str = "https://maven.fabricmc.net";
pub const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    Fabric,
    Neoforge,
    Forge,
    Quilt,
    #[serde(rename = "legacyfabric")]
    LegacyFabric,
}

impl Loader {
    pub const ALL: [Loader; 6] =
        [Loader::Vanilla, Loader::Fabric, Loader::Neoforge, Loader::Forge, Loader::Quilt, Loader::LegacyFabric];

    pub fn as_str(&self) -> &'static str {
        match self {
            Loader::Vanilla => "vanilla",
            Loader::Fabric => "fabric",
            Loader::Neoforge => "neoforge",
            Loader::Forge => "forge",
            Loader::Quilt => "quilt",
            Loader::LegacyFabric => "legacyfabric",
        }
    }

    pub fn parse(s: &str) -> Option<Loader> {
        Loader::ALL.into_iter().find(|l| l.as_str() == s)
    }

    /// The name people know it by.
    pub fn display_name(&self) -> &'static str {
        match self {
            Loader::Vanilla => "Vanilla",
            Loader::Fabric => "Fabric",
            Loader::Neoforge => "NeoForge",
            Loader::Forge => "Forge",
            Loader::Quilt => "Quilt",
            Loader::LegacyFabric => "Legacy Fabric",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
    /// The build the loader's own site recommends (Forge only).
    pub recommended: bool,
}

/// Versions of `loader` for Minecraft `mc`, newest first. Empty when the
/// loader doesn't support that Minecraft version.
pub async fn versions(l: &Launcher, loader: Loader, mc: &str) -> Result<Vec<LoaderVersion>> {
    match loader {
        Loader::Vanilla => Ok(vec![]),
        Loader::Fabric | Loader::Quilt | Loader::LegacyFabric => fabric_like_versions(l, loader, mc).await,
        Loader::Neoforge => neoforge_versions(l, mc).await,
        Loader::Forge => crate::forge::versions(l, mc).await,
    }
}

/// The version to use when none was picked: the recommended one, else the
/// newest stable one, else the newest.
pub fn default_version(v: &[LoaderVersion]) -> Option<&LoaderVersion> {
    v.iter().find(|x| x.recommended).or_else(|| v.iter().find(|x| x.stable)).or(v.first())
}

/// Why `loader` has nothing for `mc`, for error messages.
pub fn unsupported_message(loader: Loader, mc: &str) -> String {
    match loader {
        Loader::Neoforge => "NeoForge supports Minecraft 1.20.2 and newer".into(),
        Loader::LegacyFabric => format!("Legacy Fabric supports Minecraft 1.3 to 1.13.2, not {mc}"),
        Loader::Fabric => format!("Fabric doesn't support Minecraft {mc} (for 1.13.2 and older, use Legacy Fabric)"),
        _ => format!("{} has no builds for Minecraft {mc}", loader.display_name()),
    }
}

/// Install `loader` version `version` for Minecraft `mc` and return the
/// version id to launch.
pub async fn install(l: &Launcher, loader: Loader, mc: &str, version: Option<&str>) -> Result<String> {
    if loader == Loader::Vanilla {
        return Ok(mc.to_string());
    }
    let v = version.ok_or_else(|| anyhow!("no {} version set", loader.display_name()))?;
    match loader {
        Loader::Vanilla => unreachable!(),
        Loader::Fabric | Loader::Quilt | Loader::LegacyFabric => install_fabric_like(l, loader, mc, v).await,
        Loader::Neoforge => install_neoforge(l, mc, v).await,
        Loader::Forge => crate::forge::install(l, mc, v).await,
    }
}

// ---------- Fabric, Quilt and Legacy Fabric ----------
//
// All three publish the same kind of metadata service, which hands out a
// ready-made version JSON ("profile") per loader version.

fn meta_base(loader: Loader) -> Result<&'static str> {
    match loader {
        Loader::Fabric => Ok(FABRIC_META),
        Loader::Quilt => Ok(QUILT_META),
        Loader::LegacyFabric => Ok(LEGACY_FABRIC_META),
        _ => bail!("{} isn't a Fabric-style loader", loader.display_name()),
    }
}

#[derive(Deserialize)]
struct FabricLoaderEntry {
    loader: FabricLoaderInfo,
}
#[derive(Deserialize)]
struct FabricLoaderInfo {
    version: String,
    /// Quilt doesn't say; its pre-releases have `-beta` style suffixes.
    #[serde(default)]
    stable: Option<bool>,
}

fn is_prerelease(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    ["alpha", "beta", "pre", "rc"].iter().any(|t| v.contains(t))
}

pub fn parse_fabric_like(entries: Vec<serde_json::Value>) -> Vec<LoaderVersion> {
    let mut out: Vec<LoaderVersion> = entries
        .into_iter()
        .filter_map(|e| serde_json::from_value::<FabricLoaderEntry>(e).ok())
        .map(|e| LoaderVersion {
            stable: e.loader.stable.unwrap_or_else(|| !is_prerelease(&e.loader.version)),
            version: e.loader.version,
            recommended: false,
        })
        .collect();
    out.sort_by(|a, b| maven::cmp_versions(&b.version, &a.version));
    out
}

pub async fn fabric_like_versions(l: &Launcher, loader: Loader, mc: &str) -> Result<Vec<LoaderVersion>> {
    let url = format!("{}/versions/loader/{}", meta_base(loader)?, crate::browse::urlencode(mc));
    let resp = l.http.get(&url).send().await.with_context(|| format!("GET {url}"))?;
    // Quilt answers 404 and Legacy Fabric 400 for Minecraft versions they don't support.
    if matches!(resp.status(), reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::BAD_REQUEST) {
        return Ok(vec![]);
    }
    let entries: Vec<serde_json::Value> = resp.error_for_status()?.json().await?;
    Ok(parse_fabric_like(entries))
}

/// Installs the loader's profile JSON and returns its version id.
pub async fn install_fabric_like(l: &Launcher, loader: Loader, mc: &str, version: &str) -> Result<String> {
    let url = format!("{}/versions/loader/{mc}/{version}/profile/json", meta_base(loader)?);
    let profile: mojang::VersionJson = http::get_json(&l.http, &url)
        .await
        .with_context(|| format!("{} {version} is not available for Minecraft {mc}", loader.display_name()))?;
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
        .map(|v| LoaderVersion { version: v.clone(), stable: !v.contains("beta") && !v.contains("alpha"), recommended: false })
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
    run_installer(l, &format!("NeoForge {neo}"), &installer, &jdk, &id, &["--install-client", "--installClient"]).await
}

/// Run an official Forge-style installer jar against the data folder, trying
/// each client-install flag in turn, and return `id` once its version JSON
/// exists.
pub(crate) async fn run_installer(
    l: &Launcher,
    name: &str,
    installer: &Path,
    jdk: &Path,
    id: &str,
    flags: &[&str],
) -> Result<String> {
    // The installers refuse to run without the vanilla launcher's profile file.
    let profiles = l.paths.root.join("launcher_profiles.json");
    if !profiles.exists() {
        tokio::fs::write(&profiles, br#"{"profiles":{}}"#).await?;
    }
    l.progress("loader", 0, 1, format!("Running the {name} installer"));
    let mut last_output = String::new();
    for flag in flags {
        let mut cmd = tokio::process::Command::new(java::java_bin(jdk));
        cmd.arg("-jar").arg(installer).arg(flag).arg(&l.paths.root);
        cmd.current_dir(l.paths.cache());
        crate::process::hide_window(&mut cmd);
        let out = cmd.output().await.with_context(|| format!("running the {name} installer"))?;
        if out.status.success() && l.paths.version_json(id).exists() {
            l.progress("loader", 1, 1, format!("{name} installed"));
            return Ok(id.to_string());
        }
        last_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let tail: Vec<&str> = last_output.lines().rev().take(30).collect();
    bail!(
        "the {name} installer failed:\n{}",
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
    fn loader_names_round_trip() {
        for l in Loader::ALL {
            assert_eq!(Loader::parse(l.as_str()), Some(l));
            assert_eq!(serde_json::to_value(l).unwrap(), serde_json::json!(l.as_str()));
        }
        assert_eq!(Loader::parse("liteloader"), None);
    }

    #[test]
    fn fabric_like_lists() {
        // Fabric and Legacy Fabric say which builds are stable; Quilt doesn't.
        let quilt = serde_json::json!([
            { "loader": { "version": "0.20.0-beta.9", "maven": "x" } },
            { "loader": { "version": "0.29.1", "maven": "x" } },
            { "loader": { "version": "0.29.2-beta.1", "maven": "x" } },
        ]);
        let v = parse_fabric_like(serde_json::from_value(quilt).unwrap());
        assert_eq!(v.iter().map(|x| (x.version.as_str(), x.stable)).collect::<Vec<_>>(),
            vec![("0.29.2-beta.1", false), ("0.29.1", true), ("0.20.0-beta.9", false)]);
        assert_eq!(default_version(&v).unwrap().version, "0.29.1");

        let fabric = serde_json::json!([{ "loader": { "version": "0.16.9", "stable": true } }, { "loader": { "version": "0.17.0", "stable": false } }]);
        let v = parse_fabric_like(serde_json::from_value(fabric).unwrap());
        assert_eq!(default_version(&v).unwrap().version, "0.16.9");
        assert!(default_version(&[]).is_none());
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
