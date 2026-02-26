use std::path::Path;

use anyhow::{Context, Result};
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
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    let config: GlobalConfig = toml::from_str(&contents)
        .with_context(|| format!("invalid TOML in config file: {}", path.display()))?;
    Ok(config)
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

    fn write_config(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

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

    #[test]
    fn full_valid_toml_loads_all_fields() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[ui]
theme = "dark"
date_format = "%Y-%m-%d"
show_ahead_behind = true
show_dirty_count = false

[git]
default_base = "main"
auto_prune = true
fetch_on_open = false

[worktrees]
root = "{{ repo }}/{{ branch | sanitize }}"
scan = ["/home/user/projects", "/tmp/worktrees"]
"#,
        );

        let config = load_global_config_from(&path).unwrap();

        let ui = config.ui.unwrap();
        assert_eq!(ui.theme.as_deref(), Some("dark"));
        assert_eq!(ui.date_format.as_deref(), Some("%Y-%m-%d"));
        assert_eq!(ui.show_ahead_behind, Some(true));
        assert_eq!(ui.show_dirty_count, Some(false));

        let git = config.git.unwrap();
        assert_eq!(git.default_base.as_deref(), Some("main"));
        assert_eq!(git.auto_prune, Some(true));
        assert_eq!(git.fetch_on_open, Some(false));

        let wt = config.worktrees.unwrap();
        assert_eq!(
            wt.root.as_deref(),
            Some("{{ repo }}/{{ branch | sanitize }}")
        );
        assert_eq!(
            wt.scan,
            Some(vec![
                "/home/user/projects".to_string(),
                "/tmp/worktrees".to_string()
            ])
        );
    }
}
