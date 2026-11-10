use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Azure application (client) ID used for Microsoft sign-in.
    /// Every third-party launcher needs its own; see README.
    pub ms_client_id: Option<String>,
    pub default_ram_mb: u32,
    pub selected_account: Option<String>,
    /// Extra JVM arguments added to every launch.
    pub extra_jvm_args: Vec<String>,
    /// CurseForge API key (from console.curseforge.com). Modrinth needs none.
    pub curseforge_api_key: Option<String>,
    /// The first-run setup has been finished or skipped.
    pub onboarded: bool,
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
    }
}
