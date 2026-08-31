use std::path::PathBuf;

use anyhow::{Context, Result};

const APP_NAME: &str = "trench";
const CONFIG_FILENAME: &str = "config.toml";
const LOG_FILENAME: &str = "trench.log";

fn home_dir_path() -> Result<PathBuf> {
    dirs::home_dir().context("could not determine home directory")
}

fn env_dir_path(env_var: &str) -> Option<PathBuf> {
    std::env::var_os(env_var).and_then(|value| {
        let path = PathBuf::from(value);
        (!path.as_os_str().is_empty()).then_some(path)
    })
}

fn xdg_home(env_var: &str, default: &str) -> Result<PathBuf> {
    match env_dir_path(env_var) {
        Some(path) => Ok(path),
        None => Ok(home_dir_path()?.join(default)),
    }
}

pub fn config_file_path() -> Result<PathBuf> {
    Ok(xdg_home("XDG_CONFIG_HOME", ".config")?
        .join(APP_NAME)
        .join(CONFIG_FILENAME))
}

pub fn log_file_path() -> Result<PathBuf> {
    Ok(xdg_home("XDG_STATE_HOME", ".local/state")?
        .join(APP_NAME)
        .join(LOG_FILENAME))
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
