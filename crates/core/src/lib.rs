//! Claude Launcher core.
//!
//! Everything the launcher does lives here so that the desktop app, the
//! headless binary and the local control API (used by the MCP server) all
//! share one implementation.

pub mod api;
pub mod auth;
pub mod browse;
pub mod config;
pub mod events;
pub mod gradle;
pub mod http;
pub mod instances;
pub mod java;
pub mod launch;
pub mod loaders;
pub mod maven;
pub mod mods;
pub mod mojang;
pub mod paths;
pub mod platform;
pub mod process;
pub mod projects;
pub mod templates;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, RwLock};

pub use events::Event;
pub use paths::Paths;

/// Shared launcher state. Cheap to clone (it is an `Arc`).
#[derive(Clone)]
pub struct Launcher(Arc<Inner>);

pub struct Inner {
    pub paths: Paths,
    pub http: reqwest::Client,
    pub events: broadcast::Sender<Event>,
    pub config: RwLock<config::Config>,
    pub processes: process::ProcessTable,
    /// Serialises installs so two requests never write the same files at once.
    pub install_lock: tokio::sync::Mutex<()>,
}

impl std::ops::Deref for Launcher {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl Launcher {
    pub async fn new(paths: Paths) -> Result<Self> {
        paths.ensure_dirs().await?;
        let config = config::Config::load(&paths).await?;
        let (tx, _) = broadcast::channel(2048);
        Ok(Self(Arc::new(Inner {
            paths,
            http: http::client()?,
            events: tx,
            config: RwLock::new(config),
            processes: process::ProcessTable::default(),
            install_lock: tokio::sync::Mutex::new(()),
        })))
    }

    pub fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    pub fn progress(&self, task: &str, current: u64, total: u64, message: impl Into<String>) {
        self.emit(Event::Progress {
            task: task.to_string(),
            current,
            total,
            message: message.into(),
        });
    }
}
