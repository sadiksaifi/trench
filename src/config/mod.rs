use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

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
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GlobalConfig::default());
        }
        Err(e) => {
            return Err(
                anyhow::Error::new(e)
                    .context(format!("failed to read config file: {}", path.display())),
            );
        }
    };
    let config: GlobalConfig = toml::from_str(&contents)
        .with_context(|| format!("invalid TOML in config file: {}", path.display()))?;
    Ok(config)
}

/// Return the path to the global config file (`~/.config/trench/config.toml`).
pub fn global_config_path() -> Result<PathBuf> {
    Ok(paths::config_dir()?.join("config.toml"))
}

/// Load global config from the XDG config directory.
///
/// Reads `~/.config/trench/config.toml` (or platform equivalent).
/// Returns defaults if the file does not exist.
pub fn load_global_config() -> Result<GlobalConfig> {
    let path = global_config_path()?;
    load_global_config_from(&path)
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

    #[test]
    fn non_notfound_io_error_propagates() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let restricted = dir.path().join("restricted");
        std::fs::create_dir(&restricted).unwrap();
        let path = restricted.join("config.toml");
        std::fs::write(&path, "[ui]\ntheme = \"dark\"\n").unwrap();

        // Remove all permissions from parent → metadata/read on child fails with PermissionDenied.
        std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_global_config_from(&path);

        // Restore permissions so TempDir cleanup succeeds.
        std::fs::set_permissions(&restricted, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("failed to read config file"),
            "expected 'failed to read config file' in error: {msg}"
        );
    }

    #[test]
    fn global_config_path_points_to_xdg_config() {
        let path = global_config_path().unwrap();
        assert!(path.ends_with("trench/config.toml"));
        assert!(path.starts_with(dirs::config_dir().unwrap()));
    }

    #[test]
    fn load_global_config_returns_valid_result() {
        // On the test runner, the file likely doesn't exist — should return defaults.
        // If it does exist, should parse successfully.
        let config = load_global_config().unwrap();
        // Regardless of file state, this should not error
        let _ = config;
    }
}
