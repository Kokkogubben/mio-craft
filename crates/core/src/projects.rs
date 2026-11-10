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
    /// Instance to reinstall the jar into when its game exits (after a hotswap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_install: Option<String>,
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
        pending_install: None,
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
    let (status, all) = run_gradle(l, &p, &["build", "-x", "test"], &source).await?;

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

/// Run Gradle tasks, streaming output under `source`. Returns the exit status and all lines.
async fn run_gradle(l: &Launcher, p: &ModProject, tasks: &[&str], source: &str) -> Result<(std::process::ExitStatus, Vec<String>)> {
    let source = source.to_string();
    let mut cmd = gradle_command(l, p, tasks).await?;
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
    Ok((status, all))
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

// ---------- live hotswap ----------

#[derive(Debug, Clone, Serialize)]
pub struct HotswapResult {
    pub compiled: bool,
    /// Compiler errors when `compiled` is false.
    pub errors: Vec<String>,
    /// Classes whose bytecode differed from what the game has.
    pub changed: Vec<String>,
    pub agent: Option<crate::live::RedefineResult>,
    /// The jar will be reinstalled into the instance when the game exits,
    /// so the change survives a restart.
    pub install_queued: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HotswapState {
    agent_token: String,
    /// Binary class name → sha1 of what the running game has.
    sent: std::collections::HashMap<String, String>,
}

fn sha1_hex(b: &[u8]) -> String {
    use sha1::Digest;
    hex::encode(sha1::Sha1::digest(b))
}

/// Class name → sha1 for every class in a jar.
fn jar_class_hashes(jar: &Path) -> std::collections::HashMap<String, String> {
    use std::io::Read;
    let mut out = std::collections::HashMap::new();
    let Ok(f) = std::fs::File::open(jar) else { return out };
    let Ok(mut z) = zip::ZipArchive::new(f) else { return out };
    for i in 0..z.len() {
        let Ok(mut e) = z.by_index(i) else { continue };
        let name = e.name().to_string();
        if let Some(c) = name.strip_suffix(".class") {
            let mut b = vec![];
            if e.read_to_end(&mut b).is_ok() {
                out.insert(c.replace('/', "."), sha1_hex(&b));
            }
        }
    }
    out
}

fn walk_classes(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "class").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

/// Compile a project and push changed classes into its running game.
pub async fn hotswap(l: &Launcher, id: &str) -> Result<HotswapResult> {
    let mut p = get(l, id).await?;
    let inst = instances::get(l, &p.instance_id).await?;
    if !l.processes.is_running(&crate::launch::game_source(&inst.id)).await {
        bail!("{} isn't running. Launch it with live mode Hotswap on, then try again (or use install_mod while it's closed).", inst.name);
    }
    let agent_json = crate::live::agent_json(l, &inst.id);
    let info = crate::live::agent_info(&agent_json).await?;

    let (status, lines) = run_gradle(l, &p, &["classes", "-x", "test"], &format!("build:{id}")).await?;
    let mut result = HotswapResult { compiled: status.success(), errors: vec![], changed: vec![], agent: None, install_queued: false, notes: vec![] };
    if !status.success() {
        result.errors = lines.iter().filter(|x| is_error_line(x)).take(60).cloned().collect();
        return Ok(result);
    }

    // What the game has: the jar it loaded, plus anything we've sent since.
    let state_file = p.path.join("build").join("launcher-hotswap.json");
    let mut state: HotswapState = http::read_json(&state_file).await.unwrap_or_default();
    if state.agent_token != info.token {
        state = HotswapState { agent_token: info.token.clone(), sent: Default::default() };
    }
    let tags = mods::load_tags(l, &inst.id).await;
    let installed_jar = tags
        .files
        .iter()
        .find(|(_, pid)| pid.as_str() == id)
        .map(|(f, _)| l.paths.mods_dir(&inst.id).join(f));
    let classes_root = p.path.join("build").join("classes").join("java").join("main");
    let (root, jar, sent) = (classes_root.clone(), installed_jar.clone(), state.sent.clone());
    let changed: Vec<(String, Vec<u8>, String)> = tokio::task::spawn_blocking(move || {
        let base = jar.as_deref().map(jar_class_hashes).unwrap_or_default();
        walk_classes(&root)
            .into_iter()
            .filter_map(|f| {
                let name = crate::live::class_name(&root, &f)?;
                let bytes = std::fs::read(&f).ok()?;
                let h = sha1_hex(&bytes);
                Some((name, bytes, h))
            })
            .filter(|(name, _, h)| sent.get(name).or(base.get(name)) != Some(h))
            .collect::<Vec<_>>()
    })
    .await?;
    if installed_jar.is_none() {
        result.notes.push("This mod wasn't installed in the instance when the game started, so its classes only take effect if the game already loads them. Use install_mod and relaunch for a new mod.".into());
    }
    if changed.is_empty() {
        result.notes.push("No code changes to send.".into());
        return Ok(result);
    }
    result.changed = changed.iter().map(|c| c.0.clone()).collect();
    let r = crate::live::redefine(&agent_json, changed.iter().map(|c| (c.0.clone(), c.1.clone())).collect()).await?;
    let failed: std::collections::HashSet<String> =
        r.errors.iter().filter_map(|e| e.split(':').next().map(str::to_string)).collect();
    for (name, _, h) in &changed {
        if !failed.contains(name) {
            state.sent.insert(name.clone(), h.clone());
        }
    }
    http::write_json(&state_file, &state).await?;
    if r.needs_restart {
        result.notes.push("Some changes can't be applied live; they'll take effect after a restart.".into());
    }
    result.notes.push("Changed resources (textures, models, lang, data) aren't hotswapped: use F3+T or /reload in game.".into());
    result.agent = Some(r);
    p.pending_install = Some(inst.id.clone());
    save(&p).await?;
    result.install_queued = true;
    Ok(result)
}

/// Install jars of projects that were hotswapped into this instance (called when its game exits).
pub async fn install_pending(l: &Launcher, instance_id: &str) {
    let Ok(list) = list(l).await else { return };
    for mut p in list.into_iter().filter(|p| p.pending_install.as_deref() == Some(instance_id)) {
        p.pending_install = None;
        let _ = save(&p).await;
        match install(l, &p.id, Some(instance_id)).await {
            Ok(r) if r.success => l.emit(Event::BuildFinished { project_id: p.id.clone(), success: true, message: "Live changes saved to the mod jar".into() }),
            Ok(_) | Err(_) => l.emit(Event::BuildFinished { project_id: p.id.clone(), success: false, message: "Couldn't save live changes to the mod jar; run install_mod".into() }),
        }
    }
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
