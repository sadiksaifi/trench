use std::path::{Path, PathBuf};

use anyhow::Result;

const TRENCH_TOML_FILENAME: &str = ".trench.toml";

/// Errors specific to the `init` command.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("`.trench.toml` already exists. Use `--force` to overwrite.")]
    FileAlreadyExists,
}

/// The scaffold content for `.trench.toml`.
const SCAFFOLD: &str = r#"# trench — project configuration
# Uncomment and modify the sections you need.
# This file is intended to be committed to version control.
#
# Configuration precedence:
#   CLI flags > .trench.toml > ~/.config/trench/config.toml > defaults

# ─── UI ──────────────────────────────────────────────────────────────

# [ui]
# theme = "default"
# date_format = "%Y-%m-%d %H:%M"
# show_ahead_behind = true
# show_dirty_count = true

# ─── Git ─────────────────────────────────────────────────────────────

# [git]
# default_base = "main"          # Base branch for new worktrees
# auto_prune = false              # Prune stale remote-tracking branches
# fetch_on_open = true            # Fetch from remote when opening a worktree

# ─── Worktrees ───────────────────────────────────────────────────────

# [worktrees]
# root = "{{ repo }}/{{ branch | sanitize }}"   # Path template for worktree dirs
# scan = []                                      # Extra directories to scan for worktrees

# ─── Hooks ───────────────────────────────────────────────────────────
#
# Six lifecycle hooks: pre_create, post_create, pre_sync, post_sync,
# pre_remove, post_remove.
#
# Each hook supports:
#   copy         — glob patterns to copy from repo root (prefix with ! to exclude)
#   run          — commands to execute sequentially
#   shell        — a shell script to run
#   timeout_secs — max seconds for run + shell combined (default: 120)
#
# Execution order within a hook: copy → run → shell
# If any step fails (non-zero exit), the hook stops.
#
# Pre-hooks cancel the operation on failure.
# Project hooks (.trench.toml) completely replace global hooks — no merging.

# [hooks.pre_create]
# run = []

# [hooks.post_create]
# copy = [".env*", "!.env.example"]
# run = ["bun install"]
# shell = ""
# timeout_secs = 300

# [hooks.pre_sync]
# run = []

# [hooks.post_sync]
# run = []

# [hooks.pre_remove]
# shell = "pkill -f 'next dev' || true"

# [hooks.post_remove]
# run = []
"#;

/// Execute `trench init` — scaffold a commented `.trench.toml` at the repo root.
pub fn execute(repo_root: &Path, force: bool) -> Result<PathBuf> {
    let path = repo_root.join(TRENCH_TOML_FILENAME);

    if path.exists() && !force {
        return Err(InitError::FileAlreadyExists.into());
    }

    std::fs::write(&path, SCAFFOLD)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_trench_toml_at_repo_root() {
        let dir = TempDir::new().unwrap();

        let result = execute(dir.path(), false);

        assert!(result.is_ok(), "init should succeed: {:?}", result.err());
        let created_path = result.unwrap();
        assert_eq!(created_path, dir.path().join(".trench.toml"));
        assert!(created_path.exists(), ".trench.toml should exist on disk");

        let contents = std::fs::read_to_string(&created_path).unwrap();
        assert!(!contents.is_empty(), "file should not be empty");
    }

    #[test]
    fn scaffold_contains_all_config_sections_commented_out() {
        let dir = TempDir::new().unwrap();
        let path = execute(dir.path(), false).unwrap();
        let contents = std::fs::read_to_string(path).unwrap();

        // All config sections should be present as comments
        assert!(contents.contains("# [ui]"), "should contain commented [ui] section");
        assert!(contents.contains("# [git]"), "should contain commented [git] section");
        assert!(contents.contains("# [worktrees]"), "should contain commented [worktrees] section");

        // All six hook sections
        assert!(contents.contains("# [hooks.pre_create]"), "should contain pre_create hook");
        assert!(contents.contains("# [hooks.post_create]"), "should contain post_create hook");
        assert!(contents.contains("# [hooks.pre_sync]"), "should contain pre_sync hook");
        assert!(contents.contains("# [hooks.post_sync]"), "should contain post_sync hook");
        assert!(contents.contains("# [hooks.pre_remove]"), "should contain pre_remove hook");
        assert!(contents.contains("# [hooks.post_remove]"), "should contain post_remove hook");

        // Key config fields should be documented
        assert!(contents.contains("# theme"), "should document theme");
        assert!(contents.contains("# default_base"), "should document default_base");
        assert!(contents.contains("# root"), "should document worktrees.root");

        // Hook fields should be documented
        assert!(contents.contains("# copy"), "should document hook copy field");
        assert!(contents.contains("# run"), "should document hook run field");
        assert!(contents.contains("# shell"), "should document hook shell field");
        assert!(contents.contains("# timeout_secs"), "should document hook timeout_secs");

        // Should have inline documentation
        assert!(contents.contains("Uncomment"), "should have usage instructions");
    }

    #[test]
    fn init_fails_when_file_already_exists() {
        let dir = TempDir::new().unwrap();
        let existing = dir.path().join(".trench.toml");
        std::fs::write(&existing, "# existing config\n").unwrap();

        let result = execute(dir.path(), false);

        assert!(result.is_err(), "init should fail when file exists");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("already exists"),
            "error should mention 'already exists': {msg}"
        );
        assert!(
            msg.contains("--force"),
            "error should mention --force: {msg}"
        );

        // Original file should be untouched
        let contents = std::fs::read_to_string(&existing).unwrap();
        assert_eq!(contents, "# existing config\n");
    }
}
