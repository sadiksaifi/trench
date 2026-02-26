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

    #[test]
    fn partial_toml_only_ui_section() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[ui]
theme = "solarized"
"#,
        );

        let config = load_global_config_from(&path).unwrap();

        let ui = config.ui.unwrap();
        assert_eq!(ui.theme.as_deref(), Some("solarized"));
        assert!(ui.date_format.is_none());
        assert!(ui.show_ahead_behind.is_none());
        assert!(ui.show_dirty_count.is_none());

        assert!(config.git.is_none());
        assert!(config.worktrees.is_none());
    }

    #[test]
    fn partial_toml_mixed_sections_and_fields() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[git]
default_base = "develop"

[worktrees]
scan = ["/opt/trees"]
"#,
        );

        let config = load_global_config_from(&path).unwrap();

        assert!(config.ui.is_none());

        let git = config.git.unwrap();
        assert_eq!(git.default_base.as_deref(), Some("develop"));
        assert!(git.auto_prune.is_none());
        assert!(git.fetch_on_open.is_none());

        let wt = config.worktrees.unwrap();
        assert!(wt.root.is_none());
        assert_eq!(wt.scan, Some(vec!["/opt/trees".to_string()]));
    }

    #[test]
    fn empty_file_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        let config = load_global_config_from(&path).unwrap();
        assert_eq!(config, GlobalConfig::default());
    }

    #[test]
    fn invalid_toml_returns_error_with_path() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "this is not [valid toml");

        let err = load_global_config_from(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid TOML"),
            "expected 'invalid TOML' in error: {msg}"
        );
        assert!(
            msg.contains("config.toml"),
            "expected file path in error: {msg}"
        );
    }

    #[test]
    fn wrong_type_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[ui]
show_ahead_behind = "yes"
"#,
        );

        let err = load_global_config_from(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid TOML"),
            "expected 'invalid TOML' in error: {msg}"
        );
    }
}
