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
    pub hooks: Option<HooksConfig>,
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

/// Read and parse an optional TOML config file.
///
/// Returns `Ok(None)` if the file does not exist.
/// Returns an error if the file exists but cannot be read or contains invalid TOML.
fn load_optional_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(e) => {
            return Err(
                anyhow::Error::new(e)
                    .context(format!("failed to read config file: {}", path.display())),
            );
        }
    };
    let config: T = toml::from_str(&contents)
        .with_context(|| format!("invalid TOML in config file: {}", path.display()))?;
    Ok(Some(config))
}

/// Load project config from a specific file path.
///
/// Returns `Ok(None)` if the file does not exist.
/// Returns an error if the file exists but contains invalid TOML.
pub fn load_project_config_from(path: &Path) -> Result<Option<ProjectConfig>> {
    load_optional_toml(path)
}

// --- Resolved config (FR-1) ---

/// CLI-level overrides that take highest precedence in the fallback chain.
#[derive(Debug, Default)]
pub struct CliConfigOverrides {
    pub default_base: Option<String>,
    pub worktree_root: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct ResolvedConfig {
    pub ui: ResolvedUiConfig,
    pub git: ResolvedGitConfig,
    pub worktrees: ResolvedWorktreesConfig,
    pub hooks: Option<HooksConfig>,
}

#[derive(Debug, PartialEq)]
pub struct ResolvedUiConfig {
    pub theme: String,
    pub date_format: String,
    pub show_ahead_behind: bool,
    pub show_dirty_count: bool,
}

#[derive(Debug, PartialEq)]
pub struct ResolvedGitConfig {
    pub default_base: String,
    pub auto_prune: bool,
    pub fetch_on_open: bool,
}

#[derive(Debug, PartialEq)]
pub struct ResolvedWorktreesConfig {
    pub root: String,
    pub scan: Vec<String>,
}

impl Default for ResolvedUiConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            date_format: "%Y-%m-%d %H:%M".to_string(),
            show_ahead_behind: true,
            show_dirty_count: true,
        }
    }
}

impl Default for ResolvedGitConfig {
    fn default() -> Self {
        Self {
            default_base: "main".to_string(),
            auto_prune: false,
            fetch_on_open: true,
        }
    }
}

impl Default for ResolvedWorktreesConfig {
    fn default() -> Self {
        Self {
            root: crate::paths::DEFAULT_WORKTREE_TEMPLATE.to_string(),
            scan: Vec::new(),
        }
    }
}

/// Resolve configuration by merging: CLI flags → project → global → defaults (FR-1).
///
/// Project hooks completely replace global hooks when present (FR-2).
/// Non-hook fields merge per-field: first non-None value wins.
pub fn resolve_config(
    cli: Option<&CliConfigOverrides>,
    project: Option<&ProjectConfig>,
    global: &GlobalConfig,
) -> ResolvedConfig {
    let defaults_ui = ResolvedUiConfig::default();
    let defaults_git = ResolvedGitConfig::default();
    let defaults_wt = ResolvedWorktreesConfig::default();

    let p_ui = project.and_then(|p| p.ui.as_ref());
    let p_git = project.and_then(|p| p.git.as_ref());
    let p_wt = project.and_then(|p| p.worktrees.as_ref());

    let g_ui = global.ui.as_ref();
    let g_git = global.git.as_ref();
    let g_wt = global.worktrees.as_ref();

    // Hooks: project replaces global entirely (FR-2)
    let p_hooks = project.and_then(|p| p.hooks.as_ref());
    let hooks = p_hooks.or(global.hooks.as_ref()).cloned();

    ResolvedConfig {
        ui: ResolvedUiConfig {
            theme: p_ui
                .and_then(|u| u.theme.clone())
                .or_else(|| g_ui.and_then(|u| u.theme.clone()))
                .unwrap_or(defaults_ui.theme),
            date_format: p_ui
                .and_then(|u| u.date_format.clone())
                .or_else(|| g_ui.and_then(|u| u.date_format.clone()))
                .unwrap_or(defaults_ui.date_format),
            show_ahead_behind: p_ui
                .and_then(|u| u.show_ahead_behind)
                .or_else(|| g_ui.and_then(|u| u.show_ahead_behind))
                .unwrap_or(defaults_ui.show_ahead_behind),
            show_dirty_count: p_ui
                .and_then(|u| u.show_dirty_count)
                .or_else(|| g_ui.and_then(|u| u.show_dirty_count))
                .unwrap_or(defaults_ui.show_dirty_count),
        },
        git: ResolvedGitConfig {
            default_base: cli
                .and_then(|c| c.default_base.clone())
                .or_else(|| p_git.and_then(|g| g.default_base.clone()))
                .or_else(|| g_git.and_then(|g| g.default_base.clone()))
                .unwrap_or(defaults_git.default_base),
            auto_prune: p_git
                .and_then(|g| g.auto_prune)
                .or_else(|| g_git.and_then(|g| g.auto_prune))
                .unwrap_or(defaults_git.auto_prune),
            fetch_on_open: p_git
                .and_then(|g| g.fetch_on_open)
                .or_else(|| g_git.and_then(|g| g.fetch_on_open))
                .unwrap_or(defaults_git.fetch_on_open),
        },
        worktrees: ResolvedWorktreesConfig {
            root: cli
                .and_then(|c| c.worktree_root.clone())
                .or_else(|| p_wt.and_then(|w| w.root.clone()))
                .or_else(|| g_wt.and_then(|w| w.root.clone()))
                .unwrap_or(defaults_wt.root),
            scan: p_wt
                .and_then(|w| w.scan.clone())
                .or_else(|| g_wt.and_then(|w| w.scan.clone()))
                .unwrap_or(defaults_wt.scan),
        },
        hooks,
    }
}

const PROJECT_CONFIG_FILENAME: &str = ".trench.toml";

/// Load project config from the repo root directory.
///
/// Looks for `.trench.toml` at the given repo root path.
/// Returns `Ok(None)` if the file does not exist.
pub fn load_project_config(repo_root: &Path) -> Result<Option<ProjectConfig>> {
    let path = repo_root.join(PROJECT_CONFIG_FILENAME);
    load_project_config_from(&path)
}

/// Load global config from a specific file path.
///
/// Returns `GlobalConfig::default()` if the file does not exist.
/// Returns an error if the file exists but contains invalid TOML.
pub fn load_global_config_from(path: &Path) -> Result<GlobalConfig> {
    load_optional_toml(path).map(|opt| opt.unwrap_or_default())
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
    fn load_project_config_from_valid_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".trench.toml");
        std::fs::write(
            &path,
            r#"
[git]
default_base = "develop"

[hooks.post_create]
run = ["bun install"]
"#,
        )
        .unwrap();

        let config = load_project_config_from(&path)
            .expect("should not error")
            .expect("should return Some for existing file");

        assert_eq!(
            config.git.unwrap().default_base.as_deref(),
            Some("develop")
        );
        assert!(config.hooks.unwrap().post_create.is_some());
    }

    #[test]
    fn load_project_config_from_missing_file_returns_none() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.toml");

        let result = load_project_config_from(&path).expect("should not error");
        assert!(result.is_none(), "missing file should return None");
    }

    #[test]
    fn load_project_config_finds_trench_toml_at_repo_root() {
        let dir = TempDir::new().unwrap();
        // Init a git repo
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        // Write .trench.toml at repo root
        std::fs::write(
            dir.path().join(".trench.toml"),
            "[git]\ndefault_base = \"develop\"\n",
        )
        .unwrap();

        let config = load_project_config(dir.path())
            .expect("should not error")
            .expect("should find .trench.toml");

        assert_eq!(
            config.git.unwrap().default_base.as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn load_project_config_returns_none_when_no_trench_toml() {
        let dir = TempDir::new().unwrap();
        let _repo = git2::Repository::init(dir.path()).unwrap();

        let result = load_project_config(dir.path()).expect("should not error");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_defaults_only() {
        let resolved = resolve_config(None, None, &GlobalConfig::default());

        assert_eq!(resolved.ui.theme, "default");
        assert_eq!(resolved.ui.date_format, "%Y-%m-%d %H:%M");
        assert!(resolved.ui.show_ahead_behind);
        assert!(resolved.ui.show_dirty_count);

        assert_eq!(resolved.git.default_base, "main");
        assert!(!resolved.git.auto_prune);
        assert!(resolved.git.fetch_on_open);

        assert_eq!(
            resolved.worktrees.root,
            crate::paths::DEFAULT_WORKTREE_TEMPLATE
        );
        assert!(resolved.worktrees.scan.is_empty());

        assert!(resolved.hooks.is_none());
    }

    #[test]
    fn resolve_global_overrides_defaults() {
        let global = GlobalConfig {
            ui: Some(UiConfig {
                theme: Some("nord".to_string()),
                date_format: None,
                show_ahead_behind: Some(false),
                show_dirty_count: None,
            }),
            git: Some(GitConfig {
                default_base: Some("develop".to_string()),
                auto_prune: Some(true),
                fetch_on_open: None,
            }),
            worktrees: Some(WorktreesConfig {
                root: Some("custom/{{ repo }}/{{ branch }}".to_string()),
                scan: Some(vec!["/extra".to_string()]),
            }),
            hooks: None,
        };

        let resolved = resolve_config(None, None, &global);

        // Overridden fields
        assert_eq!(resolved.ui.theme, "nord");
        assert!(!resolved.ui.show_ahead_behind);
        assert_eq!(resolved.git.default_base, "develop");
        assert!(resolved.git.auto_prune);
        assert_eq!(
            resolved.worktrees.root,
            "custom/{{ repo }}/{{ branch }}"
        );
        assert_eq!(resolved.worktrees.scan, vec!["/extra".to_string()]);

        // Fallback to defaults
        assert_eq!(resolved.ui.date_format, "%Y-%m-%d %H:%M");
        assert!(resolved.ui.show_dirty_count);
        assert!(resolved.git.fetch_on_open);
    }

    #[test]
    fn resolve_project_overrides_global_non_hook_fields() {
        let global = GlobalConfig {
            ui: Some(UiConfig {
                theme: Some("dark".to_string()),
                date_format: Some("%d/%m/%Y".to_string()),
                show_ahead_behind: None,
                show_dirty_count: None,
            }),
            git: Some(GitConfig {
                default_base: Some("develop".to_string()),
                auto_prune: Some(true),
                fetch_on_open: None,
            }),
            worktrees: None,
            hooks: None,
        };

        let project = ProjectConfig {
            ui: Some(UiConfig {
                theme: Some("nord".to_string()),
                date_format: None, // not overridden — should fall through to global
                show_ahead_behind: Some(false),
                show_dirty_count: None,
            }),
            git: Some(GitConfig {
                default_base: Some("staging".to_string()),
                auto_prune: None, // fall through to global
                fetch_on_open: Some(false),
            }),
            worktrees: Some(WorktreesConfig {
                root: Some("proj/{{ repo }}/{{ branch }}".to_string()),
                scan: None,
            }),
            hooks: None,
        };

        let resolved = resolve_config(None, Some(&project), &global);

        // Project wins over global
        assert_eq!(resolved.ui.theme, "nord");
        assert!(!resolved.ui.show_ahead_behind);
        assert_eq!(resolved.git.default_base, "staging");
        assert!(!resolved.git.fetch_on_open);
        assert_eq!(resolved.worktrees.root, "proj/{{ repo }}/{{ branch }}");

        // Global fills in where project is None
        assert_eq!(resolved.ui.date_format, "%d/%m/%Y");
        assert!(resolved.git.auto_prune);

        // Default fills in where both are None
        assert!(resolved.ui.show_dirty_count);
        assert!(resolved.worktrees.scan.is_empty());
    }

    #[test]
    fn resolve_project_hooks_replace_global_hooks_entirely() {
        let global = GlobalConfig {
            hooks: Some(HooksConfig {
                post_create: Some(HookDef {
                    run: Some(vec!["npm install".to_string()]),
                    ..HookDef::default()
                }),
                pre_remove: Some(HookDef {
                    shell: Some("echo global-cleanup".to_string()),
                    ..HookDef::default()
                }),
                ..HooksConfig::default()
            }),
            ..GlobalConfig::default()
        };

        let project = ProjectConfig {
            hooks: Some(HooksConfig {
                post_create: Some(HookDef {
                    run: Some(vec!["bun install".to_string()]),
                    ..HookDef::default()
                }),
                // pre_remove intentionally missing in project
                ..HooksConfig::default()
            }),
            ..ProjectConfig::default()
        };

        let resolved = resolve_config(None, Some(&project), &global);

        let hooks = resolved.hooks.expect("hooks should be present");
        // Project's post_create wins
        let post_create = hooks.post_create.expect("post_create should exist");
        assert_eq!(post_create.run, Some(vec!["bun install".to_string()]));

        // Global's pre_remove is NOT carried over — project hooks replace entirely
        assert!(
            hooks.pre_remove.is_none(),
            "global pre_remove should not be merged into project hooks"
        );
    }

    #[test]
    fn resolve_global_hooks_used_when_project_has_no_hooks() {
        let global = GlobalConfig {
            hooks: Some(HooksConfig {
                post_create: Some(HookDef {
                    run: Some(vec!["npm install".to_string()]),
                    ..HookDef::default()
                }),
                ..HooksConfig::default()
            }),
            ..GlobalConfig::default()
        };

        let project = ProjectConfig {
            git: Some(GitConfig {
                default_base: Some("staging".to_string()),
                ..GitConfig::default()
            }),
            hooks: None, // no hooks in project
            ..ProjectConfig::default()
        };

        let resolved = resolve_config(None, Some(&project), &global);

        // Global hooks used because project has no hooks section
        let hooks = resolved.hooks.expect("global hooks should be used");
        assert!(hooks.post_create.is_some());
        assert_eq!(resolved.git.default_base, "staging");
    }

    #[test]
    fn resolve_cli_overrides_trump_everything() {
        let global = GlobalConfig {
            git: Some(GitConfig {
                default_base: Some("develop".to_string()),
                ..GitConfig::default()
            }),
            worktrees: Some(WorktreesConfig {
                root: Some("global/{{ repo }}".to_string()),
                scan: None,
            }),
            ..GlobalConfig::default()
        };

        let project = ProjectConfig {
            git: Some(GitConfig {
                default_base: Some("staging".to_string()),
                ..GitConfig::default()
            }),
            worktrees: Some(WorktreesConfig {
                root: Some("project/{{ repo }}".to_string()),
                scan: None,
            }),
            ..ProjectConfig::default()
        };

        let cli = CliConfigOverrides {
            default_base: Some("cli-branch".to_string()),
            worktree_root: Some("cli/{{ repo }}".to_string()),
        };

        let resolved = resolve_config(Some(&cli), Some(&project), &global);

        assert_eq!(resolved.git.default_base, "cli-branch");
        assert_eq!(resolved.worktrees.root, "cli/{{ repo }}");
    }

    #[test]
    fn resolve_cli_partial_overrides_fall_through() {
        let global = GlobalConfig {
            git: Some(GitConfig {
                default_base: Some("develop".to_string()),
                ..GitConfig::default()
            }),
            ..GlobalConfig::default()
        };

        let cli = CliConfigOverrides {
            default_base: None,
            worktree_root: Some("cli-root/{{ repo }}".to_string()),
        };

        let resolved = resolve_config(Some(&cli), None, &global);

        // CLI worktree_root wins
        assert_eq!(resolved.worktrees.root, "cli-root/{{ repo }}");
        // No CLI default_base → falls through to global
        assert_eq!(resolved.git.default_base, "develop");
    }

    #[test]
    fn resolve_no_hooks_anywhere() {
        let resolved = resolve_config(None, None, &GlobalConfig::default());
        assert!(resolved.hooks.is_none());
    }

    #[test]
    fn integration_temp_repo_with_trench_toml_full_chain() {
        // Set up a temp git repo with .trench.toml
        let repo_dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(repo_dir.path()).unwrap();
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }

        // Write .trench.toml at repo root
        std::fs::write(
            repo_dir.path().join(".trench.toml"),
            r#"
[git]
default_base = "develop"

[worktrees]
root = "project/{{ repo }}/{{ branch | sanitize }}"

[hooks.post_create]
copy = [".env"]
run = ["bun install"]
"#,
        )
        .unwrap();

        // Write a "global" config file
        let global_dir = TempDir::new().unwrap();
        let global_path = global_dir.path().join("config.toml");
        std::fs::write(
            &global_path,
            r#"
[ui]
theme = "solarized"
show_ahead_behind = false

[git]
default_base = "main"
auto_prune = true

[hooks.post_create]
run = ["npm install"]

[hooks.pre_remove]
shell = "echo global-cleanup"
"#,
        )
        .unwrap();

        // Load both configs
        let project = load_project_config(repo_dir.path())
            .expect("should load project config")
            .expect("project config should exist");

        let global = load_global_config_from(&global_path)
            .expect("should load global config");

        // Discover repo to verify git wiring
        let repo_info = crate::git::discover_repo(repo_dir.path())
            .expect("should discover repo");
        assert_eq!(repo_info.path, repo_dir.path().canonicalize().unwrap());

        // Resolve the full chain
        let resolved = resolve_config(None, Some(&project), &global);

        // Project git.default_base overrides global
        assert_eq!(resolved.git.default_base, "develop");

        // Global auto_prune fills in (project didn't set it)
        assert!(resolved.git.auto_prune);

        // Global UI fills in (project has no UI section)
        assert_eq!(resolved.ui.theme, "solarized");
        assert!(!resolved.ui.show_ahead_behind);

        // Defaults fill in for unset fields
        assert!(resolved.ui.show_dirty_count);
        assert!(resolved.git.fetch_on_open);

        // Project worktrees override global
        assert_eq!(
            resolved.worktrees.root,
            "project/{{ repo }}/{{ branch | sanitize }}"
        );

        // Project hooks REPLACE global hooks entirely (FR-2)
        let hooks = resolved.hooks.expect("hooks should be present");
        let post_create = hooks.post_create.expect("post_create should exist");
        assert_eq!(post_create.run, Some(vec!["bun install".to_string()]));
        assert_eq!(post_create.copy, Some(vec![".env".to_string()]));

        // Global pre_remove NOT present — project hooks replace entirely
        assert!(
            hooks.pre_remove.is_none(),
            "global pre_remove should not bleed through"
        );
    }

    #[test]
    fn global_config_with_hooks_deserializes() {
        let dir = TempDir::new().unwrap();
        let path = write_config(
            &dir,
            r#"
[git]
default_base = "main"

[hooks.post_create]
copy = [".env"]
run = ["npm install"]

[hooks.pre_remove]
shell = "echo cleanup"
"#,
        );

        let config = load_global_config_from(&path).unwrap();

        let hooks = config.hooks.expect("hooks should be present");
        let post_create = hooks.post_create.expect("post_create should exist");
        assert_eq!(post_create.copy, Some(vec![".env".to_string()]));
        assert_eq!(post_create.run, Some(vec!["npm install".to_string()]));

        let pre_remove = hooks.pre_remove.expect("pre_remove should exist");
        assert_eq!(pre_remove.shell.as_deref(), Some("echo cleanup"));
    }

    #[test]
    fn load_project_config_from_invalid_toml_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".trench.toml");
        std::fs::write(&path, "not valid [toml").unwrap();

        let err = load_project_config_from(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid TOML"), "error should mention 'invalid TOML': {msg}");
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
