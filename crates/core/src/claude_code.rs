//! Working with the user's Claude Code install: find it, register our MCP
//! server with it, and open it in a terminal with a ready-made prompt.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use crate::instances::Instance;
use crate::loaders::Loader;
use crate::projects::ModProject;
use crate::Launcher;

pub const MCP_NAME: &str = "claude-launcher";
const MCP_SCRIPT: &str = "claude-launcher-mcp.mjs";

/// Find an executable on PATH (and Claude Code's usual install folders).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local").join("bin")); // Claude Code's native installer
        dirs.push(home.join(".claude").join("local")); // older local installs
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm")); // npm global on Windows
    }
    for d in dirs {
        for ext in exts {
            let p = d.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub claude_path: Option<PathBuf>,
    pub node_path: Option<PathBuf>,
    /// Whether `claude mcp get claude-launcher` succeeds.
    pub connected: bool,
}

pub async fn status() -> Status {
    let claude = find_program("claude");
    let node = find_program("node");
    let connected = match &claude {
        Some(c) => {
            let mut cmd = tokio::process::Command::new(c);
            cmd.args(["mcp", "get", MCP_NAME]);
            crate::process::hide_window(&mut cmd);
            matches!(
                tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output()).await,
                Ok(Ok(o)) if o.status.success()
            )
        }
        None => false,
    };
    Status { claude_path: claude, node_path: node, connected }
}

/// Register (or re-register, e.g. after the launcher moved) the MCP server
/// with Claude Code at user scope.
pub async fn connect(mcp_script: &str) -> Result<()> {
    let script = Path::new(mcp_script);
    if script.file_name().and_then(|f| f.to_str()) != Some(MCP_SCRIPT) || !script.is_file() {
        bail!("that isn't the launcher's MCP server script");
    }
    let claude = find_program("claude").ok_or_else(|| {
        anyhow!("Claude Code isn't installed (or isn't on PATH). Install it from https://claude.com/claude-code, then try again.")
    })?;
    let node = find_program("node").ok_or_else(|| {
        anyhow!("The launcher's MCP server runs on Node.js, which isn't installed. Install Node.js LTS from https://nodejs.org, then try again.")
    })?;
    let run = |args: Vec<String>| {
        let claude = claude.clone();
        async move {
            let mut cmd = tokio::process::Command::new(&claude);
            cmd.args(&args);
            crate::process::hide_window(&mut cmd);
            cmd.output().await.context("running Claude Code")
        }
    };
    // Remove any old registration first; ignore "not found".
    let _ = run(vec!["mcp".into(), "remove".into(), "--scope".into(), "user".into(), MCP_NAME.into()]).await;
    let out = run(vec![
        "mcp".into(),
        "add".into(),
        "--scope".into(),
        "user".into(),
        MCP_NAME.into(),
        "--".into(),
        node.to_string_lossy().into(),
        mcp_script.into(),
    ])
    .await?;
    if !out.status.success() {
        bail!(
            "Claude Code couldn't add the server: {}{}",
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    Ok(())
}

fn loader_label(l: Loader) -> &'static str {
    match l {
        Loader::Fabric => "Fabric",
        Loader::Neoforge => "NeoForge",
        Loader::Vanilla => "vanilla",
    }
}

const NO_TOOLS: &str = "If the claude-launcher tools aren't available, tell me to open Claude Launcher and use Settings → Connect Claude Code.";

/// Prompt for a brand-new mod in an instance.
pub fn new_mod_prompt(inst: &Instance, idea: &str) -> String {
    format!(
        "I want a new Minecraft mod for my Claude Launcher instance \"{name}\" (instance id {id}: Minecraft {mc}, {loader} {lv}). \
         The idea: {idea} \
         Use the claude-launcher MCP tools: create_mod for instance {id}, read the CLAUDE.md in the project it creates, write the mod, \
         run build_mod and fix errors until it compiles, then install_mod. Test it with run_dev_client and check read_dev_log for errors, \
         then stop_dev_client and tell me what you made. {NO_TOOLS}",
        name = inst.name,
        id = inst.id,
        mc = inst.mc_version,
        loader = loader_label(inst.loader),
        lv = inst.loader_version.clone().unwrap_or_default(),
        idea = idea.trim(),
    )
}

/// Prompt for changing an existing mod project.
pub fn project_prompt(p: &ModProject, request: &str) -> String {
    format!(
        "Work on my Minecraft mod \"{name}\" (mod project id {id}, a {loader} mod for Minecraft {mc}). Its source is in {path}; read its CLAUDE.md first. \
         What I want: {req} \
         When done, run install_mod for {id} with the claude-launcher MCP tools, fix any build errors, and test with run_dev_client and read_dev_log. {NO_TOOLS}",
        name = p.name,
        id = p.id,
        loader = loader_label(p.loader),
        mc = p.mc_version,
        path = p.path.display(),
        req = request.trim(),
    )
}

/// Prompt for diagnosing (and, for Claude's own mods, fixing) a crash.
pub fn crash_prompt(inst: &Instance, c: &crate::crash::CrashInfo) -> String {
    let suspects = if c.suspects.is_empty() {
        "none identified".to_string()
    } else {
        c.suspects
            .iter()
            .map(|s| match &s.claude_project {
                Some(p) => format!("{} ({}, made by you: mod project {p})", s.name, s.file_name),
                None => format!("{} ({})", s.name, s.file_name),
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let report = c.report_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none was written".into());
    format!(
        "My Minecraft instance \"{name}\" (instance id {id}: Minecraft {mc}, {loader} {lv}) crashed. \
         Summary: {desc}{exc}. Crash report: {report}. Game log: {game}. Raw launcher output: {out}. \
         Mods that appear in the stack trace: {suspects}. \
         Read the crash report and logs (read_game_log with kind crash, game or launcher works too) and find the cause. \
         If it's in a mod you made (a claude-launcher mod project), fix it, run install_mod, and test with run_dev_client and read_dev_log. \
         If it's a third-party mod or a conflict between mods, explain the problem and suggest a fix such as update_mods or set_mod_enabled, \
         but ask me before changing third-party mods. {NO_TOOLS}",
        name = inst.name,
        id = inst.id,
        mc = inst.mc_version,
        loader = loader_label(inst.loader),
        lv = inst.loader_version.clone().unwrap_or_default(),
        desc = c.description.clone().unwrap_or_else(|| "no description".into()),
        exc = c.exception.as_ref().map(|e| format!(" ({e})")).unwrap_or_default(),
        game = c.game_log.display(),
        out = c.launcher_log.display(),
    )
}

/// Make text safe as one PowerShell argument inside single quotes, and avoid
/// Windows PowerShell 5.1 mangling double quotes when passing it to `claude`.
pub fn ps_quote(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '"' | '\u{201C}' | '\u{201D}' => '\'',
            '\r' | '\n' | '\t' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    format!("'{}'", cleaned.replace('\'', "''"))
}

/// Open Claude Code in a new terminal window in `cwd` with `prompt`.
pub async fn open(l: &Launcher, cwd: &Path, prompt: &str) -> Result<()> {
    let claude = find_program("claude").ok_or_else(|| {
        anyhow!("Claude Code isn't installed (or isn't on PATH). Install it from https://claude.com/claude-code, or copy the prompt into Claude Code yourself.")
    })?;
    tokio::fs::create_dir_all(cwd).await?;
    open_terminal(l, &claude, cwd, prompt)
}

#[cfg(windows)]
fn open_terminal(_l: &Launcher, claude: &Path, cwd: &Path, prompt: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // An npm-installed `claude.cmd` goes through cmd.exe, which expands %VAR%.
    let is_batch = claude.extension().map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")).unwrap_or(false);
    let prompt = if is_batch { prompt.replace('%', " percent") } else { prompt.to_string() };
    let script = format!(
        "Set-Location -LiteralPath {cwd}; & {claude} {prompt}",
        cwd = ps_quote(&cwd.to_string_lossy()),
        claude = ps_quote(&claude.to_string_lossy()),
        prompt = ps_quote(&prompt),
    );
    std::process::Command::new("powershell.exe")
        .args(["-NoExit", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .context("opening a terminal")?;
    Ok(())
}

#[cfg(not(windows))]
fn open_terminal(_l: &Launcher, _claude: &Path, _cwd: &Path, _prompt: &str) -> Result<()> {
    bail!("Opening Claude Code from the launcher only works on Windows so far. Copy the prompt into Claude Code instead.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst() -> Instance {
        Instance {
            id: "creative-lab".into(),
            name: "Creative Lab".into(),
            mc_version: "1.21.1".into(),
            loader: Loader::Fabric,
            loader_version: Some("0.17.2".into()),
            version_id: None,
            ram_mb: None,
            created_at: 0,
            last_played: None,
            last_exit: None,
            crash_dismissed_at: None,
        }
    }

    #[test]
    fn prompts_carry_context() {
        let p = new_mod_prompt(&inst(), "  ores glow  ");
        assert!(p.contains("instance id creative-lab"));
        assert!(p.contains("Fabric 0.17.2"));
        assert!(p.contains("The idea: ores glow "));
        assert!(p.contains("create_mod for instance creative-lab"));
    }

    #[test]
    fn crash_prompt_names_claude_mods() {
        let c = crate::crash::CrashInfo {
            instance_id: "creative-lab".into(),
            at: 0,
            exit_code: Some(-1),
            report_path: Some("C:/r/crash.txt".into()),
            description: Some("Initializing game".into()),
            exception: Some("java.lang.NullPointerException: x".into()),
            game_log: "C:/g/latest.log".into(),
            launcher_log: "C:/l/out.log".into(),
            suspects: vec![crate::crash::Suspect {
                mod_id: "glow".into(),
                name: "Glow".into(),
                file_name: "glow.jar".into(),
                claude_project: Some("glow".into()),
            }],
        };
        let p = crash_prompt(&inst(), &c);
        assert!(p.contains("Initializing game (java.lang.NullPointerException: x)"));
        assert!(p.contains("made by you: mod project glow"));
        assert!(p.contains("ask me before changing third-party mods"));
        assert!(p.contains("C:/r/crash.txt"));
    }

    #[test]
    fn quoting() {
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(ps_quote("say \"hi\"\nnow"), "'say ''hi'' now'");
        assert_eq!(ps_quote("$(rm -rf) `x` ; & |"), "'$(rm -rf) `x` ; & |'");
    }
}
