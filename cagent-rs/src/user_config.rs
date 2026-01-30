//! User configuration (aliases, settings)
//!
//! Stored at XDG config dir: ~/.config/cagent/config.yaml (platform-dependent).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserConfig {
    #[serde(default)]
    pub version: String,

    #[serde(default)]
    pub models_gateway: Option<String>,

    #[serde(default)]
    pub aliases: HashMap<String, Alias>,

    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub theme: Option<String>,

    #[serde(default)]
    pub hide_tool_results: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Alias {
    pub path: String,

    #[serde(default)]
    pub yolo: bool,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub hide_tool_results: bool,
}

pub struct UserConfigStore {
    path: PathBuf,
}

impl UserConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn new_default() -> anyhow::Result<Self> {
        Ok(Self::new(default_config_path()?))
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn load(&self) -> anyhow::Result<UserConfig> {
        if !self.path.exists() {
            return Ok(UserConfig {
                version: "v1".to_string(),
                ..Default::default()
            });
        }

        let content = std::fs::read_to_string(&self.path)?;
        let mut cfg: UserConfig = serde_yaml::from_str(&content)?;
        if cfg.version.trim().is_empty() {
            cfg.version = "v1".to_string();
        }
        Ok(cfg)
    }

    pub fn save(&self, cfg: &UserConfig) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut cfg = cfg.clone();
        if cfg.version.trim().is_empty() {
            cfg.version = "v1".to_string();
        }

        let yaml = serde_yaml::to_string(&cfg)?;

        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, yaml.as_bytes())?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

pub fn default_config_dir() -> anyhow::Result<PathBuf> {
    dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("failed to resolve config dir"))
        .map(|p| p.join("cagent"))
}

pub fn default_config_path() -> anyhow::Result<PathBuf> {
    Ok(default_config_dir()?.join("config.yaml"))
}

pub fn expand_tilde(path: &str) -> anyhow::Result<String> {
    if !path.starts_with("~/") {
        return Ok(path.to_string());
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("failed to resolve home dir"))?;
    Ok(home
        .join(path.trim_start_matches("~/"))
        .to_string_lossy()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_roundtrip_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let store = UserConfigStore::new(tmp.path().join("config.yaml"));

        let mut cfg = store.load().unwrap();
        assert_eq!(cfg.version, "v1");

        cfg.aliases.insert(
            "code".to_string(),
            Alias {
                path: "./agent.yaml".to_string(),
                yolo: true,
                model: Some("openai/gpt-4o-mini".to_string()),
                hide_tool_results: true,
            },
        );
        cfg.settings.theme = Some("dark".to_string());

        store.save(&cfg).unwrap();
        let got = store.load().unwrap();

        assert_eq!(got.aliases.len(), 1);
        let a = got.aliases.get("code").unwrap();
        assert_eq!(a.path, "./agent.yaml");
        assert!(a.yolo);
        assert_eq!(a.model.as_deref(), Some("openai/gpt-4o-mini"));
        assert!(a.hide_tool_results);
        assert_eq!(got.settings.theme.as_deref(), Some("dark"));
    }
}
