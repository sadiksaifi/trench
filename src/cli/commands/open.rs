use std::path::Path;

use anyhow::Result;

use crate::state::Database;

/// Result of resolving the open command (before actually launching the editor).
#[derive(Debug)]
pub struct OpenResult {
    /// Sanitized name of the worktree.
    pub name: String,
    /// Absolute path to the worktree.
    pub path: String,
    /// Editor command that should be used to open the worktree.
    pub editor: String,
    /// Repo ID (for deferred DB writes after editor launch).
    pub repo_id: i64,
    /// Worktree ID (for deferred DB writes after editor launch).
    pub wt_id: i64,
}

/// Resolve the editor command from the fallback chain:
/// config override → $EDITOR → $VISUAL → error.
fn resolve_editor(config_editor: Option<&str>) -> Result<String> {
    if let Some(cmd) = config_editor.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(cmd.to_string());
    }
    if let Ok(editor) = std::env::var("EDITOR") {
        let trimmed = editor.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Ok(visual) = std::env::var("VISUAL") {
        let trimmed = visual.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    anyhow::bail!(
        "no editor configured. Set $EDITOR, $VISUAL, or add [editor] command = \"...\" to your config"
    )
}

/// Resolve the worktree and editor for `trench open <identifier>`.
///
/// Does NOT launch the editor — returns the resolved information so the
/// caller (or tests) can decide what to do with it.
pub fn resolve(
    identifier: &str,
    cwd: &Path,
    db: &Database,
    config_editor: Option<&str>,
) -> Result<OpenResult> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db
        .get_repo_by_path(repo_path_str)?
        .ok_or_else(|| anyhow::anyhow!("repository not tracked by trench"))?;

    // Try the identifier as-is first, then try sanitizing it
    let wt = match db.find_worktree_by_identifier(repo.id, identifier)? {
        Some(wt) => wt,
        None => {
            let sanitized = crate::paths::sanitize_branch(identifier);
            if sanitized != identifier {
                db.find_worktree_by_identifier(repo.id, &sanitized)?
            } else {
                None
            }
            .ok_or_else(|| anyhow::anyhow!("worktree not found: {identifier}"))?
        }
    };

    let editor = resolve_editor(config_editor)?;

    Ok(OpenResult {
        name: wt.name.clone(),
        path: wt.path.clone(),
        editor,
        repo_id: repo.id,
        wt_id: wt.id,
    })
}

/// Record a successful open: update last_accessed and insert an "opened" event.
///
/// Call this only after the editor has exited successfully.
pub fn record_open(db: &Database, repo_id: i64, wt_id: i64) -> Result<()> {
    let now = crate::state::unix_epoch_secs() as i64;
    db.update_worktree(
        wt_id,
        &crate::state::WorktreeUpdate {
            last_accessed: Some(Some(now)),
            ..Default::default()
        },
    )?;
    db.insert_event(repo_id, Some(wt_id), "opened", None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;
    use std::ffi::OsString;
    use std::path::Path;

    /// RAII guard that saves the current value of an env var and restores it on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let prev = std::env::var_os(key);
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn init_repo_with_commit(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).expect("failed to init repo");
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
                .unwrap();
        }
        repo
    }

    #[test]
    fn resolve_returns_worktree_path_and_config_editor() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let result = resolve("my-feature", repo_dir.path(), &db, Some("code")).unwrap();

        assert_eq!(result.name, "my-feature");
        assert_eq!(result.path, "/wt/my-feature");
        assert_eq!(result.editor, "code");
    }

    #[test]
    #[serial_test::serial]
    fn resolve_uses_editor_env_when_no_config() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let _editor = EnvGuard::set("EDITOR", Some("vim"));
        let _visual = EnvGuard::set("VISUAL", None);
        let result = resolve("my-feature", repo_dir.path(), &db, None).unwrap();

        assert_eq!(result.editor, "vim");
    }

    #[test]
    #[serial_test::serial]
    fn resolve_uses_visual_env_when_no_editor() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let _editor = EnvGuard::set("EDITOR", None);
        let _visual = EnvGuard::set("VISUAL", Some("nano"));
        let result = resolve("my-feature", repo_dir.path(), &db, None).unwrap();

        assert_eq!(result.editor, "nano");
    }

    #[test]
    #[serial_test::serial]
    fn resolve_errors_when_no_editor_available() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let _editor = EnvGuard::set("EDITOR", None);
        let _visual = EnvGuard::set("VISUAL", None);
        let err = resolve("my-feature", repo_dir.path(), &db, None).unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("no editor configured"),
            "error should mention 'no editor configured', got: {msg}"
        );
    }

    #[test]
    #[serial_test::serial]
    fn config_editor_overrides_env_vars() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let _editor = EnvGuard::set("EDITOR", Some("vim"));
        let _visual = EnvGuard::set("VISUAL", Some("nano"));
        let result = resolve("my-feature", repo_dir.path(), &db, Some("code")).unwrap();

        assert_eq!(result.editor, "code", "config should override env vars");
    }

    #[test]
    fn resolve_not_found_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        let err = resolve("nonexistent", repo_dir.path(), &db, Some("vim")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "error should mention 'not found', got: {msg}"
        );
    }

    #[test]
    fn resolve_does_not_write_db() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        let wt = db
            .insert_worktree(
                db_repo.id,
                "my-feature",
                "my-feature",
                "/wt/my-feature",
                Some("main"),
            )
            .unwrap();

        resolve("my-feature", repo_dir.path(), &db, Some("vim")).unwrap();

        // resolve() must NOT touch the DB — no last_accessed update, no event
        let unchanged = db.get_worktree(wt.id).unwrap().unwrap();
        assert!(
            unchanged.last_accessed.is_none(),
            "resolve should not update last_accessed"
        );
        let event_count = db.count_events(wt.id, Some("opened")).unwrap();
        assert_eq!(event_count, 0, "resolve should not insert events");
    }

    #[test]
    fn record_open_updates_last_accessed_and_event() {
        let db = Database::open_in_memory().unwrap();
        let db_repo = db
            .insert_repo("my-project", "/tmp/fake", Some("main"))
            .unwrap();
        let wt = db
            .insert_worktree(
                db_repo.id,
                "my-feature",
                "my-feature",
                "/wt/my-feature",
                Some("main"),
            )
            .unwrap();

        assert!(wt.last_accessed.is_none());

        record_open(&db, db_repo.id, wt.id).unwrap();

        let updated = db.get_worktree(wt.id).unwrap().unwrap();
        assert!(updated.last_accessed.is_some());
        assert!(updated.last_accessed.unwrap() > 0);

        let event_count = db.count_events(wt.id, Some("opened")).unwrap();
        assert_eq!(event_count, 1, "exactly one 'opened' event should exist");
    }

    #[test]
    #[serial_test::serial]
    fn resolve_editor_trims_whitespace_config() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let _editor = EnvGuard::set("EDITOR", None);
        let _visual = EnvGuard::set("VISUAL", None);

        // Whitespace-only config should fall through → error
        let err = resolve("my-feature", repo_dir.path(), &db, Some("   ")).unwrap_err();
        assert!(
            err.to_string().contains("no editor configured"),
            "whitespace-only config should fall through, got: {}",
            err
        );
    }

    #[test]
    #[serial_test::serial]
    fn resolve_editor_trims_empty_config() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        let _editor = EnvGuard::set("EDITOR", None);
        let _visual = EnvGuard::set("VISUAL", None);

        // Empty config should fall through → error
        let err = resolve("my-feature", repo_dir.path(), &db, Some("")).unwrap_err();
        assert!(
            err.to_string().contains("no editor configured"),
            "empty config should fall through, got: {}",
            err
        );
    }

    #[test]
    #[serial_test::serial]
    fn resolve_editor_trims_whitespace_env() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "my-feature",
            "my-feature",
            "/wt/my-feature",
            Some("main"),
        )
        .unwrap();

        // Whitespace-only EDITOR should fall through to VISUAL
        let _editor = EnvGuard::set("EDITOR", Some("  \t "));
        let _visual = EnvGuard::set("VISUAL", Some("nano"));
        let result = resolve("my-feature", repo_dir.path(), &db, None).unwrap();

        assert_eq!(result.editor, "nano");
    }

    #[test]
    fn resolve_by_branch_with_slash() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db
            .insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "feature-auth",
            "feature/auth",
            "/wt/feature-auth",
            Some("main"),
        )
        .unwrap();

        // Resolve using original branch name (with slash)
        let result = resolve("feature/auth", repo_dir.path(), &db, Some("vim")).unwrap();
        assert_eq!(result.name, "feature-auth");
        assert_eq!(result.path, "/wt/feature-auth");

        // Resolve using sanitized name
        let result = resolve("feature-auth", repo_dir.path(), &db, Some("vim")).unwrap();
        assert_eq!(result.name, "feature-auth");
    }
}
