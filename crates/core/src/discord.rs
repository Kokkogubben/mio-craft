//! Discord Rich Presence: shows what is being played on the user's Discord
//! profile while a game runs. Off unless it is turned on in Settings.
//!
//! This talks only to the Discord app on the same computer, over its local IPC
//! pipe. A message is a frame of: opcode (u32 LE), length (u32 LE), JSON.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::timeout;

use crate::loaders::Loader;
use crate::{instances, projects, Event, Launcher};

/// MioCraft's Discord application ID (discord.com/developers/applications).
/// Application IDs are not secrets. The name and icon Discord shows come from
/// that application. A build with `None` here has no Rich Presence.
pub const DISCORD_APP_ID: Option<&str> = Some("1570505425374412870");

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const MAX_FRAME: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// How often the presence is re-sent while a game runs. This is also how a
/// Discord that was started (or restarted) after the game gets picked up.
const REFRESH: Duration = Duration::from_secs(15);
/// Discord rejects `details` and `state` longer than this.
const MAX_TEXT: usize = 128;

/// What Discord shows under "Playing MioCraft".
#[derive(Debug, Clone, PartialEq)]
struct Activity {
    details: String,
    state: String,
    /// Unix seconds; Discord shows the time elapsed since then.
    start: u64,
}

impl Activity {
    fn game(name: &str, mc_version: &str, loader: Loader, start: u64) -> Self {
        Self { details: clip(format!("Playing {name}")), state: version_line(mc_version, loader), start }
    }

    fn dev(project: &str, mc_version: &str, loader: Loader, start: u64) -> Self {
        Self { details: clip(format!("Testing a mod: {project}")), state: version_line(mc_version, loader), start }
    }

    fn to_json(&self) -> Value {
        json!({
            "details": self.details,
            "state": self.state,
            "timestamps": { "start": self.start },
            "instance": false,
        })
    }
}

fn version_line(mc_version: &str, loader: Loader) -> String {
    let loader = match loader {
        Loader::Vanilla => "Vanilla",
        Loader::Fabric => "Fabric",
        Loader::Neoforge => "NeoForge",
    };
    clip(format!("Minecraft {mc_version} · {loader}"))
}

fn clip(s: String) -> String {
    if s.chars().count() <= MAX_TEXT {
        return s;
    }
    let mut out: String = s.chars().take(MAX_TEXT - 1).collect();
    out.push('…');
    out
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Keep the Discord presence in step with what is running, until the launcher
/// exits. Does nothing while the setting is off.
pub fn start(l: &Launcher) {
    let Some(app_id) = DISCORD_APP_ID else { return };
    let l = l.clone();
    let mut events = l.events.subscribe();
    tokio::spawn(async move {
        // When each running process was first seen, by source (`game:<id>`, `dev:<id>`).
        let mut started: HashMap<String, u64> = HashMap::new();
        let mut conn: Option<Ipc> = None;
        let mut tick = tokio::time::interval(REFRESH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                ev = events.recv() => match ev {
                    Ok(Event::ProcessStarted { .. } | Event::ProcessExited { .. } | Event::ConfigChanged) => {}
                    // Events were dropped; the process table below is still the truth.
                    Err(RecvError::Lagged(_)) => {}
                    Ok(_) => continue,
                    Err(RecvError::Closed) => break,
                },
                _ = tick.tick() => {}
            }

            let running = l.processes.running().await;
            started.retain(|source, _| running.contains(source));
            for source in running {
                started.entry(source).or_insert_with(now);
            }

            let enabled = l.config.read().await.discord_rpc;
            let want = if enabled { pick_activity(&l, &started).await } else { None };
            match want {
                None => {
                    // Closing the pipe would clear it too; asking first is tidier.
                    if let Some(mut c) = conn.take() {
                        let _ = c.set_activity(None).await;
                    }
                }
                Some(activity) => {
                    // Two tries: a pipe left over from a Discord that has since
                    // been closed only fails once it is written to.
                    for _ in 0..2 {
                        if conn.is_none() {
                            conn = Ipc::connect(app_id).await.ok();
                        }
                        let Some(c) = conn.as_mut() else { break };
                        if c.set_activity(Some(&activity)).await.is_ok() {
                            break;
                        }
                        conn = None;
                    }
                }
            }
        }
    });
}

/// A real game wins over a dev client; among equals, the one started first.
async fn pick_activity(l: &Launcher, started: &HashMap<String, u64>) -> Option<Activity> {
    let mut sources: Vec<(&String, u64)> = started.iter().map(|(s, t)| (s, *t)).collect();
    sources.sort_by_key(|(source, start)| (!source.starts_with("game:"), *start));
    for (source, start) in sources {
        if let Some(id) = source.strip_prefix("game:") {
            if let Ok(i) = instances::get(l, id).await {
                return Some(Activity::game(&i.name, &i.mc_version, i.loader, start));
            }
        } else if let Some(id) = source.strip_prefix("dev:") {
            if let Ok(p) = projects::get(l, id).await {
                return Some(Activity::dev(&p.name, &p.mc_version, p.loader, start));
            }
        }
    }
    None
}

trait Pipe: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Pipe for T {}

/// A connection to the local Discord app.
struct Ipc {
    pipe: Box<dyn Pipe>,
}

impl Ipc {
    async fn connect(app_id: &str) -> Result<Self> {
        Self::handshake(open_pipe().await?, app_id).await
    }

    async fn handshake(pipe: Box<dyn Pipe>, app_id: &str) -> Result<Self> {
        let mut ipc = Self { pipe };
        ipc.send(OP_HANDSHAKE, &json!({ "v": 1, "client_id": app_id })).await?;
        ipc.reply().await?;
        Ok(ipc)
    }

    /// Show `activity`, or clear the presence with `None`.
    async fn set_activity(&mut self, activity: Option<&Activity>) -> Result<()> {
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": std::process::id(), "activity": activity.map(Activity::to_json) },
            "nonce": uuid::Uuid::new_v4().to_string(),
        });
        self.send(OP_FRAME, &payload).await?;
        self.reply().await
    }

    async fn send(&mut self, op: u32, payload: &Value) -> Result<()> {
        timeout(IO_TIMEOUT, self.pipe.write_all(&encode(op, payload))).await??;
        Ok(())
    }

    /// Read Discord's answer. Every command gets one, and leaving them unread
    /// would fill the pipe.
    async fn reply(&mut self) -> Result<()> {
        let (op, body) = timeout(IO_TIMEOUT, read_frame(&mut self.pipe)).await??;
        if op == OP_CLOSE || body["evt"] == "ERROR" {
            let message = body["data"]["message"].as_str().or(body["message"].as_str()).unwrap_or("no reason given");
            bail!("Discord refused the request: {message}");
        }
        Ok(())
    }
}

fn encode(op: u32, payload: &Value) -> Vec<u8> {
    let body = payload.to_string();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body.as_bytes());
    out
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head).await?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_FRAME {
        bail!("Discord sent an oversized message ({len} bytes)");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

/// Discord listens on the first free pipe of `discord-ipc-0` to `-9`.
#[cfg(windows)]
async fn open_pipe() -> Result<Box<dyn Pipe>> {
    use tokio::net::windows::named_pipe::ClientOptions;
    for n in 0..10 {
        if let Ok(pipe) = ClientOptions::new().open(format!(r"\\.\pipe\discord-ipc-{n}")) {
            return Ok(Box::new(pipe));
        }
    }
    bail!("Discord isn't running")
}

#[cfg(unix)]
async fn open_pipe() -> Result<Box<dyn Pipe>> {
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|k| std::env::var_os(k))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into());
    // Flatpak and Snap builds of Discord put the socket in their own folder.
    for dir in [base.clone(), base.join("app/com.discordapp.Discord"), base.join("snap.discord")] {
        for n in 0..10 {
            if let Ok(sock) = tokio::net::UnixStream::connect(dir.join(format!("discord-ipc-{n}"))).await {
                return Ok(Box::new(sock));
            }
        }
    }
    bail!("Discord isn't running")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_text() {
        let a = Activity::game("My World", "1.21.1", Loader::Fabric, 100);
        assert_eq!(a.details, "Playing My World");
        assert_eq!(a.state, "Minecraft 1.21.1 · Fabric");
        assert_eq!(a.to_json()["timestamps"]["start"], 100);

        let d = Activity::dev("Ruby Ore", "26.3", Loader::Neoforge, 5);
        assert_eq!(d.details, "Testing a mod: Ruby Ore");
        assert_eq!(d.state, "Minecraft 26.3 · NeoForge");

        let long = Activity::game(&"x".repeat(300), "1.21.1", Loader::Vanilla, 0);
        assert_eq!(long.details.chars().count(), MAX_TEXT);
        assert!(long.details.ends_with('…'));
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let bytes = encode(OP_FRAME, &json!({ "cmd": "X" }));
        assert_eq!(&bytes[..4], &1u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
        let (op, body) = read_frame(&mut &bytes[..]).await.unwrap();
        assert_eq!((op, body), (OP_FRAME, json!({ "cmd": "X" })));

        let mut huge = OP_FRAME.to_le_bytes().to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_frame(&mut &huge[..]).await.is_err());
    }

    /// A stand-in for Discord: answers the handshake, then each command.
    #[tokio::test]
    async fn talks_to_discord() {
        let (ours, mut theirs) = tokio::io::duplex(4096);
        let discord = tokio::spawn(async move {
            let (op, hello) = read_frame(&mut theirs).await.unwrap();
            assert_eq!(op, OP_HANDSHAKE);
            assert_eq!(hello, json!({ "v": 1, "client_id": "123" }));
            theirs.write_all(&encode(OP_FRAME, &json!({ "cmd": "DISPATCH", "evt": "READY" }))).await.unwrap();

            let (op, set) = read_frame(&mut theirs).await.unwrap();
            assert_eq!(op, OP_FRAME);
            assert_eq!(set["cmd"], "SET_ACTIVITY");
            assert_eq!(set["args"]["activity"]["details"], "Playing My World");
            theirs.write_all(&encode(OP_FRAME, &json!({ "cmd": "SET_ACTIVITY", "evt": null }))).await.unwrap();

            let (_, clear) = read_frame(&mut theirs).await.unwrap();
            assert!(clear["args"]["activity"].is_null());
            let refused = json!({ "cmd": "SET_ACTIVITY", "evt": "ERROR", "data": { "message": "nope" } });
            theirs.write_all(&encode(OP_FRAME, &refused)).await.unwrap();
        });

        let mut ipc = Ipc::handshake(Box::new(ours), "123").await.unwrap();
        let activity = Activity::game("My World", "1.21.1", Loader::Fabric, 1);
        ipc.set_activity(Some(&activity)).await.unwrap();
        let err = ipc.set_activity(None).await.unwrap_err();
        assert!(err.to_string().contains("nope"));
        discord.await.unwrap();
    }

    /// Shows a status on the Discord running on this computer for 20 seconds:
    /// `cargo test -p launcher-core real_discord -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the Discord app to be open"]
    async fn real_discord() {
        let mut ipc = Ipc::connect(DISCORD_APP_ID.unwrap()).await.unwrap();
        let activity = Activity::game("Test instance", "1.21.1", Loader::Fabric, now());
        ipc.set_activity(Some(&activity)).await.unwrap();
        println!("status shown; look at your Discord profile");
        tokio::time::sleep(Duration::from_secs(20)).await;
        ipc.set_activity(None).await.unwrap();
    }
}
