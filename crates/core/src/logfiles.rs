//! An instance's log files for the Console tab: the game's current and older
//! logs (`logs/*.log`, `logs/*.log.gz`), its crash reports, and the launcher's
//! record of the last launch.

use std::io::Read;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use anyhow::{bail, Result};
use serde::Serialize;

use crate::{instances, Launcher};

#[derive(Debug, Clone, Serialize)]
pub struct LogFile {
    /// `log` (logs folder), `crash` (crash-reports) or `launcher` (the last launch's output).
    pub kind: &'static str,
    pub name: String,
    pub size: u64,
    /// Seconds since 1970.
    pub modified: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogText {
    pub text: String,
    /// Only the end of a very long file is returned.
    pub truncated: bool,
    /// The folder the file is in, for "Open folder".
    pub folder: PathBuf,
}

const MAX_FILES: usize = 300;
/// How much of a file is read (after unzipping), from the end.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const LAUNCHER_LOG: &str = "launcher-output.log";

fn folder(l: &Launcher, id: &str, kind: &str) -> Option<PathBuf> {
    let game = l.paths.game_dir(id);
    match kind {
        "log" => Some(game.join("logs")),
        "crash" => Some(game.join("crash-reports")),
        "launcher" => Some(l.paths.instance_dir(id)),
        _ => None,
    }
}

fn wanted(kind: &str, name: &str) -> bool {
    match kind {
        "log" => name.ends_with(".log") || name.ends_with(".log.gz"),
        "crash" => name.ends_with(".txt"),
        "launcher" => name == LAUNCHER_LOG,
        _ => false,
    }
}

pub async fn list(l: &Launcher, id: &str) -> Result<Vec<LogFile>> {
    instances::get(l, id).await?;
    let mut out = vec![];
    for kind in ["log", "crash", "launcher"] {
        let Some(dir) = folder(l, id, kind) else { continue };
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name().to_string_lossy().to_string();
            if !wanted(kind, &name) {
                continue;
            }
            // Plain files only: no links out of the folder.
            let Ok(m) = tokio::fs::symlink_metadata(e.path()).await else { continue };
            if !m.is_file() {
                continue;
            }
            let modified = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            out.push(LogFile { kind, name, size: m.len(), modified });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out.truncate(MAX_FILES);
    Ok(out)
}

/// The text of one log file (unzipped), or its end if it's very long.
pub async fn read(l: &Launcher, id: &str, kind: &str, name: &str) -> Result<LogText> {
    instances::get(l, id).await?;
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") || !wanted(kind, name) {
        bail!("No such log file");
    }
    let dir = folder(l, id, kind).unwrap();
    let path = dir.join(name);
    let m = tokio::fs::symlink_metadata(&path).await.map_err(|_| anyhow::anyhow!("That log file is gone"))?;
    if !m.is_file() {
        bail!("No such log file");
    }
    let gz = name.ends_with(".gz");
    let (bytes, truncated) = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, bool)> {
        let f = std::fs::File::open(&path)?;
        if gz {
            // Unzip, keeping only the last MAX_BYTES.
            let mut d = flate2::read::GzDecoder::new(f);
            let mut keep: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
            let mut buf = vec![0u8; 64 * 1024];
            let mut cut = false;
            let mut total = 0usize;
            loop {
                let n = d.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                total += n;
                // A zip bomb would take forever even if we only keep the end.
                if total > 512 * 1024 * 1024 {
                    bail!("That log is too big to show");
                }
                keep.extend(&buf[..n]);
                if keep.len() > MAX_BYTES {
                    keep.drain(..keep.len() - MAX_BYTES);
                    cut = true;
                }
            }
            Ok((keep.into_iter().collect(), cut))
        } else {
            use std::io::{Seek, SeekFrom};
            let mut f = f;
            let len = f.metadata()?.len() as usize;
            let cut = len > MAX_BYTES;
            if cut {
                f.seek(SeekFrom::Start((len - MAX_BYTES) as u64))?;
            }
            let mut v = Vec::with_capacity(len.min(MAX_BYTES));
            f.take(MAX_BYTES as u64).read_to_end(&mut v)?;
            Ok((v, cut))
        }
    })
    .await??;
    let mut text = String::from_utf8_lossy(&bytes).to_string();
    if truncated {
        // Start at a whole line.
        if let Some(i) = text.find('\n') {
            text.drain(..=i);
        }
    }
    Ok(LogText { text, truncated, folder: dir })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::Loader;
    use crate::Paths;
    use std::io::Write;

    #[tokio::test]
    async fn lists_and_reads_logs() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let inst = instances::create(&l, instances::NewInstance { name: Some("logs".into()), mc_version: "1.21.1".into(), loader: Loader::Vanilla, loader_version: None, ram_mb: None })
            .await
            .unwrap();
        let game = l.paths.game_dir(&inst.id);
        std::fs::create_dir_all(game.join("logs")).unwrap();
        std::fs::create_dir_all(game.join("crash-reports")).unwrap();
        std::fs::write(game.join("logs/latest.log"), "[12:00:00] [main/INFO]: Hello\n").unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"old line 1\nold line 2\n").unwrap();
        std::fs::write(game.join("logs/2026-11-01-1.log.gz"), gz.finish().unwrap()).unwrap();
        std::fs::write(game.join("logs/notes.txt"), "not a log").unwrap();
        std::fs::write(game.join("crash-reports/crash-2026-11-02_10.00.00-client.txt"), "---- Minecraft Crash Report ----").unwrap();

        let files = list(&l, &inst.id).await.unwrap();
        let mut names: Vec<_> = files.iter().map(|f| format!("{}/{}", f.kind, f.name)).collect();
        names.sort();
        assert_eq!(names, vec!["crash/crash-2026-11-02_10.00.00-client.txt", "log/2026-11-01-1.log.gz", "log/latest.log"]);

        assert_eq!(read(&l, &inst.id, "log", "2026-11-01-1.log.gz").await.unwrap().text, "old line 1\nold line 2\n");
        assert!(read(&l, &inst.id, "crash", "crash-2026-11-02_10.00.00-client.txt").await.unwrap().text.contains("Crash Report"));
        for (k, n) in [("log", "../instance.json"), ("log", "notes.txt"), ("crash", "x/../y.txt"), ("other", "latest.log"), ("launcher", "accounts.json")] {
            assert!(read(&l, &inst.id, k, n).await.is_err(), "{k}/{n}");
        }
    }

    #[tokio::test]
    async fn keeps_the_end_of_long_logs() {
        let dir = tempfile::tempdir().unwrap();
        let l = Launcher::new(Paths::new(dir.path())).await.unwrap();
        let inst = instances::create(&l, instances::NewInstance { name: Some("big".into()), mc_version: "1.21.1".into(), loader: Loader::Vanilla, loader_version: None, ram_mb: None })
            .await
            .unwrap();
        let logs = l.paths.game_dir(&inst.id).join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let line = "x".repeat(99) + "\n";
        let big = line.repeat(MAX_BYTES / 100 + 500) + "THE END\n";
        std::fs::write(logs.join("latest.log"), &big).unwrap();
        let t = read(&l, &inst.id, "log", "latest.log").await.unwrap();
        assert!(t.truncated);
        assert!(t.text.ends_with("THE END\n") && t.text.starts_with('x') && t.text.len() <= MAX_BYTES);
    }
}
