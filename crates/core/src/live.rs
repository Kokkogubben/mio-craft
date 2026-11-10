//! Live mode: change a running game without restarting it.
//!
//! - **Hotswap** (route 2): the game runs on JetBrains Runtime with enhanced
//!   class redefinition, Mixin's hot-swap agent, and our own agent
//!   (`live/agent/LiveAgent.java`) that accepts recompiled classes.
//! - **Content slots** (route 3), **in-memory textures** and **registry
//!   unfreezing** (route 4) are done by the companion mod
//!   (`live/companion`), configured through system properties and
//!   `<game dir>/claude-live/content.json` (see `live_content`).
//!
//! Only Fabric on unobfuscated Minecraft (26.x) is supported for now.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::instances::Instance;
use crate::loaders::Loader;
use crate::mojang::VersionJson;
use crate::{java, maven, templates, Launcher};

pub const AGENT_SOURCE: &str = include_str!("../live/agent/LiveAgent.java");
const AGENT_MANIFEST: &str = "Manifest-Version: 1.0\nPremain-Class: dev.claudelauncher.agent.LiveAgent\nAgent-Class: dev.claudelauncher.agent.LiveAgent\nCan-Redefine-Classes: true\nCan-Retransform-Classes: true\n";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LiveSettings {
    /// Route 2: push recompiled code into the running game.
    pub hotswap: bool,
    /// Route 3: pre-registered blank blocks/items that get filled in live.
    pub content_slots: bool,
    /// Paint textures straight into the texture atlas instead of reloading resources.
    pub in_memory_textures: bool,
    /// Route 4 (experimental): register real content after startup.
    pub unfreeze_registries: bool,
    /// Blocks and items each (only used with content_slots).
    pub slot_count: u32,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self { hotswap: false, content_slots: false, in_memory_textures: false, unfreeze_registries: false, slot_count: 64 }
    }
}

impl LiveSettings {
    pub fn any(&self) -> bool {
        self.hotswap || self.content_slots || self.unfreeze_registries
    }
    pub fn needs_companion(&self) -> bool {
        self.content_slots || self.unfreeze_registries
    }
    /// Drop combinations that don't make sense (textures need the companion mod).
    pub fn normalized(mut self) -> Self {
        if !self.needs_companion() {
            self.in_memory_textures = false;
        }
        self.slot_count = self.slot_count.clamp(1, 256);
        self
    }
}

/// Why live mode can't be used for this instance, if it can't.
pub fn unsupported_reason(inst: &Instance) -> Option<String> {
    if inst.loader != Loader::Fabric {
        return Some("Live mode works with Fabric instances for now.".into());
    }
    if !templates::is_unobfuscated(&inst.mc_version) {
        return Some(format!(
            "Live mode needs Minecraft 26.1 or newer (unobfuscated); this instance is {}.",
            inst.mc_version
        ));
    }
    None
}

pub fn live_dir(l: &Launcher, inst: &Instance) -> PathBuf {
    l.paths.game_dir(&inst.id).join("claude-live")
}

pub fn agent_json(l: &Launcher, instance_id: &str) -> PathBuf {
    l.paths.game_dir(instance_id).join("claude-live").join("agent.json")
}

/// Build the agent jar from the embedded source (once per source version).
pub async fn ensure_agent(l: &Launcher, jdk_home: &Path) -> Result<PathBuf> {
    let dir = l.paths.root.join("live");
    let jar = dir.join("claude-live-agent.jar");
    let hash_file = dir.join("claude-live-agent.hash");
    let hash = {
        use sha1::Digest;
        hex::encode(sha1::Sha1::digest(AGENT_SOURCE.as_bytes()))
    };
    if jar.exists() && tokio::fs::read_to_string(&hash_file).await.ok().as_deref() == Some(hash.as_str()) {
        return Ok(jar);
    }
    let work = dir.join("agent-build");
    let _ = tokio::fs::remove_dir_all(&work).await;
    let src = work.join("src/dev/claudelauncher/agent/LiveAgent.java");
    tokio::fs::create_dir_all(src.parent().unwrap()).await?;
    tokio::fs::create_dir_all(work.join("classes")).await?;
    tokio::fs::write(&src, AGENT_SOURCE).await?;
    tokio::fs::write(work.join("MANIFEST.MF"), AGENT_MANIFEST).await?;
    let bin = jdk_home.join("bin");
    let run = |exe: &str, args: Vec<std::ffi::OsString>| {
        let mut cmd = tokio::process::Command::new(bin.join(crate::platform::exe(exe)));
        cmd.args(args).current_dir(&work).env_remove("JAVA_TOOL_OPTIONS");
        crate::process::hide_window(&mut cmd);
        cmd
    };
    let out = run("javac", vec!["--release".into(), "17".into(), "-d".into(), "classes".into(), src.clone().into()])
        .output()
        .await
        .context("compiling the live agent")?;
    if !out.status.success() {
        bail!("couldn't compile the live agent: {}", String::from_utf8_lossy(&out.stderr));
    }
    let tmp_jar = dir.join("claude-live-agent.jar.tmp");
    let out = run("jar", vec!["cfm".into(), tmp_jar.clone().into(), "MANIFEST.MF".into(), "-C".into(), "classes".into(), ".".into()])
        .output()
        .await
        .context("packaging the live agent")?;
    if !out.status.success() {
        bail!("couldn't package the live agent: {}", String::from_utf8_lossy(&out.stderr));
    }
    tokio::fs::rename(&tmp_jar, &jar).await?;
    tokio::fs::write(&hash_file, &hash).await?;
    let _ = tokio::fs::remove_dir_all(&work).await;
    Ok(jar)
}

/// Mixin's jar from the version's libraries (it doubles as Mixin's hot-swap agent).
pub fn mixin_jar(l: &Launcher, v: &VersionJson) -> Option<PathBuf> {
    v.libraries
        .iter()
        .find(|lib| lib.name.starts_with("net.fabricmc:sponge-mixin:"))
        .and_then(|lib| maven::coord_to_path(&lib.name).ok())
        .map(|rel| l.paths.libraries().join(rel))
        .filter(|p| p.exists())
}

pub struct LivePlan {
    /// Java to launch with (JetBrains Runtime when hotswap is on).
    pub java_home: Option<PathBuf>,
    pub jvm_args: Vec<String>,
}

/// Extra JVM arguments (and Java) for a launch with live mode.
pub async fn plan(l: &Launcher, inst: &Instance, v: &VersionJson) -> Result<LivePlan> {
    let s = inst.live.clone().normalized();
    let mut plan = LivePlan { java_home: None, jvm_args: vec![] };
    if !s.any() {
        return Ok(plan);
    }
    if let Some(why) = unsupported_reason(inst) {
        bail!("{why} Turn live mode off in the instance's settings to play.");
    }
    let dir = live_dir(l, inst);
    tokio::fs::create_dir_all(&dir).await?;
    let _ = tokio::fs::remove_file(dir.join("agent.json")).await;
    let props = |k: &str, v: String| format!("-Dclaudelauncher.live.{k}={v}");
    plan.jvm_args.push(props("dir", dir.to_string_lossy().into()));
    if s.hotswap {
        let major = java::required_major(v);
        let jbr = java::ensure_jbr(l, major).await?;
        let agent = ensure_agent(l, &jbr).await?;
        plan.jvm_args.push("-XX:+AllowEnhancedClassRedefinition".into());
        plan.jvm_args.push(format!("-javaagent:{}={}", agent.display(), dir.join("agent.json").display()));
        match mixin_jar(l, v) {
            Some(m) => {
                plan.jvm_args.push(format!("-javaagent:{}", m.display()));
                plan.jvm_args.push("-Dmixin.hotSwap=true".into());
            }
            None => l.progress("live", 0, 0, "Mixin jar not found; Mixin changes will need a restart"),
        }
        plan.java_home = Some(jbr);
    }
    if s.content_slots {
        plan.jvm_args.push(props("slots", s.slot_count.to_string()));
    }
    plan.jvm_args.push(props("inMemoryTextures", s.in_memory_textures.to_string()));
    plan.jvm_args.push(props("unfreeze", s.unfreeze_registries.to_string()));
    Ok(plan)
}

// ---------- talking to the agent ----------

#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub pid: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RedefineResult {
    #[serde(default)]
    pub redefined: Vec<String>,
    /// Not loaded yet; the new version is used when they load.
    #[serde(default)]
    pub deferred: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    /// Some change can't be applied live (e.g. without enhanced redefinition).
    #[serde(default)]
    pub needs_restart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

async fn write_utf(w: &mut (impl AsyncWriteExt + Unpin), s: &str) -> Result<()> {
    let b = s.as_bytes();
    if b.len() > u16::MAX as usize {
        bail!("string too long for the agent protocol");
    }
    w.write_u16(b.len() as u16).await?;
    w.write_all(b).await?;
    Ok(())
}

async fn read_utf(r: &mut (impl AsyncReadExt + Unpin)) -> Result<String> {
    let n = r.read_u16().await? as usize;
    let mut b = vec![0; n];
    r.read_exact(&mut b).await?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

pub async fn agent_info(path: &Path) -> Result<AgentInfo> {
    let raw = tokio::fs::read(path)
        .await
        .map_err(|_| anyhow!("the game isn't running with live hotswap (no agent found). Turn on Hotswap in the instance's live mode settings and launch again."))?;
    Ok(serde_json::from_slice(&raw)?)
}

/// Send classes (binary name → bytecode) to the agent in a running game.
pub async fn redefine(agent: &Path, classes: Vec<(String, Vec<u8>)>) -> Result<RedefineResult> {
    let info = agent_info(agent).await?;
    let mut s = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", info.port)),
    )
    .await
    .map_err(|_| anyhow!("timed out reaching the game"))?
    .map_err(|_| anyhow!("couldn't reach the game's live agent; is the game still running?"))?;
    write_utf(&mut s, &info.token).await?;
    write_utf(&mut s, "REDEFINE").await?;
    s.write_i32(classes.len() as i32).await?;
    for (name, bytes) in &classes {
        write_utf(&mut s, name).await?;
        s.write_i32(bytes.len() as i32).await?;
        s.write_all(bytes).await?;
    }
    s.flush().await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(60), read_utf(&mut s))
        .await
        .map_err(|_| anyhow!("the game didn't answer the hotswap request"))??;
    let r: RedefineResult = serde_json::from_str(&reply)?;
    if let Some(e) = &r.error {
        bail!("live agent: {e}");
    }
    Ok(r)
}

/// Binary class name of a `.class` file under `classes_root`.
pub fn class_name(classes_root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(classes_root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    let s = s.strip_suffix(".class")?;
    Some(s.replace('/', "."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(loader: Loader, mc: &str) -> Instance {
        Instance {
            id: "x".into(),
            name: "x".into(),
            mc_version: mc.into(),
            loader,
            loader_version: Some("1".into()),
            version_id: None,
            ram_mb: None,
            created_at: 0,
            last_played: None,
            last_exit: None,
            crash_dismissed_at: None,
            live: LiveSettings::default(),
        }
    }

    #[test]
    fn support() {
        assert!(unsupported_reason(&inst(Loader::Fabric, "26.1")).is_none());
        assert!(unsupported_reason(&inst(Loader::Fabric, "1.21.1")).is_some());
        assert!(unsupported_reason(&inst(Loader::Neoforge, "26.1")).is_some());
        let s = LiveSettings { in_memory_textures: true, slot_count: 999, ..Default::default() }.normalized();
        assert!(!s.in_memory_textures);
        assert_eq!(s.slot_count, 256);
    }

    #[test]
    fn class_names() {
        let root = Path::new("/p/build/classes/java/main");
        assert_eq!(
            class_name(root, &root.join("com/claudemods/glow/GlowMod$1.class")).as_deref(),
            Some("com.claudemods.glow.GlowMod$1")
        );
    }

    /// End-to-end against a real JVM: build the agent, run a program that
    /// prints a value in a loop, redefine the class, and check the output
    /// changes — including for a class that only loads afterwards.
    #[tokio::test(flavor = "multi_thread")]
    async fn hotswaps_a_running_jvm() {
        let Some(javac) = crate::claude_code::find_program("javac") else {
            eprintln!("no JDK on PATH; skipping");
            return;
        };
        let jdk = std::fs::canonicalize(javac).unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(dir.path())).await.unwrap();
        let agent = ensure_agent(&l, &jdk).await.unwrap();
        // Second call is cached.
        assert_eq!(ensure_agent(&l, &jdk).await.unwrap(), agent);

        let w = dir.path().join("app");
        for d in ["src1", "src2", "v1", "v2"] {
            std::fs::create_dir_all(w.join(d)).unwrap();
        }
        std::fs::write(w.join("Main.java"), "public class Main { public static void main(String[] a) throws Exception { for (int i = 0; i < 60; i++) { System.out.println(\"greet=\" + Greeter.greet() + (i == 40 ? \" late=\" + Late.value() : \"\")); Thread.sleep(100); } } }").unwrap();
        std::fs::write(w.join("src1/Greeter.java"), "public class Greeter { public static String greet() { return \"old\"; } }").unwrap();
        std::fs::write(w.join("src1/Late.java"), "public class Late { public static String value() { return \"old-late\"; } }").unwrap();
        std::fs::write(w.join("src2/Greeter.java"), "public class Greeter { public static String greet() { return \"NEW\"; } }").unwrap();
        std::fs::write(w.join("src2/Late.java"), "public class Late { public static String value() { return \"NEW-late\"; } }").unwrap();
        let javac = jdk.join("bin/javac");
        let ok = |o: std::process::Output| assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        ok(std::process::Command::new(&javac).current_dir(&w).args(["-d", "v1", "Main.java", "src1/Greeter.java", "src1/Late.java"]).env_remove("JAVA_TOOL_OPTIONS").output().unwrap());
        ok(std::process::Command::new(&javac).current_dir(&w).args(["-d", "v2", "-cp", "v1", "src2/Greeter.java", "src2/Late.java"]).env_remove("JAVA_TOOL_OPTIONS").output().unwrap());

        let agent_json = dir.path().join("agent.json");
        let mut child = tokio::process::Command::new(jdk.join("bin/java"))
            .current_dir(&w)
            .env_remove("JAVA_TOOL_OPTIONS")
            .arg(format!("-javaagent:{}={}", agent.display(), agent_json.display()))
            .args(["-cp", "v1", "Main"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if agent_json.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let classes = vec![
            ("Greeter".to_string(), std::fs::read(w.join("v2/Greeter.class")).unwrap()),
            ("Late".to_string(), std::fs::read(w.join("v2/Late.class")).unwrap()),
        ];
        let r = redefine(&agent_json, classes).await.unwrap();
        assert_eq!(r.redefined, vec!["Greeter"]);
        assert_eq!(r.deferred, vec!["Late"]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);

        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
        child.wait().await.unwrap();
        assert!(out.contains("greet=old"), "{out}");
        assert!(out.contains("greet=NEW"), "{out}");
        assert!(out.contains("late=NEW-late"), "deferred class used new bytes: {out}");
        assert!(!out.contains("late=old-late"));
    }
}
