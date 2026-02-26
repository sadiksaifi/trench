use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const APP_NAME: &str = "trench";
const DEFAULT_WORKTREE_DIR: &str = ".worktrees";
/// Fallback path segments for platforms where `dirs::state_dir()` returns `None` (macOS/Windows).
const STATE_DIR_FALLBACK_SEGMENTS: &[&str] = &[".local", "state"];

/// Ensure a directory exists, creating it (and parents) if needed.
fn ensure_dir(path: &Path) -> Result<()> {
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
///
/// Uses `dirs::state_dir()` when available (Linux), falls back to
/// `~/.local/state` on platforms that return `None` (macOS/Windows).
pub fn state_dir() -> Result<PathBuf> {
    let base = dirs::state_dir().unwrap_or_else(|| {
        let home = dirs::home_dir().expect("could not determine home directory");
        STATE_DIR_FALLBACK_SEGMENTS.iter().fold(home, |p, s| p.join(s))
    });
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
    Ok(PathBuf::from(rendered))
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
        let expected_base = dirs::state_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap()
                .join(".local")
                .join("state")
        });
        assert!(path.starts_with(expected_base));
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
    fn render_default_template_with_repo_and_branch() {
        let path = render_worktree_path(DEFAULT_WORKTREE_TEMPLATE, "my-project", "feature/auth").unwrap();
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
        assert_eq!(sanitize_branch("feature/..secret/auth"), "feature-secret-auth");
    }
}
