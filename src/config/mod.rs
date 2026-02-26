use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct GlobalConfig {
    pub ui: Option<UiConfig>,
    pub git: Option<GitConfig>,
    pub worktrees: Option<WorktreesConfig>,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct UiConfig {
    pub theme: Option<String>,
    pub date_format: Option<String>,
    pub show_ahead_behind: Option<bool>,
    pub show_dirty_count: Option<bool>,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct GitConfig {
    pub default_base: Option<String>,
    pub auto_prune: Option<bool>,
    pub fetch_on_open: Option<bool>,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct WorktreesConfig {
    pub root: Option<String>,
    pub scan: Option<Vec<String>>,
}

/// Load global config from a specific file path.
///
/// Returns `GlobalConfig::default()` if the file does not exist.
/// Returns an error if the file exists but contains invalid TOML.
pub fn load_global_config_from(path: &Path) -> Result<GlobalConfig> {
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    todo!()
}

/// Load global config from the XDG config directory.
///
/// Reads `~/.config/trench/config.toml` (or platform equivalent).
/// Returns defaults if the file does not exist.
pub fn load_global_config() -> Result<GlobalConfig> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.toml");

        let config = load_global_config_from(&path).unwrap();

        assert_eq!(config, GlobalConfig::default());
        assert!(config.ui.is_none());
        assert!(config.git.is_none());
        assert!(config.worktrees.is_none());
    }
}
