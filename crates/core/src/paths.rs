use std::path::{Path, PathBuf};

use anyhow::Result;

/// Folder layout under the launcher's data directory
/// (`%APPDATA%\MioCraft` on Windows).
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_root() -> PathBuf {
        if let Ok(p) = std::env::var("MIO_CRAFT_HOME") {
            return PathBuf::from(p);
        }
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("MioCraft")
    }

    pub async fn ensure_dirs(&self) -> Result<()> {
        for d in [
            self.versions(),
            self.libraries(),
            self.assets(),
            self.java(),
            self.instances(),
            self.projects(),
            self.gradle(),
            self.cache(),
        ] {
            tokio::fs::create_dir_all(d).await?;
        }
        Ok(())
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }
    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }
    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn java(&self) -> PathBuf {
        self.root.join("java")
    }
    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }
    pub fn instance_dir(&self, id: &str) -> PathBuf {
        self.instances().join(id)
    }
    /// The game directory (`.minecraft` equivalent) of an instance.
    pub fn game_dir(&self, id: &str) -> PathBuf {
        self.instance_dir(id).join("minecraft")
    }
    pub fn mods_dir(&self, id: &str) -> PathBuf {
        self.game_dir(id).join("mods")
    }
    pub fn projects(&self) -> PathBuf {
        self.root.join("projects")
    }
    pub fn gradle(&self) -> PathBuf {
        self.root.join("gradle")
    }
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }
    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }
    /// Written at startup so the MCP server can find the local API.
    pub fn api_file(&self) -> PathBuf {
        self.root.join("api.json")
    }
}

/// Make sure a path given by a client stays inside `base` (no `..` tricks).
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    for c in rel_path.components() {
        match c {
            std::path::Component::Normal(_) => {}
            _ => anyhow::bail!("invalid path component in {rel:?}"),
        }
    }
    Ok(base.join(rel_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_join_rejects_traversal() {
        let b = Path::new("/base");
        assert!(safe_join(b, "a/b.jar").is_ok());
        assert!(safe_join(b, "../x").is_err());
        assert!(safe_join(b, "/abs").is_err());
    }
}
