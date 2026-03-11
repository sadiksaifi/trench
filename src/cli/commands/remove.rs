use std::path::Path;

use anyhow::{Context, Result};

use crate::config::HooksConfig;
use crate::git::{self, RepoInfo};
use crate::hooks::{self, HookEnvContext, HookEvent};
use crate::state::{Database, Repo, Worktree};

/// Typed errors for the `remove` command.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("pre_remove hook failed")]
    PreRemoveHookFailed(#[source] anyhow::Error),
}

/// Hook execution status for the remove operation.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveHooksStatus {
    /// No hooks were configured.
    None,
    /// Hooks executed successfully.
    Ran,
    /// Hooks were configured but skipped (`--no-hooks`).
    Skipped,
}

/// Result of `execute_resolved_with_hooks` — includes remove result, hooks
/// status, and any post_remove hook warning.
#[derive(Debug)]
pub struct RemoveWithHooksResult {
    pub result: RemoveResult,
    pub hooks_status: RemoveHooksStatus,
    /// If post_remove hook failed, this contains the error.
    /// The worktree was already removed — this is a warning only (FR-24).
    pub post_remove_warning: Option<anyhow::Error>,
}

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
    let (repo, wt) = crate::adopt::resolve_or_adopt(identifier, &repo_info, db)?;
    execute_resolved(&repo, &wt, &repo_info, db, prune)
}

/// Execute removal with pre-resolved worktree data.
///
/// Use this when the caller has already resolved the worktree (e.g. for
/// the confirmation prompt) to avoid a redundant DB/git round-trip.
pub fn execute_resolved(
    repo: &Repo,
    wt: &Worktree,
    repo_info: &RepoInfo,
    db: &Database,
    prune: bool,
) -> Result<RemoveResult> {
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

/// Execute `trench remove` with lifecycle hooks.
///
/// Orchestrates: pre_remove hook → removal → post_remove hook.
/// - If `no_hooks` is true or no hooks configured, hooks are skipped.
/// - Pre_remove failure cancels the operation (worktree not removed).
/// - Post_remove failure: worktree already gone, warning only (FR-24).
pub async fn execute_resolved_with_hooks(
    repo: &Repo,
    wt: &Worktree,
    repo_info: &RepoInfo,
    db: &Database,
    prune: bool,
    hooks_config: Option<&HooksConfig>,
    no_hooks: bool,
) -> Result<RemoveWithHooksResult> {
    let has_hooks = hooks_config
        .map(|h| h.pre_remove.is_some() || h.post_remove.is_some())
        .unwrap_or(false);

    // Fast path: no hooks to run
    if no_hooks || !has_hooks {
        let hooks_status = if no_hooks && has_hooks {
            RemoveHooksStatus::Skipped
        } else {
            RemoveHooksStatus::None
        };
        let result = execute_resolved(repo, wt, repo_info, db, prune)?;
        return Ok(RemoveWithHooksResult {
            result,
            hooks_status,
            post_remove_warning: None,
        });
    }

    let hooks = hooks_config.unwrap(); // safe: has_hooks is true

    let env_ctx = HookEnvContext {
        worktree_path: wt.path.clone(),
        worktree_name: wt.name.clone(),
        branch: wt.branch.clone(),
        repo_name: repo.name.clone(),
        repo_path: repo_info.path.to_string_lossy().to_string(),
        base_branch: wt.base_branch.clone().unwrap_or_default(),
    };

    // Step 1: pre_remove hook (cwd = worktree path, FR-22)
    if let Some(pre_remove) = &hooks.pre_remove {
        let worktree_path = Path::new(&wt.path);
        hooks::runner::execute_hook(
            &HookEvent::PreRemove,
            pre_remove,
            &env_ctx,
            &repo_info.path,
            worktree_path,
            db,
            repo.id,
            Some(wt.id),
        )
        .await
        .map_err(RemoveError::PreRemoveHookFailed)?;
    }

    // Step 2: remove worktree
    let result = execute_resolved(repo, wt, repo_info, db, prune)?;

    Ok(RemoveWithHooksResult {
        result,
        hooks_status: RemoveHooksStatus::Ran,
        post_remove_warning: None,
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
        let create_result = crate::cli::commands::create::execute(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(
            create_result.path.exists(),
            "worktree should exist after create"
        );

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
        let result =
            execute("my-feature", repo_dir.path(), &db, false).expect("remove should succeed");
        assert_eq!(result.name, "my-feature");

        // Verify: directory is gone
        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );

        // Verify: DB record has removed_at set
        let wt = db
            .get_worktree(wt_id)
            .unwrap()
            .expect("worktree record should still exist in DB");
        assert!(wt.removed_at.is_some(), "removed_at should be set");

        // list_worktrees should no longer include the removed worktree
        let worktrees = db.list_worktrees(db_repo.id).unwrap();
        assert_eq!(
            worktrees.len(),
            0,
            "removed worktree should not appear in list"
        );

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
        let create_result = crate::cli::commands::create::execute(
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
        let db_repo = db
            .get_repo_by_path(repo_path_str.to_str().unwrap())
            .unwrap()
            .unwrap();
        let worktrees = db.list_worktrees(db_repo.id).unwrap();
        // Directly update branch in DB to simulate slashed branch
        db.conn_for_test()
            .execute(
                "UPDATE worktrees SET branch = 'feature/auth' WHERE id = ?1",
                rusqlite::params![worktrees[0].id],
            )
            .unwrap();

        // Remove using the original branch name (feature/auth)
        let result = execute("feature/auth", repo_dir.path(), &db, false)
            .expect("remove by branch name should succeed");
        assert_eq!(result.name, "feature-auth");
        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );
    }

    #[test]
    fn remove_resolves_by_sanitized_name() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
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
        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );
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
        let create_result = crate::cli::commands::create::execute(
            "prune-me",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(create_result.path.exists());

        // Push the branch to the remote
        let clone = git2::Repository::open(clone_dir.path()).unwrap();
        {
            let mut origin = clone.find_remote("origin").unwrap();
            origin
                .push(&["refs/heads/prune-me:refs/heads/prune-me"], None)
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
            remote_repo
                .find_branch("prune-me", git2::BranchType::Local)
                .is_ok(),
            "branch should exist on remote before prune"
        );

        // Remove with prune
        let result = execute("prune-me", clone_dir.path(), &db, true)
            .expect("remove with prune should succeed");
        assert_eq!(result.name, "prune-me");
        assert!(result.pruned_remote, "should have pruned remote branch");

        // Verify: worktree directory is gone
        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );

        // Verify: remote branch is gone
        // Reopen the bare remote to get fresh state
        let remote_repo = git2::Repository::open_bare(remote_dir.path()).unwrap();
        assert!(
            remote_repo
                .find_branch("prune-me", git2::BranchType::Local)
                .is_err(),
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
        let create_result = crate::cli::commands::create::execute(
            "no-remote",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(create_result.path.exists());

        // Remove with prune — remote branch doesn't exist, should warn but succeed
        let result = execute("no-remote", clone_dir.path(), &db, true)
            .expect("remove with prune should succeed even without remote branch");
        assert_eq!(result.name, "no-remote");
        assert!(
            !result.pruned_remote,
            "should NOT have pruned remote branch"
        );

        // Verify: worktree directory is gone
        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );
    }

    #[test]
    fn remove_adopts_unmanaged_worktree_before_removing() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Register repo in DB but NOT the worktree
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("unmanaged-rm");
        git_repo
            .branch(
                "unmanaged-rm",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("unmanaged-rm", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("unmanaged-rm", &wt_path, Some(&opts))
            .unwrap();
        assert!(wt_path.exists(), "worktree should exist on disk");

        // Remove the unmanaged worktree — should adopt then remove
        let result = execute("unmanaged-rm", repo_dir.path(), &db, false)
            .expect("remove of unmanaged worktree should succeed");
        assert_eq!(result.name, "unmanaged-rm");

        // Verify worktree was adopted (has adopted_at) and removed (has removed_at)
        let db_repo = db.get_repo_by_path(repo_path_str).unwrap().unwrap();
        // Check via raw query since find_worktree_by_identifier excludes removed
        let wt_count: i64 = db
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM worktrees WHERE repo_id = ?1 AND name = 'unmanaged-rm'",
                rusqlite::params![db_repo.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wt_count, 1, "worktree should exist in DB");

        // Check adopted_at and removed_at via raw query
        let (adopted_at, removed_at): (Option<i64>, Option<i64>) = db.conn_for_test().query_row(
            "SELECT adopted_at, removed_at FROM worktrees WHERE repo_id = ?1 AND name = 'unmanaged-rm'",
            rusqlite::params![db_repo.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert!(adopted_at.is_some(), "adopted_at should be set");
        assert!(removed_at.is_some(), "removed_at should be set");
    }

    #[test]
    fn execute_resolved_removes_with_preresolved_data() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a worktree via the normal path
        let create_result = crate::cli::commands::create::execute(
            "pre-resolved",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(create_result.path.exists());

        // Resolve the worktree manually (simulating what run_remove does for the prompt)
        let repo_info = git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) = crate::adopt::resolve_or_adopt("pre-resolved", &repo_info, &db).unwrap();

        // Call execute_resolved with the pre-resolved data
        let result =
            execute_resolved(&repo, &wt, &repo_info, &db, false).expect("should succeed");
        assert_eq!(result.name, "pre-resolved");
        assert!(!create_result.path.exists(), "worktree dir should be gone");
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

    // ── Hook integration tests ──────────────────────────────────────────

    fn sample_hooks_config() -> crate::config::HooksConfig {
        crate::config::HooksConfig {
            pre_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec!["echo pre_remove_ran".to_string()]),
                shell: None,
                timeout_secs: Some(30),
            }),
            post_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec!["echo post_remove_ran".to_string()]),
                shell: None,
                timeout_secs: Some(30),
            }),
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_with_hooks_no_hooks_configured_returns_none_status() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a worktree first
        let create_result = crate::cli::commands::create::execute(
            "hooks-none",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(create_result.path.exists());

        // Resolve worktree for the hooks variant
        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("hooks-none", &repo_info, &db).unwrap();

        // Remove with no hooks configured
        let outcome = execute_resolved_with_hooks(
            &repo,
            &wt,
            &repo_info,
            &db,
            false,
            None,  // no hooks
            false, // no_hooks flag irrelevant
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.result.name, "hooks-none");
        assert_eq!(outcome.hooks_status, RemoveHooksStatus::None);
        assert!(outcome.post_remove_warning.is_none());
        assert!(!create_result.path.exists(), "worktree dir should be deleted");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_with_hooks_no_hooks_flag_skips_hooks() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "skip-hooks",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("skip-hooks", &repo_info, &db).unwrap();

        let hooks = sample_hooks_config();

        // Remove with --no-hooks
        let outcome = execute_resolved_with_hooks(
            &repo,
            &wt,
            &repo_info,
            &db,
            false,
            Some(&hooks),
            true, // no_hooks = true
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.result.name, "skip-hooks");
        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Skipped);
        assert!(outcome.post_remove_warning.is_none());
        assert!(!create_result.path.exists(), "worktree dir should be deleted");

        // Verify no hook events were recorded
        let wt_record = db.get_worktree(wt.id).unwrap().unwrap();
        let hook_events = db.count_events(wt_record.id, Some("hook:pre_remove")).unwrap();
        assert_eq!(hook_events, 0, "no hook events should be recorded when --no-hooks");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_remove_hook_runs_before_worktree_deletion() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "pre-rm-test",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("pre-rm-test", &repo_info, &db).unwrap();

        // pre_remove hook writes a marker file to prove it ran with cwd = worktree path
        let hooks = crate::config::HooksConfig {
            pre_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec!["echo pre_remove_executed".to_string()]),
                shell: None,
                timeout_secs: Some(30),
            }),
            ..Default::default()
        };

        let outcome = execute_resolved_with_hooks(
            &repo,
            &wt,
            &repo_info,
            &db,
            false,
            Some(&hooks),
            false,
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Ran);
        assert!(!create_result.path.exists(), "worktree dir should be deleted after hooks");

        // Verify hook event was logged
        let hook_events = db.count_events(wt.id, Some("hook:pre_remove")).unwrap();
        assert_eq!(hook_events, 1, "pre_remove hook event should be logged");

        // Verify hook output was captured in logs
        let events = db.list_events(wt.id, 10).unwrap();
        let hook_event = events.iter().find(|e| e.event_type == "hook:pre_remove").unwrap();
        let logs = db.get_logs(hook_event.id).unwrap();
        let stdout_lines: Vec<&str> = logs
            .iter()
            .filter(|(s, _, _)| s == "stdout")
            .map(|(_, l, _)| l.as_str())
            .collect();
        assert!(
            stdout_lines.contains(&"pre_remove_executed"),
            "pre_remove output should be logged: {stdout_lines:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_remove_failure_cancels_removal() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "fail-pre-rm",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("fail-pre-rm", &repo_info, &db).unwrap();

        // pre_remove hook that fails
        let hooks = crate::config::HooksConfig {
            pre_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec!["exit 1".to_string()]),
                shell: None,
                timeout_secs: Some(30),
            }),
            ..Default::default()
        };

        let err = execute_resolved_with_hooks(
            &repo,
            &wt,
            &repo_info,
            &db,
            false,
            Some(&hooks),
            false,
        )
        .await
        .expect_err("should fail when pre_remove hook fails");

        // Verify error is a RemoveError::PreRemoveHookFailed
        assert!(
            err.downcast_ref::<RemoveError>().is_some(),
            "error should be RemoveError, got: {err:#}"
        );

        // Verify worktree was NOT deleted
        assert!(
            create_result.path.exists(),
            "worktree directory should still exist after pre_remove failure"
        );

        // Verify DB record was NOT marked as removed
        let wt_record = db.get_worktree(wt.id).unwrap().unwrap();
        assert!(
            wt_record.removed_at.is_none(),
            "removed_at should be None after pre_remove failure"
        );
    }
}
