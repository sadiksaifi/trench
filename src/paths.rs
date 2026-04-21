use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const APP_NAME: &str = "trench";
const DEFAULT_WORKTREE_DIR: &str = ".worktrees";
const FALLBACK_WORKTREE_DIR: &str = "trench-worktrees";
const CONFIG_FILENAME: &str = "config.toml";
const DATABASE_FILENAME: &str = "trench.db";
const LOG_FILENAME: &str = "trench.log";
const CONFIG_BASE_SEGMENTS: &[&str] = &[".config"];
const DATA_BASE_SEGMENTS: &[&str] = &[".local", "share"];
const STATE_BASE_SEGMENTS: &[&str] = &[".local", "state"];
const CACHE_BASE_SEGMENTS: &[&str] = &[".cache"];

/// XDG environment variable honored for config files on every platform.
pub const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
/// XDG environment variable honored for persistent app data on every platform.
pub const XDG_DATA_HOME_ENV: &str = "XDG_DATA_HOME";
/// XDG environment variable honored for logs and state on every platform.
pub const XDG_STATE_HOME_ENV: &str = "XDG_STATE_HOME";
/// XDG environment variable honored for cache files on every platform.
pub const XDG_CACHE_HOME_ENV: &str = "XDG_CACHE_HOME";

/// Ensure a directory exists, creating it (and parents) if needed.
fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory: {}", path.display()))?;
    Ok(())
}

fn dir_is_writable(path: &Path) -> bool {
    let probe = path.join(format!(".write-test-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

fn runtime_data_dir_fallback() -> PathBuf {
    std::env::temp_dir()
        .join(APP_NAME)
        .join(format!("cwd-{:016x}", current_dir_hash()))
}

pub(crate) fn data_dir_fallback_path() -> PathBuf {
    runtime_data_dir_fallback()
}

fn runtime_worktree_root_fallback() -> PathBuf {
    std::env::temp_dir()
        .join(FALLBACK_WORKTREE_DIR)
        .join(format!("cwd-{:016x}", current_dir_hash()))
}

fn current_dir_hash() -> u64 {
    let mut hasher = DefaultHasher::new();
    match std::env::current_dir() {
        Ok(cwd) => cwd.hash(&mut hasher),
        Err(_) => APP_NAME.hash(&mut hasher),
    }
    hasher.finish()
}

fn ensure_dir_with_fallback(path: &Path) -> Result<PathBuf> {
    if ensure_dir(path).is_ok() && dir_is_writable(path) {
        return Ok(path.to_path_buf());
    }

    let fallback = runtime_data_dir_fallback();
    ensure_dir(&fallback)?;
    Ok(fallback)
}

fn home_dir_path() -> Result<PathBuf> {
    dirs::home_dir().context("could not determine home directory")
}

fn env_dir_path(env_var: &str) -> Option<PathBuf> {
    std::env::var_os(env_var).and_then(|value| {
        let path = PathBuf::from(value);
        (!path.as_os_str().is_empty()).then_some(path)
    })
}

fn home_dir_with_segments(segments: &[&str]) -> Result<PathBuf> {
    Ok(segments
        .iter()
        .fold(home_dir_path()?, |path, segment| path.join(segment)))
}

#[cfg(target_os = "windows")]
fn base_dir_path(
    env_var: &str,
    native: Option<PathBuf>,
    unix_segments: &[&str],
) -> Result<PathBuf> {
    if let Some(path) = env_dir_path(env_var) {
        return Ok(path);
    }
    native
        .or_else(|| Some(home_dir_with_segments(unix_segments).ok()?))
        .context(format!("could not determine base directory for {env_var}"))
}

#[cfg(not(target_os = "windows"))]
fn base_dir_path(
    env_var: &str,
    _native: Option<PathBuf>,
    unix_segments: &[&str],
) -> Result<PathBuf> {
    if let Some(path) = env_dir_path(env_var) {
        return Ok(path);
    }
    home_dir_with_segments(unix_segments)
}

fn config_base_dir_path() -> Result<PathBuf> {
    base_dir_path(
        XDG_CONFIG_HOME_ENV,
        dirs::config_dir(),
        CONFIG_BASE_SEGMENTS,
    )
}

fn data_base_dir_path() -> Result<PathBuf> {
    base_dir_path(XDG_DATA_HOME_ENV, dirs::data_dir(), DATA_BASE_SEGMENTS)
}

fn state_base_dir_path() -> Result<PathBuf> {
    base_dir_path(XDG_STATE_HOME_ENV, dirs::state_dir(), STATE_BASE_SEGMENTS)
}

fn cache_base_dir_path() -> Result<PathBuf> {
    base_dir_path(XDG_CACHE_HOME_ENV, dirs::cache_dir(), CACHE_BASE_SEGMENTS)
}

fn db_file_is_accessible(path: &Path) -> bool {
    path.exists()
        && std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .is_ok()
}

/// Central path policy for trench.
///
/// All runtime code should resolve config/data/state/cache files through this
/// module instead of calling `dirs` directly or hand-joining filenames.
///
/// Defaults:
/// - Linux/macOS: XDG-style home layout
/// - Windows: native platform dirs, unless an explicit `XDG_*_HOME` override exists

/// Return the trench config directory path without creating it.
///
/// Use this in read-only contexts (e.g. config loading, `--dry-run`) where no
/// side effects are allowed. For contexts that need the directory to exist,
/// use [`config_dir`].
pub fn config_dir_path() -> Result<PathBuf> {
    let path = config_base_dir_path()?.join(APP_NAME);
    Ok(path)
}

/// Return the trench config directory, creating it if needed.
pub fn config_dir() -> Result<PathBuf> {
    let path = config_dir_path()?;
    ensure_dir(&path)?;
    Ok(path)
}

/// Return the global config file path without creating parent directories.
pub fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir_path()?.join(CONFIG_FILENAME))
}

/// Return the trench data directory path without creating it.
///
/// Use this in read-only contexts (e.g. `--dry-run`) where no side effects
/// are allowed. For contexts that need the directory to exist, use [`data_dir`].
pub fn data_dir_path() -> Result<PathBuf> {
    Ok(data_base_dir_path()?.join(APP_NAME))
}

/// Return the trench data directory, creating it if needed.
pub fn data_dir() -> Result<PathBuf> {
    let path = data_dir_path()?;
    ensure_dir_with_fallback(&path)
}

/// Return the database file path without creating parent directories.
pub fn database_file_path() -> Result<PathBuf> {
    Ok(data_dir_path()?.join(DATABASE_FILENAME))
}

/// Return the writable runtime database file path.
///
/// Prefers the canonical data directory, falls back to the temp-based runtime
/// data directory when an existing writable DB is found there or the canonical
/// directory cannot be created.
pub fn runtime_database_file_path() -> Result<PathBuf> {
    let preferred = database_file_path()?;
    if db_file_is_accessible(&preferred) {
        return Ok(preferred);
    }

    let fallback = data_dir_fallback_path().join(DATABASE_FILENAME);
    if db_file_is_accessible(&fallback) {
        return Ok(fallback);
    }

    Ok(data_dir()?.join(DATABASE_FILENAME))
}

/// Return the trench state directory path without creating it.
pub fn state_dir_path() -> Result<PathBuf> {
    Ok(state_base_dir_path()?.join(APP_NAME))
}

/// Return the trench state directory, creating it if needed.
///
pub fn state_dir() -> Result<PathBuf> {
    let path = state_dir_path()?;
    ensure_dir(&path)?;
    Ok(path)
}

/// Return the log file path without creating parent directories.
pub fn log_file_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join(LOG_FILENAME))
}

/// Return the trench cache directory path without creating it.
pub fn cache_dir_path() -> Result<PathBuf> {
    Ok(cache_base_dir_path()?.join(APP_NAME))
}

/// Return the trench cache directory, creating it if needed.
pub fn cache_dir() -> Result<PathBuf> {
    let path = cache_dir_path()?;
    ensure_dir(&path)?;
    Ok(path)
}

/// Return the worktree root path (`~/.worktrees/`) without creating it on disk.
///
/// Use this in read-only contexts (e.g. `--dry-run`) where no side effects
/// are allowed. For real execution, use [`worktree_root`] which also creates
/// the directory.
pub fn worktree_root_path() -> Result<PathBuf> {
    let path = home_dir_path()?.join(DEFAULT_WORKTREE_DIR);
    Ok(path)
}

/// Return the worktree root directory (`~/.worktrees/`), creating it if needed.
pub fn worktree_root() -> Result<PathBuf> {
    let path = worktree_root_path()?;
    if ensure_dir(&path).is_ok() && dir_is_writable(&path) {
        return Ok(path);
    }

    let fallback = runtime_worktree_root_fallback();
    ensure_dir(&fallback)?;
    Ok(fallback)
}

/// Default worktree path template (FR-17).
pub const DEFAULT_WORKTREE_TEMPLATE: &str = "{{ repo }}/{{ branch | sanitize }}";

/// Render a worktree path template using minijinja.
///
/// The template receives `repo` and `branch` variables, and a `sanitize` filter
/// that applies branch name sanitization (FR-17).
///
/// Returns the rendered path relative to the worktree root.
pub fn render_worktree_path(template: &str, repo: &str, branch: &str) -> Result<PathBuf> {
    let mut env = minijinja::Environment::new();
    env.add_filter("sanitize", sanitize_branch);
    env.add_template("path", template)
        .context("invalid worktree path template")?;
    let tmpl = env.get_template("path").unwrap();
    let rendered = tmpl
        .render(minijinja::context! { repo => repo, branch => branch })
        .context("failed to render worktree path template")?;
    let path = PathBuf::from(rendered);
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("worktree path template must render a relative path without '..'");
    }
    Ok(path)
}

/// Expand a leading `~` or `~/` in a path string to the user's home directory.
///
/// Returns the original string unchanged if it doesn't start with `~` or if
/// the home directory cannot be determined.
pub fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = home_dir_path() {
            if path == "~" {
                return home.to_string_lossy().into_owned();
            }
            return home.join(&path[2..]).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Sanitize a branch name for use as a filesystem directory name.
///
/// Rules (FR-15, FR-16):
/// - `/` → `-`
/// - spaces → `-`
/// - `@` → `-`
/// - `..` → `-`
/// - consecutive dashes collapsed
/// - single dots preserved
pub fn sanitize_branch(branch: &str) -> String {
    // Replace `..` sequences (path traversal) with dash
    let stripped = branch.replace("..", "-");

    let mut result = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        match ch {
            '/' | '@' | ' ' => {
                // Replace with dash, but avoid consecutive dashes
                if !result.ends_with('-') {
                    result.push('-');
                }
            }
            '-' => {
                if !result.ends_with('-') {
                    result.push('-');
                }
            }
            _ => result.push(ch),
        }
    }

    // Trim leading/trailing dashes
    result.trim_matches('-').to_string()
}

/// Validate a branch name against git ref naming rules.
///
/// Returns `Ok(())` if the name is valid, or `Err(reason)` describing why it's invalid.
pub fn validate_branch_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Branch name is required".into());
    }
    if trimmed != name {
        return Err("Branch name cannot have leading or trailing whitespace".into());
    }
    if trimmed.contains("..") {
        return Err("Branch name cannot contain '..'".into());
    }
    if trimmed.ends_with(".lock") {
        return Err("Branch name cannot end with '.lock'".into());
    }
    if trimmed.starts_with('.') || trimmed.ends_with('.') {
        return Err("Branch name cannot start or end with '.'".into());
    }
    let invalid_chars = [' ', '~', '^', ':', '?', '*', '[', '\\'];
    for ch in trimmed.chars() {
        if ch.is_ascii_control() {
            return Err("Branch name cannot contain control characters".into());
        }
        if invalid_chars.contains(&ch) {
            return Err(format!("Branch name cannot contain '{ch}'"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_runtime_data_dir_path(path: &Path) -> bool {
        path == data_dir_fallback_path().as_path()
    }

    fn is_runtime_worktree_root_path(path: &Path) -> bool {
        path == runtime_worktree_root_fallback().as_path()
    }

    fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn config_dir_ends_with_trench() {
        let path = config_dir().unwrap();
        assert!(path.ends_with("trench"));
        assert!(path.starts_with(config_base_dir_path().unwrap()));
        assert!(path.exists());
    }

    #[test]
    fn data_dir_ends_with_trench() {
        let path = data_dir().unwrap();
        assert!(
            path.ends_with("trench") || is_runtime_data_dir_path(&path),
            "unexpected data dir: {}",
            path.display()
        );
        assert!(
            path.starts_with(data_base_dir_path().unwrap()) || is_runtime_data_dir_path(&path),
            "unexpected data dir base: {}",
            path.display()
        );
        assert!(path.exists());
    }

    #[test]
    fn state_dir_ends_with_trench() {
        let path = state_dir().unwrap();
        assert!(path.ends_with("trench"));
        let expected_base = state_base_dir_path().unwrap();
        assert!(path.starts_with(expected_base));
        assert!(path.exists());
    }

    #[test]
    fn worktree_root_is_dot_worktrees() {
        let path = worktree_root().unwrap();
        assert!(
            path.ends_with(".worktrees") || is_runtime_worktree_root_path(&path),
            "unexpected worktree root: {}",
            path.display()
        );
        assert!(
            path.starts_with(dirs::home_dir().unwrap()) || is_runtime_worktree_root_path(&path),
            "unexpected worktree root base: {}",
            path.display()
        );
        assert!(path.exists());
    }

    #[test]
    fn worktree_root_path_returns_path_without_creating_it() {
        // worktree_root_path() should return the same path as worktree_root()
        // but must NOT create the directory. We can't easily test non-creation
        // on a real home dir (it likely already exists), so we verify the
        // function exists and returns the expected path shape.
        let path = worktree_root_path().unwrap();
        assert!(path.ends_with(".worktrees"));
        assert!(path.starts_with(dirs::home_dir().unwrap()));
    }

    #[test]
    fn config_dir_path_returns_path_without_creating_it() {
        let path = config_dir_path().unwrap();
        assert!(path.ends_with("trench"));
        assert!(path.starts_with(config_base_dir_path().unwrap()));
    }

    #[test]
    fn config_dir_path_prefers_xdg_config_home() {
        let original = std::env::var_os(XDG_CONFIG_HOME_ENV);
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(XDG_CONFIG_HOME_ENV, tmp.path());

        let path = config_dir_path().unwrap();

        restore_env(XDG_CONFIG_HOME_ENV, original);

        assert_eq!(path, tmp.path().join(APP_NAME));
    }

    #[test]
    fn data_dir_path_returns_path_without_creating_it() {
        let path = data_dir_path().unwrap();
        assert!(path.ends_with("trench"));
        assert!(path.starts_with(data_base_dir_path().unwrap()));
    }

    #[test]
    fn data_dir_path_prefers_xdg_data_home() {
        let original = std::env::var_os(XDG_DATA_HOME_ENV);
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(XDG_DATA_HOME_ENV, tmp.path());

        let path = data_dir_path().unwrap();

        restore_env(XDG_DATA_HOME_ENV, original);

        assert_eq!(path, tmp.path().join(APP_NAME));
    }

    #[test]
    fn state_dir_path_prefers_xdg_state_home() {
        let original = std::env::var_os(XDG_STATE_HOME_ENV);
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(XDG_STATE_HOME_ENV, tmp.path());

        let path = state_dir_path().unwrap();

        restore_env(XDG_STATE_HOME_ENV, original);

        assert_eq!(path, tmp.path().join(APP_NAME));
    }

    #[test]
    fn cache_dir_path_prefers_xdg_cache_home() {
        let original = std::env::var_os(XDG_CACHE_HOME_ENV);
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var(XDG_CACHE_HOME_ENV, tmp.path());

        let path = cache_dir_path().unwrap();

        restore_env(XDG_CACHE_HOME_ENV, original);

        assert_eq!(path, tmp.path().join(APP_NAME));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn data_dir_path_defaults_to_xdg_home_layout() {
        let original = std::env::var_os(XDG_DATA_HOME_ENV);
        std::env::remove_var(XDG_DATA_HOME_ENV);

        let path = data_dir_path().unwrap();

        restore_env(XDG_DATA_HOME_ENV, original);

        assert_eq!(
            path,
            dirs::home_dir()
                .unwrap()
                .join(".local")
                .join("share")
                .join(APP_NAME)
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn state_dir_path_defaults_to_xdg_home_layout() {
        let original = std::env::var_os(XDG_STATE_HOME_ENV);
        std::env::remove_var(XDG_STATE_HOME_ENV);

        let path = state_dir_path().unwrap();

        restore_env(XDG_STATE_HOME_ENV, original);

        assert_eq!(
            path,
            dirs::home_dir()
                .unwrap()
                .join(".local")
                .join("state")
                .join(APP_NAME)
        );
    }

    #[test]
    fn file_helpers_use_canonical_app_dirs() {
        assert_eq!(
            config_file_path().unwrap(),
            config_dir_path().unwrap().join("config.toml")
        );
        assert_eq!(
            database_file_path().unwrap(),
            data_dir_path().unwrap().join("trench.db")
        );
        assert_eq!(
            log_file_path().unwrap(),
            state_dir_path().unwrap().join("trench.log")
        );
    }

    #[test]
    fn render_default_template_with_repo_and_branch() {
        let path =
            render_worktree_path(DEFAULT_WORKTREE_TEMPLATE, "my-project", "feature/auth").unwrap();
        assert_eq!(path, PathBuf::from("my-project/feature-auth"));
    }

    #[test]
    fn render_custom_template() {
        let tmpl = "projects/{{ repo }}/{{ branch | sanitize }}";
        let path = render_worktree_path(tmpl, "trench", "fix@home").unwrap();
        assert_eq!(path, PathBuf::from("projects/trench/fix-home"));
    }

    #[test]
    fn render_template_branch_without_sanitize_filter() {
        // Using {{ branch }} directly (no filter) should pass through raw
        let tmpl = "{{ repo }}/{{ branch }}";
        let path = render_worktree_path(tmpl, "trench", "feature/auth").unwrap();
        assert_eq!(path, PathBuf::from("trench/feature/auth"));
    }

    #[test]
    fn render_template_rejects_absolute_path() {
        let result = render_worktree_path("/absolute/{{ repo }}", "trench", "main");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("relative"),
            "expected 'relative' in error: {msg}"
        );
    }

    #[test]
    fn render_template_rejects_parent_dir() {
        let result = render_worktree_path("{{ repo }}/../../etc", "trench", "main");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("'..'"), "expected '..' in error: {msg}");
    }

    #[test]
    fn sanitize_slash_to_dash() {
        assert_eq!(sanitize_branch("feature/auth"), "feature-auth");
    }

    #[test]
    fn sanitize_at_to_dash() {
        assert_eq!(sanitize_branch("fix@home"), "fix-home");
    }

    #[test]
    fn sanitize_double_dots_stripped() {
        assert_eq!(sanitize_branch("a..b"), "a-b");
    }

    #[test]
    fn sanitize_consecutive_dashes_collapsed() {
        assert_eq!(sanitize_branch("a--b"), "a-b");
    }

    #[test]
    fn sanitize_single_dots_preserved() {
        assert_eq!(sanitize_branch("v2.1.3"), "v2.1.3");
    }

    #[test]
    fn sanitize_spaces_to_dash() {
        assert_eq!(sanitize_branch("my branch"), "my-branch");
    }

    #[test]
    fn sanitize_leading_trailing_dashes_trimmed() {
        assert_eq!(sanitize_branch("/leading"), "leading");
        assert_eq!(sanitize_branch("trailing/"), "trailing");
    }

    #[test]
    fn sanitize_empty_branch() {
        assert_eq!(sanitize_branch(""), "");
    }

    #[test]
    fn sanitize_single_dot() {
        assert_eq!(sanitize_branch("."), ".");
    }

    #[test]
    fn sanitize_triple_dots() {
        // "..." → ".." replaced with "-" → "-." → trim leading dash → "."
        assert_eq!(sanitize_branch("..."), ".");
    }

    #[test]
    fn sanitize_combined_edge_cases() {
        // Multiple replaceable chars in a row collapse to single dash
        assert_eq!(sanitize_branch("a/@b"), "a-b");
        // Empty after stripping
        assert_eq!(sanitize_branch(".."), "");
        // Nested double dots with other chars
        assert_eq!(
            sanitize_branch("feature/..secret/auth"),
            "feature-secret-auth"
        );
    }

    #[test]
    fn expand_tilde_replaces_home_prefix() {
        let expanded = expand_tilde("~/projects");
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            expanded,
            home.join("projects").to_string_lossy().to_string()
        );
    }

    #[test]
    fn expand_tilde_leaves_absolute_paths_unchanged() {
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
    }

    #[test]
    fn expand_tilde_leaves_relative_paths_unchanged() {
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    #[test]
    fn expand_tilde_bare_tilde_expands_to_home() {
        let expanded = expand_tilde("~");
        let home = dirs::home_dir().unwrap();
        assert_eq!(expanded, home.to_string_lossy().to_string());
    }

    #[test]
    fn validate_branch_name_accepts_valid() {
        assert!(validate_branch_name("feature-auth").is_ok());
        assert!(validate_branch_name("fix/login-bug").is_ok());
        assert!(validate_branch_name("my.branch").is_ok());
    }

    #[test]
    fn validate_branch_name_rejects_spaces() {
        assert!(validate_branch_name("foo bar").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_tilde() {
        assert!(validate_branch_name("branch~1").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_lock_suffix() {
        assert!(validate_branch_name("branch.lock").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_double_dots() {
        assert!(validate_branch_name("foo..bar").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_empty() {
        assert!(validate_branch_name("").is_err());
        assert!(validate_branch_name("   ").is_err());
    }

    #[test]
    fn validate_branch_name_rejects_leading_trailing_whitespace() {
        assert!(validate_branch_name(" feature").is_err());
        assert!(validate_branch_name("feature ").is_err());
        assert!(validate_branch_name(" feature ").is_err());
    }
}
