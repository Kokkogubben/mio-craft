//! Mod projects: source folders that Claude Code edits, built with the
//! launcher's own JDK + Gradle and installed into an instance.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::loaders::{self, Loader};
use crate::templates::{self, ModSpec};
use crate::{http, instances, java, maven, mods, mojang, process, Event, Launcher};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModProject {
    /// Same as the mod id.
    pub id: String,
    pub name: String,
    pub description: String,
    pub instance_id: String,
    pub loader: Loader,
    pub mc_version: String,
    pub loader_version: String,
    pub path: PathBuf,
    pub toolchain: templates::Toolchain,
    pub created_at: u64,
    #[serde(default)]
    pub last_build: Option<BuildSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSummary {
    pub success: bool,
    pub at: u64,
    pub jar: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewProject {
    pub instance_id: String,
    pub mod_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildResult {
    pub success: bool,
    pub jar: Option<String>,
    /// Compiler errors and Gradle failure lines, most useful first.
    pub errors: Vec<String>,
    /// The last lines of Gradle's output.
    pub output_tail: String,
    pub installed_as: Option<String>,
}

const META_FILE: &str = "launcher-project.json";

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

pub async fn list(l: &Launcher) -> Result<Vec<ModProject>> {
    let mut out = vec![];
    let mut rd = tokio::fs::read_dir(l.paths.projects()).await?;
    while let Some(e) = rd.next_entry().await? {
        if let Ok(p) = http::read_json::<ModProject>(&e.path().join(META_FILE)).await {
            out.push(p);
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

pub async fn get(l: &Launcher, id: &str) -> Result<ModProject> {
    let dir = crate::paths::safe_join(&l.paths.projects(), id)?;
    http::read_json(&dir.join(META_FILE))
        .await
        .map_err(|_| anyhow!("no mod project with id {id:?}"))
}

async fn save(p: &ModProject) -> Result<()> {
    http::write_json(&p.path.join(META_FILE), p).await
}

async fn latest_plugin(l: &Launcher, loader: Loader, mc: &str) -> Option<String> {
    let url = match loader {
        Loader::Fabric if templates::is_unobfuscated(mc) => {
            "https://maven.fabricmc.net/net/fabricmc/fabric-loom/maven-metadata.xml"
        }
        Loader::Neoforge => {
            "https://maven.neoforged.net/releases/net/neoforged/moddev/net.neoforged.moddev.gradle.plugin/maven-metadata.xml"
        }
        _ => return None,
    };
    let xml = http::get_text(&l.http, url).await.ok()?;
    maven::metadata_versions(&xml)
        .into_iter()
        .filter(|v| !v.contains("SNAPSHOT") && !v.contains("beta") && !v.contains("alpha") && !v.contains("rc"))
        .max_by(|a, b| maven::cmp_versions(a, b))
}

pub async fn create(l: &Launcher, req: NewProject) -> Result<ModProject> {
    let inst = instances::get(l, &req.instance_id).await?;
    let mod_id = templates::normalize_mod_id(&req.mod_id)?;
    let dir = l.paths.projects().join(&mod_id);
    if dir.exists() {
        bail!("a mod project called {mod_id} already exists; pick another mod id");
    }
    let loader_version = inst
        .loader_version
        .clone()
        .ok_or_else(|| anyhow!("this instance has no mod loader; create a Fabric or NeoForge instance"))?;
    let vanilla = mojang::fetch_version_json(l, &inst.mc_version).await?;
    let java_target = java::required_major(&vanilla);
    let plugin = latest_plugin(l, inst.loader, &inst.mc_version).await;
    let toolchain = templates::toolchain(inst.loader, &inst.mc_version, java_target, plugin)?;
    let fabric_api_version = if inst.loader == Loader::Fabric {
        loaders::fabric_api_version(l, &inst.mc_version).await.ok().flatten()
    } else {
        None
    };
    let name = req.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| mod_id.replace('_', " "));
    let spec = ModSpec {
        mod_id: mod_id.clone(),
        name: name.clone(),
        description: req.description.clone().unwrap_or_default(),
        author: req.author.unwrap_or_else(|| "Claude Launcher user".into()),
        mc_version: inst.mc_version.clone(),
        loader: inst.loader,
        loader_version: loader_version.clone(),
        fabric_api_version,
        toolchain: toolchain.clone(),
    };
    for (rel, contents) in templates::render(&spec) {
        let path = crate::paths::safe_join(&dir, &rel)?;
        if let Some(p) = path.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        tokio::fs::write(&path, contents).await?;
    }
    let project = ModProject {
        id: mod_id,
        name,
        description: req.description.unwrap_or_default(),
        instance_id: inst.id,
        loader: inst.loader,
        mc_version: inst.mc_version,
        loader_version,
        path: dir,
        toolchain,
        created_at: now(),
        last_build: None,
    };
    save(&project).await?;
    l.emit(Event::ProjectsChanged);
    Ok(project)
}

/// A Gradle command for the project with the right JDKs wired in.
async fn gradle_command(l: &Launcher, p: &ModProject, tasks: &[&str]) -> Result<tokio::process::Command> {
    let target_jdk = java::ensure_jdk(l, p.toolchain.java_target).await?;
    let gradle_jdk = java::ensure_jdk(l, p.toolchain.gradle_jdk).await?;
    let gradle = crate::gradle::ensure_gradle(l, &p.toolchain.gradle_version).await?;
    let mut cmd = tokio::process::Command::new(crate::gradle::gradle_bin(&gradle));
    cmd.current_dir(&p.path)
        .env("JAVA_HOME", &gradle_jdk)
        .env("GRADLE_USER_HOME", l.paths.gradle().join("home"))
        .arg("--console=plain")
        .arg(format!("-Porg.gradle.java.installations.paths={},{}", target_jdk.display(), gradle_jdk.display()))
        .arg("-Porg.gradle.java.installations.auto-download=false")
        .args(tasks);
    Ok(cmd)
}

pub fn is_error_line(line: &str) -> bool {
    let t = line.trim_start();
    t.contains(": error:")
        || t.starts_with("error:")
        || t.starts_with("FAILURE:")
        || t.starts_with("* What went wrong")
        || (t.starts_with("> ") && !t.starts_with("> Task") && !t.starts_with("> Configure"))
        || t.starts_with("e: ")
        || t.contains("Could not resolve")
        || t.contains("Could not find")
}

pub fn find_jar(libs: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(libs).ok()?;
    rd.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.ends_with(".jar") && !n.ends_with("-sources.jar") && !n.ends_with("-dev.jar") && !n.ends_with("-javadoc.jar")
        })
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

pub async fn build(l: &Launcher, id: &str) -> Result<BuildResult> {
    let mut p = get(l, id).await?;
    let source = format!("build:{id}");
    if l.processes.is_running(&format!("dev:{id}")).await {
        // Gradle can't rebuild jars a running dev client has open on Windows.
        bail!("stop the dev client (stop_dev_client) before building");
    }
    let libs = p.path.join("build").join("libs");
    let _ = tokio::fs::remove_dir_all(&libs).await;
    let mut cmd = gradle_command(l, &p, &["build", "-x", "test"]).await?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null()).kill_on_drop(true);
    process::hide_window(&mut cmd);
    l.emit(Event::ProcessStarted { source: source.clone() });
    let mut child = cmd.spawn().context("starting Gradle")?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for r in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });
    }
    drop(tx);
    let mut all: Vec<String> = vec![];
    let collect = async {
        while let Some(line) = rx.recv().await {
            l.emit(Event::Log { source: source.clone(), line: line.clone() });
            all.push(line);
        }
    };
    let status = tokio::time::timeout(std::time::Duration::from_secs(30 * 60), async {
        collect.await;
        child.wait().await
    })
    .await
    .map_err(|_| anyhow!("the build took longer than 30 minutes and was stopped"))??;
    l.emit(Event::ProcessExited { source: source.clone(), code: status.code() });

    let _ = tokio::fs::write(p.path.join("build").join("launcher-build.log"), all.join("\n")).await;
    let jar = if status.success() { find_jar(&libs) } else { None };
    let success = status.success() && jar.is_some();
    let errors: Vec<String> = all.iter().filter(|l| is_error_line(l)).take(60).cloned().collect();
    let tail_start = all.len().saturating_sub(80);
    let result = BuildResult {
        success,
        jar: jar.as_ref().map(|j| j.to_string_lossy().to_string()),
        errors,
        output_tail: all[tail_start..].join("\n"),
        installed_as: None,
    };
    p.last_build = Some(BuildSummary { success, at: now(), jar: result.jar.clone() });
    save(&p).await?;
    l.emit(Event::BuildFinished {
        project_id: id.into(),
        success,
        message: if success { "Build succeeded".into() } else { "Build failed".into() },
    });
    l.emit(Event::ProjectsChanged);
    Ok(result)
}

/// Build, then copy the jar into the project's instance (tagged as Claude-made).
pub async fn install(l: &Launcher, id: &str, instance_id: Option<&str>) -> Result<BuildResult> {
    let p = get(l, id).await?;
    let target = instance_id.unwrap_or(&p.instance_id).to_string();
    let target_inst = instances::get(l, &target).await?;
    if target_inst.loader != p.loader || target_inst.mc_version != p.mc_version {
        bail!(
            "{} is a {} {} instance but this mod is for {} {}",
            target_inst.name,
            target_inst.loader.as_str(),
            target_inst.mc_version,
            p.loader.as_str(),
            p.mc_version
        );
    }
    if l.processes.is_running(&crate::launch::game_source(&target)).await {
        bail!("the game is running in {}; stop it (stop_game) before installing", target_inst.name);
    }
    let mut r = build(l, id).await?;
    if r.success {
        let jar = PathBuf::from(r.jar.as_ref().unwrap());
        r.installed_as = Some(mods::install_jar(l, &target, &jar, Some(id)).await?);
    }
    Ok(r)
}

pub fn dev_source(id: &str) -> String {
    format!("dev:{id}")
}

/// Start the loader's development client (`gradle runClient`). It runs
/// offline with a dev username, which is the standard way to test mods.
pub async fn run_dev(l: &Launcher, id: &str) -> Result<()> {
    let p = get(l, id).await?;
    let cmd = gradle_command(l, &p, &["runClient"]).await?;
    let log = p.path.join("run").join("launcher-dev.log");
    process::spawn_tracked(l, &dev_source(id), cmd, log, vec![]).await
}

pub async fn dev_log(l: &Launcher, id: &str, lines: usize) -> Result<String> {
    let p = get(l, id).await?;
    let latest = p.path.join("run").join("logs").join("latest.log");
    let launcher = p.path.join("run").join("launcher-dev.log");
    let path = if latest.exists()
        && std::fs::metadata(&latest).and_then(|m| m.modified()).ok()
            >= std::fs::metadata(&launcher).and_then(|m| m.modified()).ok()
    {
        latest
    } else {
        launcher
    };
    process::tail_file(&path, lines)
        .await
        .map_err(|_| anyhow!("no dev client log yet; start one with run_dev_client"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn errors() {
        assert!(super::is_error_line("/x/A.java:12: error: cannot find symbol"));
        assert!(super::is_error_line("FAILURE: Build failed with an exception."));
        assert!(super::is_error_line("> Compilation failed; see the compiler error output for details."));
        assert!(!super::is_error_line("> Task :compileJava"));
        assert!(!super::is_error_line("BUILD SUCCESSFUL in 3s"));
    }
}
