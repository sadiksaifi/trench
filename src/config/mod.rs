use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

// --- Hook types (FR-18, FR-19) ---

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct HookDef {
    pub copy: Option<Vec<String>>,
    pub run: Option<Vec<String>>,
    pub shell: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct HooksConfig {
    pub pre_create: Option<HookDef>,
    pub post_create: Option<HookDef>,
    pub pre_sync: Option<HookDef>,
    pub post_sync: Option<HookDef>,
    pub pre_remove: Option<HookDef>,
    pub post_remove: Option<HookDef>,
}

// --- Config structs ---

#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct GlobalConfig {
    pub ui: Option<UiConfig>,
    pub git: Option<GitConfig>,
    pub worktrees: Option<WorktreesConfig>,
}

/// Project-level config parsed from `.trench.toml` at repo root.
#[derive(Debug, Default, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub ui: Option<UiConfig>,
    pub git: Option<GitConfig>,
    pub worktrees: Option<WorktreesConfig>,
    pub hooks: Option<HooksConfig>,
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
    fn project_config_deserializes_with_hooks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".trench.toml");
        std::fs::write(
            &path,
            r#"
[hooks.post_create]
copy = [".env*", "!.env.example"]
run = ["bun install", "bunx prisma generate"]
timeout_secs = 300

[hooks.pre_remove]
shell = "pkill -f 'next dev' || true"
"#,
        )
        .unwrap();

        let config: ProjectConfig =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        let hooks = config.hooks.expect("hooks should be present");
        let post_create = hooks.post_create.expect("post_create should be present");
        assert_eq!(
            post_create.copy,
            Some(vec![".env*".to_string(), "!.env.example".to_string()])
        );
        assert_eq!(
            post_create.run,
            Some(vec![
                "bun install".to_string(),
                "bunx prisma generate".to_string()
            ])
        );
        assert_eq!(post_create.timeout_secs, Some(300));
        assert!(post_create.shell.is_none());

        let pre_remove = hooks.pre_remove.expect("pre_remove should be present");
        assert_eq!(
            pre_remove.shell.as_deref(),
            Some("pkill -f 'next dev' || true")
        );
        assert!(pre_remove.copy.is_none());
        assert!(pre_remove.run.is_none());

        assert!(hooks.pre_create.is_none());
        assert!(hooks.pre_sync.is_none());
        assert!(hooks.post_sync.is_none());
        assert!(hooks.post_remove.is_none());
    }

    #[test]
    fn project_config_deserializes_all_sections() {
        let toml_str = r#"
[ui]
theme = "nord"

[git]
default_base = "develop"

[worktrees]
root = "custom/{{ repo }}/{{ branch | sanitize }}"

[hooks.post_create]
run = ["make setup"]
"#;
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.ui.unwrap().theme.as_deref(), Some("nord"));
        assert_eq!(
            config.git.unwrap().default_base.as_deref(),
            Some("develop")
        );
        assert_eq!(
            config.worktrees.unwrap().root.as_deref(),
            Some("custom/{{ repo }}/{{ branch | sanitize }}")
        );
        assert!(config.hooks.unwrap().post_create.is_some());
    }

}
