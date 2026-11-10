//! Long-running child processes (the game, Gradle dev clients) with their
//! output streamed to events and a log file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{bail, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex};

use crate::{Event, Launcher};

#[derive(Default)]
pub struct ProcessTable {
    inner: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl ProcessTable {
    pub async fn is_running(&self, source: &str) -> bool {
        self.inner.lock().await.contains_key(source)
    }
    pub async fn running(&self) -> Vec<String> {
        self.inner.lock().await.keys().cloned().collect()
    }
    /// Ask a process to stop (kills it).
    pub async fn stop(&self, source: &str) -> bool {
        match self.inner.lock().await.remove(source) {
            Some(tx) => {
                let _ = tx.send(());
                true
            }
            None => false,
        }
    }
}

pub fn hide_window(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Lines that look like secrets are masked before they reach logs or the UI.
pub fn redact(line: &str, secrets: &[String]) -> String {
    let mut out = line.to_string();
    for s in secrets {
        if s.len() >= 8 {
            out = out.replace(s.as_str(), "********");
        }
    }
    out
}

/// Spawn `cmd`, stream its output under `source`, and append it to `log_file`.
pub async fn spawn_tracked(
    l: &Launcher,
    source: &str,
    mut cmd: tokio::process::Command,
    log_file: PathBuf,
    secrets: Vec<String>,
) -> Result<()> {
    if l.processes.is_running(source).await {
        bail!("{source} is already running");
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    cmd.kill_on_drop(true);
    hide_window(&mut cmd);
    let mut child = cmd.spawn()?;

    if let Some(p) = log_file.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let log = tokio::fs::File::create(&log_file).await?;
    let log = std::sync::Arc::new(Mutex::new(log));

    let (stop_tx, stop_rx) = oneshot::channel();
    l.processes.inner.lock().await.insert(source.to_string(), stop_tx);
    l.emit(Event::ProcessStarted { source: source.to_string() });

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    for reader in [
        Box::new(stdout) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
        Box::new(stderr),
    ] {
        let l = l.clone();
        let source = source.to_string();
        let log = log.clone();
        let secrets = secrets.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = redact(&line, &secrets);
                {
                    let mut f = log.lock().await;
                    let _ = f.write_all(line.as_bytes()).await;
                    let _ = f.write_all(b"\n").await;
                }
                l.emit(Event::Log { source: source.clone(), line });
            }
        });
    }

    let l2 = l.clone();
    let source = source.to_string();
    tokio::spawn(async move {
        let code = tokio::select! {
            status = child.wait() => status.ok().and_then(|s| s.code()),
            _ = stop_rx => {
                let _ = child.kill().await;
                None
            }
        };
        l2.processes.inner.lock().await.remove(&source);
        l2.emit(Event::ProcessExited { source, code });
    });
    Ok(())
}

/// Last `n` lines of a text file.
pub async fn tail_file(path: &std::path::Path, n: usize) -> Result<String> {
    let bytes = tokio::fs::read(path).await?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    Ok(lines[start..].join("\n"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn redacts() {
        assert_eq!(
            super::redact("token=abcdefghij x", &["abcdefghij".into()]),
            "token=******** x"
        );
    }
}
