use std::path::PathBuf;

use anyhow::{Context, Result};

const APP_NAME: &str = "trench";
const DEFAULT_WORKTREE_DIR: &str = ".worktrees";

/// Ensure a directory exists, creating it (and parents) if needed.
fn ensure_dir(path: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("failed to create directory: {}", path.display()))?;
    Ok(())
}

/// Return the trench config directory (`~/.config/trench/`), creating it if needed.
pub fn config_dir() -> Result<PathBuf> {
    let path = dirs::config_dir()
        .context("could not determine config directory")?
        .join(APP_NAME);
    ensure_dir(&path)?;
    Ok(path)
}

/// Return the trench data directory (`~/.local/share/trench/`), creating it if needed.
pub fn data_dir() -> Result<PathBuf> {
    let path = dirs::data_dir()
        .context("could not determine data directory")?
        .join(APP_NAME);
    ensure_dir(&path)?;
    Ok(path)
}

/// Return the trench state directory (`~/.local/state/trench/`), creating it if needed.
pub fn state_dir() -> Result<PathBuf> {
    let base = dirs::home_dir()
        .context("could not determine home directory")?
        .join(".local")
        .join("state");
    let path = base.join(APP_NAME);
    ensure_dir(&path)?;
    Ok(path)
}

/// Return the worktree root directory (`~/.worktrees/`), creating it if needed.
pub fn worktree_root() -> Result<PathBuf> {
    let path = dirs::home_dir()
        .context("could not determine home directory")?
        .join(DEFAULT_WORKTREE_DIR);
    ensure_dir(&path)?;
    Ok(path)
}

/// Sanitize a branch name for use as a filesystem directory name.
///
/// Rules (FR-15, FR-16):
/// - `/` → `-`
/// - spaces → `-`
/// - `@` → `-`
/// - `..` → stripped
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_ends_with_trench() {
        let path = config_dir().unwrap();
        assert!(path.ends_with("trench"));
        assert!(path.starts_with(dirs::config_dir().unwrap()));
        assert!(path.exists());
    }

    #[test]
    fn data_dir_ends_with_trench() {
        let path = data_dir().unwrap();
        assert!(path.ends_with("trench"));
        assert!(path.starts_with(dirs::data_dir().unwrap()));
        assert!(path.exists());
    }

    #[test]
    fn state_dir_ends_with_trench() {
        let path = state_dir().unwrap();
        assert!(path.ends_with("trench"));
        let home = dirs::home_dir().unwrap();
        assert!(path.starts_with(home.join(".local").join("state")));
        assert!(path.exists());
    }

    #[test]
    fn worktree_root_is_dot_worktrees() {
        let path = worktree_root().unwrap();
        assert!(path.ends_with(".worktrees"));
        assert!(path.starts_with(dirs::home_dir().unwrap()));
        assert!(path.exists());
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
    fn sanitize_combined_edge_cases() {
        // Multiple replaceable chars in a row collapse to single dash
        assert_eq!(sanitize_branch("a/@b"), "a-b");
        // Empty after stripping
        assert_eq!(sanitize_branch(".."), "");
        // Nested double dots with other chars
        assert_eq!(sanitize_branch("feature/..secret/auth"), "feature-secret-auth");
    }
}
