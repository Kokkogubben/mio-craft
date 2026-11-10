//! Built-in mod browser: search and install third-party mods from Modrinth
//! and CurseForge, filtered to the instance's Minecraft version and loader,
//! with required dependencies installed too.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http::{self, Download};
use crate::instances::{self, Instance};
use crate::loaders::Loader;
use crate::{mods, Event, Launcher};

pub const MODRINTH_API: &str = "https://api.modrinth.com/v2";

/// Overridable for tests.
fn modrinth_api() -> String {
    std::env::var("CLAUDE_LAUNCHER_MODRINTH_API").unwrap_or_else(|_| MODRINTH_API.to_string())
}
pub const CURSEFORGE_API: &str = "https://api.curseforge.com/v1";
const CF_MINECRAFT: u32 = 432;
const CF_CLASS_MODS: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Modrinth,
    Curseforge,
}

impl Source {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "modrinth" => Ok(Self::Modrinth),
            "curseforge" => Ok(Self::Curseforge),
            _ => bail!("unknown mod source {s:?} (use modrinth or curseforge)"),
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Modrinth => "Modrinth",
            Self::Curseforge => "CurseForge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Relevance,
    Downloads,
    Updated,
    Newest,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub source: Source,
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub page_url: String,
    pub categories: Vec<String>,
    /// Set by the API layer when the instance already has this project.
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub total: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModFile {
    pub url: Option<String>,
    pub filename: String,
    pub sha1: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    pub project_id: String,
    pub version_id: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModVersion {
    pub source: Source,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub version_number: String,
    pub date: String,
    /// `release`, `beta` or `alpha`.
    pub channel: String,
    pub file: ModFile,
    pub dependencies: Vec<Dependency>,
}

/// What we remember about mods installed from the browser:
/// `instances/<id>/online-mods.json`, jar file name → origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineMod {
    pub source: Source,
    pub project_id: String,
    pub version_id: String,
    pub title: String,
    pub page_url: String,
    /// Installed only because another mod needed it.
    #[serde(default)]
    pub as_dependency: bool,
    /// Where the jar was downloaded from (used for modpack export).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

pub type OnlineTags = HashMap<String, OnlineMod>;

fn tags_file(l: &Launcher, instance_id: &str) -> std::path::PathBuf {
    l.paths.instance_dir(instance_id).join("online-mods.json")
}

pub async fn load_tags(l: &Launcher, instance_id: &str) -> OnlineTags {
    http::read_json(&tags_file(l, instance_id)).await.unwrap_or_default()
}

async fn save_tags(l: &Launcher, instance_id: &str, t: &OnlineTags) -> Result<()> {
    http::write_json(&tags_file(l, instance_id), t).await
}

/// Forget tags whose jar is gone (deleted from the launcher or by hand).
pub async fn prune_tags(l: &Launcher, instance_id: &str) -> OnlineTags {
    let mut tags = load_tags(l, instance_id).await;
    let dir = l.paths.mods_dir(instance_id);
    let before = tags.len();
    tags.retain(|f, _| dir.join(f).exists() || dir.join(format!("{f}.disabled")).exists());
    if tags.len() != before {
        let _ = save_tags(l, instance_id, &tags).await;
    }
    tags
}

fn loader_name(loader: Loader) -> Result<&'static str> {
    match loader {
        Loader::Fabric => Ok("fabric"),
        Loader::Neoforge => Ok("neoforge"),
        Loader::Vanilla => bail!("vanilla instances can't load mods; create a Fabric or NeoForge instance"),
    }
}

fn cf_loader_type(loader: Loader) -> Result<u32> {
    match loader {
        Loader::Fabric => Ok(4),
        Loader::Neoforge => Ok(6),
        Loader::Vanilla => bail!("vanilla instances can't load mods; create a Fabric or NeoForge instance"),
    }
}

// ---------- Modrinth ----------

pub fn modrinth_facets(mc: &str, loader: &str) -> String {
    serde_json::to_string(&vec![
        vec!["project_type:mod".to_string()],
        vec![format!("categories:{loader}")],
        vec![format!("versions:{mc}")],
    ])
    .unwrap()
}

pub fn parse_modrinth_search(v: &Value, offset: u64) -> SearchPage {
    let hits = v["hits"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|h| {
                    let slug = h["slug"].as_str().unwrap_or_default().to_string();
                    SearchHit {
                        source: Source::Modrinth,
                        project_id: h["project_id"].as_str().unwrap_or_default().to_string(),
                        title: h["title"].as_str().unwrap_or(&slug).to_string(),
                        description: h["description"].as_str().unwrap_or_default().to_string(),
                        author: h["author"].as_str().unwrap_or_default().to_string(),
                        downloads: h["downloads"].as_u64().unwrap_or(0),
                        icon_url: h["icon_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
                        page_url: format!("https://modrinth.com/mod/{slug}"),
                        categories: h["display_categories"]
                            .as_array()
                            .or(h["categories"].as_array())
                            .map(|c| c.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_default(),
                        slug,
                        installed: false,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    SearchPage { hits, total: v["total_hits"].as_u64().unwrap_or(0), offset }
}

pub fn parse_modrinth_versions(v: &Value, project_id: &str) -> Vec<ModVersion> {
    let Some(arr) = v.as_array() else { return vec![] };
    arr.iter()
        .filter_map(|ver| {
            let files = ver["files"].as_array()?;
            let f = files.iter().find(|f| f["primary"].as_bool() == Some(true)).or(files.first())?;
            Some(ModVersion {
                source: Source::Modrinth,
                project_id: ver["project_id"].as_str().unwrap_or(project_id).to_string(),
                version_id: ver["id"].as_str()?.to_string(),
                name: ver["name"].as_str().unwrap_or_default().to_string(),
                version_number: ver["version_number"].as_str().unwrap_or_default().to_string(),
                date: ver["date_published"].as_str().unwrap_or_default().to_string(),
                channel: ver["version_type"].as_str().unwrap_or("release").to_string(),
                file: ModFile {
                    url: f["url"].as_str().map(String::from),
                    filename: f["filename"].as_str()?.to_string(),
                    sha1: f["hashes"]["sha1"].as_str().map(String::from),
                    size: f["size"].as_u64().unwrap_or(0),
                },
                dependencies: ver["dependencies"]
                    .as_array()
                    .map(|d| {
                        d.iter()
                            .filter_map(|d| {
                                let kind = d["dependency_type"].as_str()?;
                                if kind != "required" && kind != "optional" {
                                    return None; // incompatible / embedded
                                }
                                Some(Dependency {
                                    project_id: d["project_id"].as_str()?.to_string(),
                                    version_id: d["version_id"].as_str().map(String::from),
                                    required: kind == "required",
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

async fn modrinth_get(l: &Launcher, url: &str) -> Result<Value> {
    let resp = l.http.get(url).send().await.with_context(|| format!("contacting Modrinth ({url})"))?;
    if resp.status().as_u16() == 404 {
        bail!("not found on Modrinth");
    }
    if !resp.status().is_success() {
        bail!("Modrinth returned {}", resp.status());
    }
    Ok(resp.json().await?)
}

// ---------- CurseForge ----------

async fn cf_key(l: &Launcher) -> Result<String> {
    l.config.read().await.curseforge_api_key.clone().ok_or_else(|| {
        anyhow!("CurseForge needs an API key. Get one free at console.curseforge.com and add it in Settings.")
    })
}

async fn cf_get(l: &Launcher, path_and_query: &str) -> Result<Value> {
    let key = cf_key(l).await?;
    let url = format!("{CURSEFORGE_API}{path_and_query}");
    let resp = l
        .http
        .get(&url)
        .header("x-api-key", key)
        .header("Accept", "application/json")
        .send()
        .await
        .context("contacting CurseForge")?;
    match resp.status().as_u16() {
        200..=299 => Ok(resp.json().await?),
        401 | 403 => bail!("CurseForge rejected the API key. Check it in Settings."),
        404 => bail!("not found on CurseForge"),
        s => bail!("CurseForge returned {s}"),
    }
}

pub fn parse_cf_search(v: &Value, offset: u64) -> SearchPage {
    let hits = v["data"]
        .as_array()
        .map(|a| a.iter().map(cf_hit).collect())
        .unwrap_or_default();
    SearchPage { hits, total: v["pagination"]["totalCount"].as_u64().unwrap_or(0), offset }
}

fn cf_hit(m: &Value) -> SearchHit {
    let slug = m["slug"].as_str().unwrap_or_default().to_string();
    SearchHit {
        source: Source::Curseforge,
        project_id: m["id"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
        title: m["name"].as_str().unwrap_or(&slug).to_string(),
        description: m["summary"].as_str().unwrap_or_default().to_string(),
        author: m["authors"][0]["name"].as_str().unwrap_or_default().to_string(),
        downloads: m["downloadCount"].as_f64().unwrap_or(0.0) as u64,
        icon_url: m["logo"]["thumbnailUrl"].as_str().filter(|s| !s.is_empty()).map(String::from),
        page_url: m["links"]["websiteUrl"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("https://www.curseforge.com/minecraft/mc-mods/{slug}")),
        categories: m["categories"]
            .as_array()
            .map(|c| c.iter().filter_map(|x| x["name"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
        slug,
        installed: false,
    }
}

pub fn parse_cf_files(v: &Value, project_id: &str) -> Vec<ModVersion> {
    let Some(arr) = v["data"].as_array() else { return vec![] };
    let mut out: Vec<ModVersion> = arr
        .iter()
        .filter(|f| f["isAvailable"].as_bool() != Some(false))
        .filter_map(|f| {
            let channel = match f["releaseType"].as_u64() {
                Some(2) => "beta",
                Some(3) => "alpha",
                _ => "release",
            };
            let sha1 = f["hashes"].as_array().and_then(|h| {
                h.iter().find(|x| x["algo"].as_u64() == Some(1)).and_then(|x| x["value"].as_str().map(String::from))
            });
            let filename = f["fileName"].as_str()?.to_string();
            Some(ModVersion {
                source: Source::Curseforge,
                project_id: f["modId"].as_u64().map(|i| i.to_string()).unwrap_or_else(|| project_id.to_string()),
                version_id: f["id"].as_u64()?.to_string(),
                name: f["displayName"].as_str().unwrap_or(&filename).to_string(),
                version_number: f["displayName"].as_str().unwrap_or(&filename).to_string(),
                date: f["fileDate"].as_str().unwrap_or_default().to_string(),
                channel: channel.into(),
                file: ModFile {
                    // null when the author has disabled third-party downloads
                    url: f["downloadUrl"].as_str().filter(|s| !s.is_empty()).map(String::from),
                    filename,
                    sha1,
                    size: f["fileLength"].as_u64().unwrap_or(0),
                },
                dependencies: f["dependencies"]
                    .as_array()
                    .map(|d| {
                        d.iter()
                            .filter_map(|d| {
                                let rel = d["relationType"].as_u64()?;
                                // 3 = required, 2 = optional; others are embedded/tool/incompatible/include
                                if rel != 3 && rel != 2 {
                                    return None;
                                }
                                Some(Dependency {
                                    project_id: d["modId"].as_u64()?.to_string(),
                                    version_id: None,
                                    required: rel == 3,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect();
    out.sort_by(|a, b| b.date.cmp(&a.date));
    out
}

// ---------- public operations ----------

pub async fn search(
    l: &Launcher,
    source: Source,
    inst: &Instance,
    query: &str,
    sort: Sort,
    offset: u64,
    limit: u64,
) -> Result<SearchPage> {
    let limit = limit.clamp(1, 50);
    let enc = |s: &str| urlencode(s);
    match source {
        Source::Modrinth => {
            let loader = loader_name(inst.loader)?;
            let index = match sort {
                Sort::Relevance => "relevance",
                Sort::Downloads => "downloads",
                Sort::Updated => "updated",
                Sort::Newest => "newest",
            };
            let url = format!(
                "{}/search?query={}&facets={}&index={index}&offset={offset}&limit={limit}",
                modrinth_api(),
                enc(query),
                enc(&modrinth_facets(&inst.mc_version, loader)),
            );
            Ok(parse_modrinth_search(&modrinth_get(l, &url).await?, offset))
        }
        Source::Curseforge => {
            let lt = cf_loader_type(inst.loader)?;
            // 1 Featured, 2 Popularity, 3 LastUpdated, 6 TotalDownloads, 11 ReleasedDate
            let field = match sort {
                Sort::Relevance => if query.trim().is_empty() { 2 } else { 1 },
                Sort::Downloads => 6,
                Sort::Updated => 3,
                Sort::Newest => 11,
            };
            // CurseForge caps index + pageSize at 10,000.
            let offset = offset.min(10_000 - limit);
            let path = format!(
                "/mods/search?gameId={CF_MINECRAFT}&classId={CF_CLASS_MODS}&searchFilter={}&gameVersion={}&modLoaderType={lt}&sortField={field}&sortOrder=desc&index={offset}&pageSize={limit}",
                enc(query),
                enc(&inst.mc_version),
            );
            Ok(parse_cf_search(&cf_get(l, &path).await?, offset))
        }
    }
}

/// Versions of a project that work with the instance, newest first.
pub async fn versions(l: &Launcher, source: Source, project_id: &str, inst: &Instance) -> Result<Vec<ModVersion>> {
    match source {
        Source::Modrinth => {
            let loader = loader_name(inst.loader)?;
            let url = format!(
                "{}/project/{}/version?loaders={}&game_versions={}",
                modrinth_api(),
                urlencode(project_id),
                urlencode(&format!("[\"{loader}\"]")),
                urlencode(&format!("[\"{}\"]", inst.mc_version)),
            );
            Ok(parse_modrinth_versions(&modrinth_get(l, &url).await?, project_id))
        }
        Source::Curseforge => {
            let lt = cf_loader_type(inst.loader)?;
            let id: u64 = project_id.parse().map_err(|_| anyhow!("CurseForge project ids are numbers"))?;
            let path = format!(
                "/mods/{id}/files?gameVersion={}&modLoaderType={lt}&pageSize=50",
                urlencode(&inst.mc_version)
            );
            Ok(parse_cf_files(&cf_get(l, &path).await?, project_id))
        }
    }
}

/// Prefer the newest release; fall back to betas, then alphas.
pub fn pick_version(versions: &[ModVersion]) -> Option<&ModVersion> {
    for ch in ["release", "beta", "alpha"] {
        if let Some(v) = versions.iter().find(|v| v.channel == ch) {
            return Some(v);
        }
    }
    versions.first()
}

async fn project_info(l: &Launcher, source: Source, project_id: &str) -> Result<(String, String, String)> {
    // (title, slug, page_url)
    match source {
        Source::Modrinth => {
            let v = modrinth_get(l, &format!("{}/project/{}", modrinth_api(), urlencode(project_id))).await?;
            let slug = v["slug"].as_str().unwrap_or(project_id).to_string();
            Ok((v["title"].as_str().unwrap_or(&slug).to_string(), slug.clone(), format!("https://modrinth.com/mod/{slug}")))
        }
        Source::Curseforge => {
            let v = cf_get(l, &format!("/mods/{}", urlencode(project_id))).await?;
            let h = cf_hit(&v["data"]);
            Ok((h.title, h.slug, h.page_url))
        }
    }
}

async fn version_by_id(l: &Launcher, source: Source, project_id: &str, version_id: &str, inst: &Instance) -> Result<ModVersion> {
    match source {
        Source::Modrinth => {
            let v = modrinth_get(l, &format!("{}/version/{}", modrinth_api(), urlencode(version_id))).await?;
            parse_modrinth_versions(&Value::Array(vec![v]), project_id)
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("that Modrinth version has no files"))
        }
        Source::Curseforge => versions(l, source, project_id, inst)
            .await?
            .into_iter()
            .find(|v| v.version_id == version_id)
            .ok_or_else(|| anyhow!("that CurseForge file isn't available for this instance")),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallReport {
    /// Jar files added to the mods folder (the mod first, then dependencies).
    pub installed: Vec<String>,
    /// Things that were already there.
    pub already_installed: Vec<String>,
    /// Problems with dependencies that didn't stop the main install.
    pub warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub source: String,
    pub project_id: String,
    /// Defaults to the newest compatible version.
    pub version_id: Option<String>,
}

/// Install a mod (and its required dependencies) into an instance.
pub async fn install(l: &Launcher, instance_id: &str, req: InstallRequest) -> Result<InstallReport> {
    install_with(l, instance_id, req, None).await
}

/// `prefetched` is the version to install for the main project, when the
/// caller already has it (saves a request).
async fn install_with(l: &Launcher, instance_id: &str, req: InstallRequest, prefetched: Option<ModVersion>) -> Result<InstallReport> {
    let source = Source::parse(&req.source)?;
    let inst = instances::get(l, instance_id).await?;
    loader_name(inst.loader)?;
    if l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        bail!("stop the game before adding mods");
    }
    let mut report = InstallReport { installed: vec![], already_installed: vec![], warnings: vec![] };

    // Mod ids already present (from jar metadata) and projects already installed from the web.
    let existing_mods = mods::list(l, &inst.id).await?;
    let mut present_ids: HashSet<String> = existing_mods.iter().filter_map(|m| m.mod_id.clone()).collect();
    let mut tags = prune_tags(l, &inst.id).await;
    let mut present_projects: HashSet<(Source, String)> =
        tags.values().map(|t| (t.source, t.project_id.clone())).collect();

    // (project_id, version_id, is_dependency)
    let mut queue: Vec<(String, Option<String>, bool)> = vec![(req.project_id.clone(), req.version_id.clone(), false)];
    let mut seen = HashSet::new();
    while let Some((pid, vid, as_dep)) = queue.pop() {
        if !seen.insert(pid.clone()) {
            continue;
        }
        if seen.len() > 40 {
            report.warnings.push("Stopped after 40 dependencies".into());
            break;
        }
        let result: Result<()> = async {
            let (title, slug, page_url) = project_info(l, source, &pid).await?;
            let already = present_projects.contains(&(source, pid.clone()))
                || present_ids.contains(&slug)
                || present_ids.contains(&slug.replace('-', "_"));
            if already && (as_dep || vid.is_none()) {
                // For the main mod with an explicit version we reinstall (switch version).
                report.already_installed.push(title);
                return Ok(());
            }
            let pre = prefetched
                .as_ref()
                .filter(|p| !as_dep && Some(&p.version_id) == vid.as_ref())
                .cloned();
            let version = match (&vid, pre) {
                (_, Some(p)) => p,
                (Some(v), None) => version_by_id(l, source, &pid, v, &inst).await?,
                (None, None) => {
                    let all = versions(l, source, &pid, &inst).await?;
                    pick_version(&all)
                        .cloned()
                        .ok_or_else(|| anyhow!("{title} has no version for {} {}", inst.loader.as_str(), inst.mc_version))?
                }
            };
            let Some(url) = version.file.url.clone() else {
                bail!(
                    "The author of {title} doesn't allow downloads from other apps. Download it from {page_url} and drop it in the mods folder."
                );
            };
            if version.file.filename.contains('/') || version.file.filename.contains('\\') || !version.file.filename.ends_with(".jar") {
                bail!("{title} has an unexpected file name ({})", version.file.filename);
            }
            // Replace older files of the same project.
            let dir = l.paths.mods_dir(&inst.id);
            let old: Vec<String> = tags
                .iter()
                .filter(|(_, t)| t.source == source && t.project_id == pid)
                .map(|(f, _)| f.clone())
                .collect();
            for f in old {
                let _ = tokio::fs::remove_file(dir.join(&f)).await;
                let _ = tokio::fs::remove_file(dir.join(format!("{f}.disabled"))).await;
                tags.remove(&f);
            }
            l.progress("mods", 0, 1, format!("Downloading {title}"));
            http::download(
                &l.http,
                &Download {
                    url: url.clone(),
                    path: dir.join(&version.file.filename),
                    sha1: version.file.sha1.clone(),
                    size: if version.file.size > 0 { Some(version.file.size) } else { None },
                },
            )
            .await
            .with_context(|| format!("downloading {title}"))?;
            tags.insert(
                version.file.filename.clone(),
                OnlineMod {
                    source,
                    project_id: pid.clone(),
                    version_id: version.version_id.clone(),
                    title: title.clone(),
                    page_url,
                    as_dependency: as_dep,
                    download_url: Some(url),
                },
            );
            save_tags(l, &inst.id, &tags).await?;
            present_projects.insert((source, pid.clone()));
            present_ids.insert(slug);
            report.installed.push(version.file.filename.clone());
            for d in version.dependencies.iter().filter(|d| d.required) {
                queue.push((d.project_id.clone(), d.version_id.clone(), true));
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            if as_dep {
                report.warnings.push(format!("Dependency {pid}: {e:#}"));
            } else {
                return Err(e);
            }
        }
    }
    l.progress("mods", 1, 1, "Mods installed");
    l.emit(Event::ModsChanged { instance_id: inst.id.clone() });
    Ok(report)
}

// ---------- updates ----------

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub file_name: String,
    pub title: String,
    pub source: Source,
    pub project_id: String,
    pub current_version_id: String,
    pub latest: ModVersion,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    pub updates: Vec<UpdateInfo>,
    pub checked: usize,
    pub warnings: Vec<String>,
}

/// Decide whether `versions` (newest first) has something newer than `current`.
/// A version that's older in the list than the one installed never counts,
/// so a newer beta isn't "updated" back to an older release.
pub fn newer_version<'a>(versions: &'a [ModVersion], current: &str) -> Option<&'a ModVersion> {
    let latest = pick_version(versions)?;
    if latest.version_id == current {
        return None;
    }
    match versions.iter().position(|v| v.version_id == current) {
        Some(cur_idx) => {
            let latest_idx = versions.iter().position(|v| v.version_id == latest.version_id)?;
            (latest_idx < cur_idx).then_some(latest)
        }
        // Installed version no longer listed for this instance: offer the latest.
        None => Some(latest),
    }
}

pub async fn check_updates(l: &Launcher, instance_id: &str) -> Result<UpdateCheck> {
    let inst = instances::get(l, instance_id).await?;
    loader_name(inst.loader)?;
    let tags = prune_tags(l, &inst.id).await;
    let has_cf_key = l.config.read().await.curseforge_api_key.is_some();
    let mut warnings = vec![];
    let items: Vec<(String, OnlineMod)> = tags
        .into_iter()
        .filter(|(_, t)| {
            if t.source == Source::Curseforge && !has_cf_key {
                false
            } else {
                true
            }
        })
        .collect();
    if !has_cf_key && load_tags(l, &inst.id).await.values().any(|t| t.source == Source::Curseforge) {
        warnings.push("Skipped CurseForge mods: add a CurseForge API key in Settings to check them.".into());
    }
    let checked = items.len();
    l.progress("updates", 0, checked as u64, "Checking for mod updates");
    use futures::StreamExt;
    let results: Vec<(String, OnlineMod, Result<Vec<ModVersion>>)> = futures::stream::iter(items)
        .map(|(file, tag)| {
            let (l, inst) = (l.clone(), inst.clone());
            async move {
                let v = versions(&l, tag.source, &tag.project_id, &inst).await;
                (file, tag, v)
            }
        })
        .buffer_unordered(6)
        .collect()
        .await;
    let mut updates = vec![];
    for (file, tag, v) in results {
        match v {
            Ok(vs) => {
                if let Some(n) = newer_version(&vs, &tag.version_id) {
                    updates.push(UpdateInfo {
                        file_name: file,
                        title: tag.title.clone(),
                        source: tag.source,
                        project_id: tag.project_id.clone(),
                        current_version_id: tag.version_id.clone(),
                        latest: n.clone(),
                    });
                }
            }
            Err(e) => warnings.push(format!("{}: {e:#}", tag.title)),
        }
    }
    updates.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    l.progress("updates", checked as u64, checked as u64, format!("{} update(s) available", updates.len()));
    Ok(UpdateCheck { updates, checked, warnings })
}

/// Update the given jar files (or every mod with an update when `files` is None).
pub async fn apply_updates(l: &Launcher, instance_id: &str, files: Option<Vec<String>>) -> Result<InstallReport> {
    let check = check_updates(l, instance_id).await?;
    let mut report = InstallReport { installed: vec![], already_installed: vec![], warnings: check.warnings };
    for u in check.updates {
        if let Some(f) = &files {
            if !f.contains(&u.file_name) {
                continue;
            }
        }
        let was_dep = load_tags(l, instance_id).await.get(&u.file_name).map(|t| t.as_dependency).unwrap_or(false);
        let r = install_with(
            l,
            instance_id,
            InstallRequest {
                source: match u.source {
                    Source::Modrinth => "modrinth".into(),
                    Source::Curseforge => "curseforge".into(),
                },
                project_id: u.project_id.clone(),
                version_id: Some(u.latest.version_id.clone()),
            },
            Some(u.latest.clone()),
        )
        .await;
        match r {
            Ok(r) => {
                if was_dep {
                    // Keep "installed as a dependency" across updates.
                    let mut tags = load_tags(l, instance_id).await;
                    if let Some(t) = r.installed.first().and_then(|f| tags.get_mut(f)) {
                        t.as_dependency = true;
                    }
                    save_tags(l, instance_id, &tags).await?;
                }
                report.installed.extend(r.installed);
                report.warnings.extend(r.warnings);
            }
            Err(e) => report.warnings.push(format!("{}: {e:#}", u.title)),
        }
    }
    l.emit(Event::ModsChanged { instance_id: instance_id.into() });
    Ok(report)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn facets() {
        assert_eq!(
            modrinth_facets("1.21.1", "fabric"),
            r#"[["project_type:mod"],["categories:fabric"],["versions:1.21.1"]]"#
        );
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }

    #[test]
    fn modrinth_parsing() {
        let s = json!({"hits":[{"project_id":"AANobbMI","slug":"sodium","title":"Sodium","description":"Fast","author":"jellysquid3","downloads":5,"icon_url":"https://cdn/x.png","display_categories":["optimization"]}],"total_hits":1});
        let p = parse_modrinth_search(&s, 0);
        assert_eq!(p.hits[0].page_url, "https://modrinth.com/mod/sodium");
        assert_eq!(p.hits[0].categories, vec!["optimization"]);

        let v = json!([
          {"id":"v2","project_id":"P","name":"2.0 beta","version_number":"2.0","version_type":"beta","date_published":"2026-02-01",
           "files":[{"url":"https://cdn/b.jar","filename":"b.jar","primary":true,"hashes":{"sha1":"aa"},"size":10}],
           "dependencies":[{"project_id":"P7dR8mSH","dependency_type":"required"},{"project_id":"X","dependency_type":"incompatible"}]},
          {"id":"v1","project_id":"P","name":"1.0","version_number":"1.0","version_type":"release","date_published":"2026-01-01",
           "files":[{"url":"https://cdn/src.jar","filename":"src.jar","primary":false},{"url":"https://cdn/a.jar","filename":"a.jar","primary":true}],
           "dependencies":[]}
        ]);
        let vs = parse_modrinth_versions(&v, "P");
        assert_eq!(vs.len(), 2);
        assert_eq!(vs[0].dependencies.len(), 1);
        assert!(vs[0].dependencies[0].required);
        assert_eq!(vs[1].file.filename, "a.jar");
        // A release beats a newer beta.
        assert_eq!(pick_version(&vs).unwrap().version_id, "v1");
    }

    /// Full install against a fake Modrinth: main mod + required dependency,
    /// checksum verified, tags written, and a second install is a no-op.
    #[tokio::test]
    async fn installs_with_dependencies_from_fake_modrinth() {
        use axum::{extract::Path as P, routing::get, Json, Router};
        use sha1::{Digest, Sha1};

        let main_jar = b"main-jar-bytes".to_vec();
        let dep_jar = b"dependency-jar-bytes".to_vec();
        let sha = |b: &[u8]| hex::encode(Sha1::digest(b));
        let (main_sha, dep_sha) = (sha(&main_jar), sha(&dep_jar));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let b2 = base.clone();
        let app = Router::new()
            .route("/project/{id}", get(|P(id): P<String>| async move {
                Json(json!({"slug": if id == "MAIN" {"glow-mod"} else {"fabric-api"}, "title": if id == "MAIN" {"Glow Mod"} else {"Fabric API"}}))
            }))
            .route("/project/{id}/version", get(move |P(id): P<String>| {
                let (base, ms, ds) = (b2.clone(), main_sha.clone(), dep_sha.clone());
                async move {
                    if id == "MAIN" {
                        Json(json!([{"id":"m1","project_id":"MAIN","name":"Glow 1.0","version_number":"1.0","version_type":"release","date_published":"x",
                            "files":[{"url":format!("{base}/files/glow.jar"),"filename":"glow-1.0.jar","primary":true,"hashes":{"sha1":ms},"size":14}],
                            "dependencies":[{"project_id":"DEP","dependency_type":"required"},{"project_id":"OPT","dependency_type":"optional"}]}]))
                    } else {
                        let v1 = json!({"id":"d1","project_id":"DEP","name":"API","version_number":"0.1","version_type":"release","date_published":"x",
                            "files":[{"url":format!("{base}/files/api.jar"),"filename":"fabric-api-0.1.jar","primary":true,"hashes":{"sha1":ds},"size":20}],
                            "dependencies":[]});
                        if NEW_API.load(std::sync::atomic::Ordering::SeqCst) {
                            let mut v2 = v1.clone();
                            v2["id"] = json!("d2");
                            v2["files"][0]["filename"] = json!("fabric-api-0.2.jar");
                            Json(json!([v2, v1]))
                        } else {
                            Json(json!([v1]))
                        }
                    }
                }
            }))
            .route("/files/{name}", get(move |P(n): P<String>| {
                let (m, d) = (main_jar.clone(), dep_jar.clone());
                async move { if n == "glow.jar" { m } else { d } }
            }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        std::env::set_var("CLAUDE_LAUNCHER_MODRINTH_API", &base);

        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(dir.path())).await.unwrap();
        // reqwest would otherwise send 127.0.0.1 through the sandbox proxy
        let l = Launcher(std::sync::Arc::new(crate::Inner {
            paths: l.paths.clone(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            events: l.events.clone(),
            config: tokio::sync::RwLock::new(Default::default()),
            processes: Default::default(),
            install_lock: tokio::sync::Mutex::new(()),
        }));
        let inst = instances::create(&l, instances::NewInstance {
            name: Some("t".into()), mc_version: "1.21.1".into(), loader: Loader::Fabric,
            loader_version: Some("0.17.2".into()), ram_mb: None,
        }).await.unwrap();

        let r = install(&l, &inst.id, InstallRequest { source: "modrinth".into(), project_id: "MAIN".into(), version_id: None }).await.unwrap();
        assert_eq!(r.installed, vec!["glow-1.0.jar", "fabric-api-0.1.jar"]);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert!(l.paths.mods_dir(&inst.id).join("glow-1.0.jar").exists());

        let tags = load_tags(&l, &inst.id).await;
        assert!(!tags["glow-1.0.jar"].as_dependency);
        assert!(tags["fabric-api-0.1.jar"].as_dependency);

        let listed = mods::list(&l, &inst.id).await.unwrap();
        assert!(listed.iter().all(|m| m.online.is_some()));

        let again = install(&l, &inst.id, InstallRequest { source: "modrinth".into(), project_id: "MAIN".into(), version_id: None }).await.unwrap();
        assert!(again.installed.is_empty());
        assert_eq!(again.already_installed, vec!["Glow Mod"]);
        assert!(tags["glow-1.0.jar"].download_url.as_deref().unwrap().ends_with("/files/glow.jar"));

        // A new release of the dependency appears; update it.
        NEW_API.store(true, std::sync::atomic::Ordering::SeqCst);
        let check = check_updates(&l, &inst.id).await.unwrap();
        assert_eq!(check.updates.len(), 1, "{:?}", check);
        assert_eq!(check.updates[0].latest.version_id, "d2");
        let r = apply_updates(&l, &inst.id, None).await.unwrap();
        assert_eq!(r.installed, vec!["fabric-api-0.2.jar"]);
        let dir = l.paths.mods_dir(&inst.id);
        assert!(dir.join("fabric-api-0.2.jar").exists() && !dir.join("fabric-api-0.1.jar").exists());
        let tags = load_tags(&l, &inst.id).await;
        assert!(tags["fabric-api-0.2.jar"].as_dependency, "dependency flag kept");
        assert!(check_updates(&l, &inst.id).await.unwrap().updates.is_empty());
    }

    static NEW_API: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    fn ver(id: &str, ch: &str) -> ModVersion {
        ModVersion {
            source: Source::Modrinth, project_id: "p".into(), version_id: id.into(), name: id.into(),
            version_number: id.into(), date: String::new(), channel: ch.into(),
            file: ModFile { url: None, filename: format!("{id}.jar"), sha1: None, size: 0 }, dependencies: vec![],
        }
    }

    #[test]
    fn update_decisions() {
        // newest first
        let vs = vec![ver("3b", "beta"), ver("2", "release"), ver("1", "release")];
        assert_eq!(newer_version(&vs, "1").unwrap().version_id, "2");
        assert!(newer_version(&vs, "2").is_none());
        assert!(newer_version(&vs, "3b").is_none(), "never downgrade a beta to an older release");
        assert_eq!(newer_version(&vs, "gone").unwrap().version_id, "2");
    }

    #[test]
    fn curseforge_parsing() {
        let s = json!({"data":[{"id":238222,"name":"JEI","slug":"jei","summary":"Items","downloadCount":3.0e8,"authors":[{"name":"mezz"}],
            "logo":{"thumbnailUrl":"https://cf/t.png"},"links":{"websiteUrl":"https://www.curseforge.com/minecraft/mc-mods/jei"},"categories":[{"name":"API"}]}],
            "pagination":{"totalCount":1}});
        let p = parse_cf_search(&s, 0);
        assert_eq!(p.hits[0].project_id, "238222");
        assert_eq!(p.hits[0].downloads, 300_000_000);

        let f = json!({"data":[
          {"id":2,"modId":238222,"displayName":"jei-2","fileName":"jei-2.jar","fileDate":"2026-03-01","releaseType":1,"downloadUrl":null,
           "hashes":[{"value":"md5x","algo":2},{"value":"sha1x","algo":1}],"fileLength":100,
           "dependencies":[{"modId":306612,"relationType":3},{"modId":1,"relationType":1}]},
          {"id":1,"modId":238222,"displayName":"jei-1","fileName":"jei-1.jar","fileDate":"2026-01-01","releaseType":2,"downloadUrl":"https://edge/jei-1.jar","hashes":[],"fileLength":90}
        ]});
        let vs = parse_cf_files(&f, "238222");
        assert_eq!(vs[0].version_id, "2");
        assert_eq!(vs[0].file.sha1.as_deref(), Some("sha1x"));
        assert!(vs[0].file.url.is_none());
        assert_eq!(vs[0].dependencies.len(), 1);
        assert_eq!(vs[1].channel, "beta");
    }
}
