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
}

/// Resolve the editor command from the fallback chain:
/// config override → $EDITOR → $VISUAL → error.
fn resolve_editor(config_editor: Option<&str>) -> Result<String> {
    if let Some(cmd) = config_editor {
        return Ok(cmd.to_string());
    }
    if let Ok(editor) = std::env::var("EDITOR") {
        if !editor.is_empty() {
            return Ok(editor);
        }
    }
    if let Ok(visual) = std::env::var("VISUAL") {
        if !visual.is_empty() {
            return Ok(visual);
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

    // Update last_accessed timestamp
    let now = crate::state::unix_epoch_secs() as i64;
    db.update_worktree(
        wt.id,
        &crate::state::WorktreeUpdate {
            last_accessed: Some(Some(now)),
            ..Default::default()
        },
    )?;

    // Record "opened" event
    db.insert_event(repo.id, Some(wt.id), "opened", None)?;

    Ok(OpenResult {
        name: wt.name.clone(),
        path: wt.path.clone(),
        editor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;
    use std::path::Path;

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
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
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
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        std::env::set_var("EDITOR", "vim");
        std::env::remove_var("VISUAL");
        let result = resolve("my-feature", repo_dir.path(), &db, None).unwrap();
        std::env::remove_var("EDITOR");

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
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        std::env::remove_var("EDITOR");
        std::env::set_var("VISUAL", "nano");
        let result = resolve("my-feature", repo_dir.path(), &db, None).unwrap();
        std::env::remove_var("VISUAL");

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
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        std::env::remove_var("EDITOR");
        std::env::remove_var("VISUAL");
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
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        std::env::set_var("EDITOR", "vim");
        std::env::set_var("VISUAL", "nano");
        let result = resolve("my-feature", repo_dir.path(), &db, Some("code")).unwrap();
        std::env::remove_var("EDITOR");
        std::env::remove_var("VISUAL");

        assert_eq!(result.editor, "code", "config should override env vars");
    }

    #[test]
    fn resolve_not_found_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();

        let err = resolve("nonexistent", repo_dir.path(), &db, Some("vim")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "error should mention 'not found', got: {msg}"
        );
    }

    #[test]
    fn resolve_updates_last_accessed() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        let wt = db
            .insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        assert!(wt.last_accessed.is_none());

        resolve("my-feature", repo_dir.path(), &db, Some("vim")).unwrap();

        let updated = db.get_worktree(wt.id).unwrap().unwrap();
        assert!(updated.last_accessed.is_some());
        assert!(updated.last_accessed.unwrap() > 0);
    }

    #[test]
    fn resolve_records_opened_event() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        let wt = db
            .insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        resolve("my-feature", repo_dir.path(), &db, Some("vim")).unwrap();

        let event_count = db.count_events(wt.id, Some("opened")).unwrap();
        assert_eq!(event_count, 1, "exactly one 'opened' event should exist");
    }

    #[test]
    fn resolve_by_branch_with_slash() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
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
