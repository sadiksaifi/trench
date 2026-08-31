//! Central filesystem path policy for trench.
//!
//! Writable application directories follow the XDG Base Directory specification
//! on every supported platform. Consumers should use this module instead of
//! reading XDG environment variables or joining application paths directly.

use std::path::PathBuf;

use anyhow::{Context, Result};

const APP_NAME: &str = "trench";
const CONFIG_FILENAME: &str = "config.toml";
const LOG_FILENAME: &str = "trench.log";

#[derive(Clone, Copy)]
struct XdgHome {
    env_var: &'static str,
    default: &'static str,
}

const XDG_CONFIG_HOME: XdgHome = XdgHome {
    env_var: "XDG_CONFIG_HOME",
    default: ".config",
};
const XDG_DATA_HOME: XdgHome = XdgHome {
    env_var: "XDG_DATA_HOME",
    default: ".local/share",
};
const XDG_STATE_HOME: XdgHome = XdgHome {
    env_var: "XDG_STATE_HOME",
    default: ".local/state",
};
const XDG_CACHE_HOME: XdgHome = XdgHome {
    env_var: "XDG_CACHE_HOME",
    default: ".cache",
};

fn home_dir_path() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .context("HOME must be set to an absolute path")
}

fn env_dir_path(env_var: &str) -> Option<PathBuf> {
    std::env::var_os(env_var).and_then(|value| {
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    })
}

fn xdg_home(env_var: &str, default: &str) -> Result<PathBuf> {
    match env_dir_path(env_var) {
        Some(path) => Ok(path),
        None => Ok(home_dir_path()?.join(default)),
    }
}

fn app_dir_path(xdg: XdgHome) -> Result<PathBuf> {
    Ok(xdg_home(xdg.env_var, xdg.default)?.join(APP_NAME))
}

/// Return the application config directory without creating it.
pub fn config_dir_path() -> Result<PathBuf> {
    app_dir_path(XDG_CONFIG_HOME)
}

/// Return the application data directory without creating it.
#[allow(dead_code)] // Trench is stateless today; keep the XDG contract centralized for future data.
pub fn data_dir_path() -> Result<PathBuf> {
    app_dir_path(XDG_DATA_HOME)
}

/// Return the application state directory without creating it.
pub fn state_dir_path() -> Result<PathBuf> {
    app_dir_path(XDG_STATE_HOME)
}

/// Return the application cache directory without creating it.
#[allow(dead_code)] // Trench has no cache today; keep the XDG contract centralized for future cache.
pub fn cache_dir_path() -> Result<PathBuf> {
    app_dir_path(XDG_CACHE_HOME)
}

pub fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir_path()?.join(CONFIG_FILENAME))
}

pub fn log_file_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join(LOG_FILENAME))
}

pub fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Ok(home) = home_dir_path() {
            return if path == "~" {
                home.to_string_lossy().into_owned()
            } else {
                home.join(&path[2..]).to_string_lossy().into_owned()
            };
        }
    }
    path.to_string()
}

pub fn sanitize_branch(branch: &str) -> String {
    let stripped = branch.replace("..", "-");
    let mut result = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        match ch {
            '/' | '@' | ' ' | '-' if !result.ends_with('-') => result.push('-'),
            '/' | '@' | ' ' | '-' => {}
            _ => result.push(ch),
        }
    }
    result.trim_matches('-').to_string()
}

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
    use std::ffi::OsString;

    use serial_test::serial;
    use tempfile::TempDir;

    use super::*;

    const XDG_ENV_VARS: [&str; 4] = [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ];

    struct EnvironmentGuard(Vec<(&'static str, Option<OsString>)>);

    impl EnvironmentGuard {
        fn capture(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvironmentGuard {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    #[serial]
    fn xdg_defaults_require_home() {
        let _guard = EnvironmentGuard::capture(&[
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ]);
        std::env::remove_var("HOME");
        for name in XDG_ENV_VARS {
            std::env::remove_var(name);
        }

        assert!(config_dir_path().is_err());
        assert!(data_dir_path().is_err());
        assert!(state_dir_path().is_err());
        assert!(cache_dir_path().is_err());
    }

    #[test]
    #[serial]
    fn absolute_xdg_home_values_override_defaults() {
        let root = TempDir::new().unwrap();
        let _guard = EnvironmentGuard::capture(&[
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ]);
        std::env::set_var("XDG_CONFIG_HOME", root.path().join("config"));
        std::env::set_var("XDG_DATA_HOME", root.path().join("data"));
        std::env::set_var("XDG_STATE_HOME", root.path().join("state"));
        std::env::set_var("XDG_CACHE_HOME", root.path().join("cache"));

        assert_eq!(
            config_dir_path().unwrap(),
            root.path().join("config/trench")
        );
        assert_eq!(data_dir_path().unwrap(), root.path().join("data/trench"));
        assert_eq!(state_dir_path().unwrap(), root.path().join("state/trench"));
        assert_eq!(cache_dir_path().unwrap(), root.path().join("cache/trench"));
    }

    #[test]
    #[serial]
    fn relative_xdg_home_values_are_ignored() {
        let home = TempDir::new().unwrap();
        let _guard = EnvironmentGuard::capture(&[
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ]);
        std::env::set_var("HOME", home.path());
        for name in XDG_ENV_VARS {
            std::env::set_var(name, "relative/path");
        }

        assert_eq!(
            config_dir_path().unwrap(),
            home.path().join(".config/trench")
        );
        assert_eq!(
            data_dir_path().unwrap(),
            home.path().join(".local/share/trench")
        );
        assert_eq!(
            state_dir_path().unwrap(),
            home.path().join(".local/state/trench")
        );
        assert_eq!(cache_dir_path().unwrap(), home.path().join(".cache/trench"));
    }

    #[test]
    #[serial]
    fn xdg_app_directories_use_standard_home_defaults() {
        let home = TempDir::new().unwrap();
        let _guard = EnvironmentGuard::capture(&[
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ]);
        std::env::set_var("HOME", home.path());
        for name in XDG_ENV_VARS {
            std::env::remove_var(name);
        }

        assert_eq!(
            config_dir_path().unwrap(),
            home.path().join(".config/trench")
        );
        assert_eq!(
            data_dir_path().unwrap(),
            home.path().join(".local/share/trench")
        );
        assert_eq!(
            state_dir_path().unwrap(),
            home.path().join(".local/state/trench")
        );
        assert_eq!(cache_dir_path().unwrap(), home.path().join(".cache/trench"));
    }
}
