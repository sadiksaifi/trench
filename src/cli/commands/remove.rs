use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::git;
use crate::paths;
use crate::state::Database;

/// Result of a worktree removal.
#[derive(Debug)]
pub struct RemoveResult {
    /// The name of the removed worktree.
    pub name: String,
    /// Whether the remote branch was pruned (only `true` if `--prune` was
    /// requested and the remote branch existed).
    pub pruned_remote: bool,
}

/// Execute the `trench remove <identifier>` command.
///
/// Resolves the worktree by sanitized name or branch name, removes it from
/// disk via git2, updates the DB record with `removed_at`, and inserts a
/// "removed" event.
///
/// When `prune` is true, also deletes the corresponding remote branch.
/// Returns a warning via `RemoveResult.pruned_remote = false` if the remote
/// branch was not found (non-fatal).
pub fn execute(identifier: &str, cwd: &Path, db: &Database, prune: bool) -> Result<RemoveResult> {
    let repo_info = git::discover_repo(cwd)?;

    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db
        .get_repo_by_path(repo_path_str)?
        .ok_or_else(|| anyhow::anyhow!("repository not tracked by trench"))?;

    // Try the identifier as-is first, then try sanitizing it
    let wt = match db.find_worktree_by_identifier(repo.id, identifier)? {
        Some(wt) => Some(wt),
        None => {
            let sanitized = paths::sanitize_branch(identifier);
            if sanitized != identifier {
                db.find_worktree_by_identifier(repo.id, &sanitized)?
            } else {
                None
            }
        }
    };

    let wt = match wt {
        Some(wt) => wt,
        None => bail!("worktree not found: {identifier}"),
    };

    let worktree_path = Path::new(&wt.path);

    // Remove worktree from disk and prune git references
    if worktree_path.exists() {
        git::remove_worktree(&repo_info.path, worktree_path)?;
    } else {
        eprintln!("warning: worktree directory already removed from disk");
    }

    // Update DB: set removed_at timestamp
    let now = crate::state::unix_epoch_secs() as i64;

    db.update_worktree(
        wt.id,
        &crate::state::WorktreeUpdate {
            removed_at: Some(Some(now)),
            ..Default::default()
        },
    )
    .context("failed to update worktree record")?;

    // Insert "removed" event
    db.insert_event(repo.id, Some(wt.id), "removed", None)
        .context("failed to insert removed event")?;

    // Optionally delete the remote branch
    let mut pruned_remote = false;
    if prune {
        match git::delete_remote_branch(&repo_info.path, "origin", &wt.branch) {
            Ok(()) => pruned_remote = true,
            Err(git::GitError::RemoteBranchNotFound { branch, remote }) => {
                eprintln!("warning: remote branch '{branch}' not found on {remote}");
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(RemoveResult {
        name: wt.name.clone(),
        pruned_remote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;

    /// Helper: create a temp git repo with an initial commit.
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
    fn remove_happy_path_end_to_end() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a worktree first
        let path = crate::cli::commands::create::execute(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(path.exists(), "worktree should exist after create");

        // Capture the worktree ID before removal
        let repo_path_str = repo_dir.path().canonicalize().unwrap();
        let db_repo = db
            .get_repo_by_path(repo_path_str.to_str().unwrap())
            .unwrap()
            .unwrap();
        let wt_before = db
            .find_worktree_by_identifier(db_repo.id, "my-feature")
            .unwrap()
            .expect("worktree should exist before removal");
        let wt_id = wt_before.id;

        // Remove it
        let result = execute("my-feature", repo_dir.path(), &db, false)
            .expect("remove should succeed");
        assert_eq!(result.name, "my-feature");

        // Verify: directory is gone
        assert!(!path.exists(), "worktree directory should be deleted");

        // Verify: DB record has removed_at set
        let wt = db
            .get_worktree(wt_id)
            .unwrap()
            .expect("worktree record should still exist in DB");
        assert!(
            wt.removed_at.is_some(),
            "removed_at should be set"
        );

        // list_worktrees should no longer include the removed worktree
        let worktrees = db.list_worktrees(db_repo.id).unwrap();
        assert_eq!(worktrees.len(), 0, "removed worktree should not appear in list");

        // Verify: "removed" event was inserted
        let event_count = db.count_events(wt_id, Some("removed")).unwrap();
        assert_eq!(event_count, 1, "exactly one 'removed' event should exist");
    }

    #[test]
    fn remove_resolves_by_branch_name_with_slash() {
        // Test DB resolution of branch names with slashes.
        // We manually insert the DB record since git2 worktree names can't contain slashes.
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a simple worktree (no slashes) then rename its DB record
        // to simulate a worktree with a slashed branch name
        let path = crate::cli::commands::create::execute(
            "feature-auth",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        // Update the DB branch to have a slash (simulating feature/auth → feature-auth mapping)
        let repo_path_str = repo_dir.path().canonicalize().unwrap();
        let db_repo = db.get_repo_by_path(repo_path_str.to_str().unwrap()).unwrap().unwrap();
        let worktrees = db.list_worktrees(db_repo.id).unwrap();
        // Directly update branch in DB to simulate slashed branch
        db.conn_for_test().execute(
            "UPDATE worktrees SET branch = 'feature/auth' WHERE id = ?1",
            rusqlite::params![worktrees[0].id],
        ).unwrap();

        // Remove using the original branch name (feature/auth)
        let result = execute("feature/auth", repo_dir.path(), &db, false)
            .expect("remove by branch name should succeed");
        assert_eq!(result.name, "feature-auth");
        assert!(!path.exists(), "worktree directory should be deleted");
    }

    #[test]
    fn remove_resolves_by_sanitized_name() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let path = crate::cli::commands::create::execute(
            "feature-auth",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        // Remove using the sanitized name
        let result = execute("feature-auth", repo_dir.path(), &db, false)
            .expect("remove by sanitized name should succeed");
        assert_eq!(result.name, "feature-auth");
        assert!(!path.exists(), "worktree directory should be deleted");
    }

    /// Helper: create a bare remote, clone it, and return (clone_path, remote_dir).
    /// The clone_dir TempDir is returned to keep it alive.
    fn setup_repo_with_remote() -> (tempfile::TempDir, tempfile::TempDir) {
        let remote_dir = tempfile::tempdir().unwrap();
        let remote_repo = git2::Repository::init_bare(remote_dir.path()).unwrap();
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let empty_tree = remote_repo.treebuilder(None).unwrap().write().unwrap();
            let tree = remote_repo.find_tree(empty_tree).unwrap();
            remote_repo
                .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        let clone_dir = tempfile::tempdir().unwrap();
        git2::build::RepoBuilder::new()
            .clone(remote_dir.path().to_str().unwrap(), clone_dir.path())
            .unwrap();
        (clone_dir, remote_dir)
    }

    #[test]
    fn remove_with_prune_deletes_remote_branch() {
        let (clone_dir, remote_dir) = setup_repo_with_remote();
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a worktree
        let path = crate::cli::commands::create::execute(
            "prune-me",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(path.exists());

        // Push the branch to the remote
        let clone = git2::Repository::open(clone_dir.path()).unwrap();
        {
            let mut origin = clone.find_remote("origin").unwrap();
            origin
                .push(
                    &["refs/heads/prune-me:refs/heads/prune-me"],
                    None,
                )
                .unwrap();
        }
        // Fetch to update remote-tracking refs
        {
            let mut origin = clone.find_remote("origin").unwrap();
            origin.fetch(&[] as &[&str], None, None).unwrap();
        }

        // Verify the remote branch exists on the bare remote
        let remote_repo = git2::Repository::open_bare(remote_dir.path()).unwrap();
        assert!(
            remote_repo.find_branch("prune-me", git2::BranchType::Local).is_ok(),
            "branch should exist on remote before prune"
        );

        // Remove with prune
        let result = execute("prune-me", clone_dir.path(), &db, true)
            .expect("remove with prune should succeed");
        assert_eq!(result.name, "prune-me");
        assert!(result.pruned_remote, "should have pruned remote branch");

        // Verify: worktree directory is gone
        assert!(!path.exists(), "worktree directory should be deleted");

        // Verify: remote branch is gone
        // Reopen the bare remote to get fresh state
        let remote_repo = git2::Repository::open_bare(remote_dir.path()).unwrap();
        assert!(
            remote_repo.find_branch("prune-me", git2::BranchType::Local).is_err(),
            "branch should be deleted on remote after prune"
        );
    }

    #[test]
    fn remove_with_prune_warns_when_remote_branch_missing() {
        let (clone_dir, _remote_dir) = setup_repo_with_remote();
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a worktree (but DON'T push the branch to remote)
        let path = crate::cli::commands::create::execute(
            "no-remote",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(path.exists());

        // Remove with prune — remote branch doesn't exist, should warn but succeed
        let result = execute("no-remote", clone_dir.path(), &db, true)
            .expect("remove with prune should succeed even without remote branch");
        assert_eq!(result.name, "no-remote");
        assert!(!result.pruned_remote, "should NOT have pruned remote branch");

        // Verify: worktree directory is gone
        assert!(!path.exists(), "worktree directory should be deleted");
    }

    #[test]
    fn remove_not_found_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Insert repo record so the repo is tracked
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        db.insert_repo("test-repo", &repo_path_str, Some("main"))
            .unwrap();

        let result = execute("nonexistent", repo_dir.path(), &db, false);
        let err = result.expect_err("should error for nonexistent worktree");
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "error should mention 'not found', got: {msg}"
        );
    }
}
