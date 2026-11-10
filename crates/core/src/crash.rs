//! Crash detection and summaries, for the launcher's crash banner and the
//! "Ask Claude to fix" prompt.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::instances::{self, Instance};
use crate::{mods, Launcher};

#[derive(Debug, Clone, Serialize)]
pub struct CrashInfo {
    pub instance_id: String,
    /// When the crash happened (unix seconds).
    pub at: u64,
    pub exit_code: Option<i32>,
    /// Newest crash report written during the last session, if any.
    pub report_path: Option<PathBuf>,
    /// The report's "Description:" line, e.g. "Initializing game".
    pub description: Option<String>,
    /// The first exception line, e.g. "java.lang.NullPointerException: …".
    pub exception: Option<String>,
    pub game_log: PathBuf,
    pub launcher_log: PathBuf,
    /// Installed mods whose id or package shows up in the stack trace.
    pub suspects: Vec<Suspect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suspect {
    pub mod_id: String,
    pub name: String,
    pub file_name: String,
    pub claude_project: Option<String>,
}

fn mtime(p: &Path) -> Option<u64> {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn newest_report(dir: &Path) -> Option<(PathBuf, u64)> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
        .filter_map(|p| mtime(&p).map(|t| (p, t)))
        .max_by_key(|(_, t)| *t)
}

/// Pull the description and the first exception out of a crash report or log.
pub fn summarize(text: &str) -> (Option<String>, Option<String>) {
    let description = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Description:"))
        .map(|d| d.trim().to_string());
    let exception = text
        .lines()
        .map(str::trim)
        .find(|l| {
            let head = l.split(':').next().unwrap_or("");
            (head.ends_with("Exception") || head.ends_with("Error"))
                && head.contains('.')
                && !head.contains(' ')
        })
        .map(|l| l.chars().take(300).collect());
    (description, exception)
}

/// Mods that appear in stack frames (`at <package>…`) or are named in loader
/// "suspected mod" lines.
pub fn find_suspects(text: &str, installed: &[mods::ModEntry]) -> Vec<Suspect> {
    let frames: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("at ") || l.contains("Suspected Mod") || l.contains("Mod File:") || l.contains("mod '"))
        .collect();
    let joined = frames.join("\n").to_lowercase();
    let mut out = vec![];
    for m in installed.iter().filter(|m| m.enabled) {
        let Some(id) = m.mod_id.as_deref() else { continue };
        if matches!(id, "minecraft" | "java" | "fabricloader" | "neoforge" | "fabric-api" | "fabric") || id.len() < 3 {
            continue;
        }
        let id_l = id.to_lowercase();
        let hit = joined.contains(&format!(".{id_l}.")) // package segment
            || joined.contains(&format!("({id_l})"))   // "Suspected Mod: Name (id)"
            || joined.contains(&format!("'{id_l}'"))
            || joined.contains(&m.file_name.to_lowercase());
        if hit {
            out.push(Suspect {
                mod_id: id.into(),
                name: m.name.clone().unwrap_or_else(|| id.into()),
                file_name: m.file_name.clone(),
                claude_project: m.claude_project.clone(),
            });
        }
    }
    out
}

/// The instance's most recent crash, unless it was dismissed or the game has
/// run cleanly since.
pub async fn latest(l: &Launcher, inst: &Instance) -> Result<Option<CrashInfo>> {
    let Some(started) = inst.last_played else { return Ok(None) };
    let game = l.paths.game_dir(&inst.id);
    let report = newest_report(&game.join("crash-reports")).filter(|(_, t)| *t + 5 >= started);
    let bad_exit = inst.last_exit.as_ref().filter(|e| e.code.map(|c| c != 0).unwrap_or(false));
    if report.is_none() && bad_exit.is_none() {
        return Ok(None);
    }
    let at = report.as_ref().map(|r| r.1).or(bad_exit.map(|e| e.at)).unwrap_or(started);
    if inst.crash_dismissed_at.map(|d| d >= at).unwrap_or(false) {
        return Ok(None);
    }
    let game_log = game.join("logs").join("latest.log");
    let launcher_log = l.paths.instance_dir(&inst.id).join("launcher-output.log");
    let text = match &report {
        Some((p, _)) => tokio::fs::read_to_string(p).await.unwrap_or_default(),
        None => {
            // No report (e.g. the loader failed before Minecraft started): use the raw output.
            let a = tokio::fs::read_to_string(&launcher_log).await.unwrap_or_default();
            if a.trim().is_empty() { tokio::fs::read_to_string(&game_log).await.unwrap_or_default() } else { a }
        }
    };
    let (description, exception) = summarize(&text);
    let installed = mods::list(l, &inst.id).await.unwrap_or_default();
    Ok(Some(CrashInfo {
        instance_id: inst.id.clone(),
        at,
        exit_code: inst.last_exit.as_ref().and_then(|e| e.code),
        report_path: report.map(|r| r.0),
        description,
        exception,
        game_log,
        launcher_log,
        suspects: find_suspects(&text, &installed),
    }))
}

pub async fn dismiss(l: &Launcher, instance_id: &str) -> Result<()> {
    let mut inst = instances::get(l, instance_id).await?;
    inst.crash_dismissed_at = Some(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
    );
    instances::save(l, &inst).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const FABRIC_REPORT: &str = r#"---- Minecraft Crash Report ----
// Who set us up the TNT?

Time: 2026-11-10 13:05:11
Description: Initializing game

java.lang.RuntimeException: Could not execute entrypoint stage 'main' due to errors, provided by 'glow_ores' at 'com.claudemods.glow_ores.GlowOresMod'!
	at net.fabricmc.loader.impl.FabricLoaderImpl.lambda$invokeEntrypoints$2(FabricLoaderImpl.java:388)
Caused by: java.lang.NullPointerException: Cannot invoke "Object.toString()" because "x" is null
	at com.claudemods.glow_ores.GlowOresMod.onInitialize(GlowOresMod.java:14)
	at net.caffeinemc.mods.sodium.client.Foo.bar(Foo.java:1)
"#;

    fn entry(id: &str, file: &str, project: Option<&str>) -> mods::ModEntry {
        mods::ModEntry {
            file_name: file.into(),
            enabled: true,
            mod_id: Some(id.into()),
            name: Some(id.into()),
            version: None,
            description: None,
            authors: vec![],
            loader: "fabric".into(),
            size: 0,
            claude_project: project.map(String::from),
            online: None,
        }
    }

    #[test]
    fn summary() {
        let (d, e) = summarize(FABRIC_REPORT);
        assert_eq!(d.as_deref(), Some("Initializing game"));
        assert!(e.unwrap().starts_with("java.lang.RuntimeException: Could not execute entrypoint"));
    }

    #[test]
    fn suspects() {
        let installed = vec![
            entry("glow_ores", "glow_ores-1.0.0.jar", Some("glow_ores")),
            entry("sodium", "sodium.jar", None),
            entry("lithium", "lithium.jar", None),
            entry("fabric-api", "fabric-api.jar", None),
        ];
        let s = find_suspects(FABRIC_REPORT, &installed);
        let ids: Vec<_> = s.iter().map(|x| x.mod_id.as_str()).collect();
        assert_eq!(ids, vec!["glow_ores", "sodium"]);
        assert_eq!(s[0].claude_project.as_deref(), Some("glow_ores"));
    }

    #[tokio::test]
    async fn detects_and_dismisses() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(dir.path())).await.unwrap();
        let mut inst = instances::create(&l, instances::NewInstance {
            name: Some("c".into()), mc_version: "1.21.1".into(), loader: crate::loaders::Loader::Fabric,
            loader_version: Some("0.17.2".into()), ram_mb: None,
        }).await.unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        inst.last_played = Some(now - 60);
        instances::save(&l, &inst).await.unwrap();
        assert!(latest(&l, &inst).await.unwrap().is_none(), "no crash yet");

        let reports = l.paths.game_dir(&inst.id).join("crash-reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("crash-2026-11-10_13.05.11-client.txt"), FABRIC_REPORT).unwrap();
        inst.last_exit = Some(instances::LastExit { code: Some(-1), at: now });
        instances::save(&l, &inst).await.unwrap();
        let c = latest(&l, &inst).await.unwrap().unwrap();
        assert_eq!(c.description.as_deref(), Some("Initializing game"));
        assert!(c.report_path.is_some());

        dismiss(&l, &inst.id).await.unwrap();
        let inst = instances::get(&l, &inst.id).await.unwrap();
        assert!(latest(&l, &inst).await.unwrap().is_none(), "dismissed");
    }

    #[tokio::test]
    async fn crash_without_report_uses_output() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(crate::Paths::new(dir.path())).await.unwrap();
        let mut inst = instances::create(&l, instances::NewInstance {
            name: Some("d".into()), mc_version: "1.21.1".into(), loader: crate::loaders::Loader::Fabric,
            loader_version: Some("0.17.2".into()), ram_mb: None,
        }).await.unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        inst.last_played = Some(now - 10);
        inst.last_exit = Some(instances::LastExit { code: Some(1), at: now });
        instances::save(&l, &inst).await.unwrap();
        std::fs::write(
            l.paths.instance_dir(&inst.id).join("launcher-output.log"),
            "[main/ERROR]: Incompatible mods found!\nnet.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible\n",
        ).unwrap();
        let c = latest(&l, &inst).await.unwrap().unwrap();
        assert!(c.report_path.is_none());
        assert!(c.exception.unwrap().starts_with("net.fabricmc.loader.impl.FormattedException"));

        // A clean stop (code None) is not a crash.
        let mut inst = instances::get(&l, &inst.id).await.unwrap();
        inst.last_exit = Some(instances::LastExit { code: None, at: now });
        instances::save(&l, &inst).await.unwrap();
        assert!(latest(&l, &inst).await.unwrap().is_none());
    }
}
