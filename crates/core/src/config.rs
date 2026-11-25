use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Azure application (client) ID used for Microsoft sign-in.
    /// Every third-party launcher needs its own; see README.
    pub ms_client_id: Option<String>,
    /// Memory for instances that haven't set their own (memory is per instance since 0.6.1).
    pub default_ram_mb: u32,
    pub selected_account: Option<String>,
    /// JVM arguments for instances that haven't set their own (they are per instance since 0.6.1).
    pub extra_jvm_args: Vec<String>,
    /// CurseForge API key (from console.curseforge.com). Modrinth needs none.
    pub curseforge_api_key: Option<String>,
    /// The first-run setup has been finished or skipped.
    pub onboarded: bool,
    /// Show what is being played on the user's Discord profile.
    pub discord_rpc: bool,
    /// Use the original layout (sidebar of instances) instead of the home screen with the player.
    /// Replaces `experimental_ui` from 0.5, when the new look was opt-in; that setting is ignored.
    pub legacy_look: bool,
    /// Use the original green and grey colours instead of the copper theme.
    pub legacy_colors: bool,
    /// The pixel font and pixel-art buttons and frames of the new look. Off, the new layout keeps
    /// the original font and plain controls.
    pub stylized: bool,
    /// Minimize the launcher window while a game runs and bring it back when the game closes.
    pub minimize_while_playing: bool,
    /// Update only to versions marked stable (the `stable` GitHub release) instead of every release.
    pub stable_updates: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ms_client_id: None,
            default_ram_mb: 4096,
            selected_account: None,
            extra_jvm_args: vec![],
            curseforge_api_key: None,
            onboarded: false,
            discord_rpc: false,
            legacy_look: false,
            legacy_colors: false,
            stylized: true,
            minimize_while_playing: false,
            stable_updates: false,
        }
    }
}

impl Config {
    pub async fn load(paths: &Paths) -> Result<Self> {
        match tokio::fs::read(paths.config_file()).await {
            Ok(b) => Ok(serde_json::from_slice(&b).unwrap_or_default()),
            Err(_) => Ok(Self::default()),
        }
    }

    pub async fn save(&self, paths: &Paths) -> Result<()> {
        crate::http::write_json(&paths.config_file(), self).await
    }
}

/// Fields the UI may change. `None` leaves a field unchanged.
#[derive(Debug, Deserialize, Default)]
pub struct ConfigPatch {
    pub ms_client_id: Option<String>,
    pub default_ram_mb: Option<u32>,
    pub selected_account: Option<String>,
    pub extra_jvm_args: Option<Vec<String>>,
    pub curseforge_api_key: Option<String>,
    pub onboarded: Option<bool>,
    pub discord_rpc: Option<bool>,
    pub legacy_look: Option<bool>,
    pub legacy_colors: Option<bool>,
    pub stylized: Option<bool>,
    pub minimize_while_playing: Option<bool>,
    pub stable_updates: Option<bool>,
}

impl Config {
    pub fn apply(&mut self, p: ConfigPatch) {
        if let Some(v) = p.ms_client_id {
            let v = v.trim().to_string();
            self.ms_client_id = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = p.default_ram_mb {
            self.default_ram_mb = v.clamp(512, 65536);
        }
        if let Some(v) = p.selected_account {
            self.selected_account = Some(v);
        }
        if let Some(v) = p.extra_jvm_args {
            self.extra_jvm_args = v;
        }
        if let Some(v) = p.curseforge_api_key {
            let v = v.trim().to_string();
            self.curseforge_api_key = if v.is_empty() { None } else { Some(v) };
        }
        if let Some(v) = p.onboarded {
            self.onboarded = v;
        }
        if let Some(v) = p.discord_rpc {
            self.discord_rpc = v;
        }
        if let Some(v) = p.legacy_look {
            self.legacy_look = v;
        }
        if let Some(v) = p.legacy_colors {
            self.legacy_colors = v;
        }
        if let Some(v) = p.stylized {
            self.stylized = v;
        }
        if let Some(v) = p.minimize_while_playing {
            self.minimize_while_playing = v;
        }
        if let Some(v) = p.stable_updates {
            self.stable_updates = v;
        }
    }
}
