use std::fmt;
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

/// Plan produced by `--dry-run` showing what `trench remove` would do.
#[derive(Debug, serde::Serialize)]
pub struct RemoveDryRunPlan {
    /// Always `true` — signals this is a preview, not a real operation.
    pub dry_run: bool,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub prune: bool,
    pub hooks: Option<RemoveDryRunHooks>,
}

impl fmt::Display for RemoveDryRunPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Dry run — no changes will be made\n")?;
        writeln!(f, "  Worktree:  {}", self.name)?;
        writeln!(f, "  Branch:    {}", self.branch)?;
        writeln!(f, "  Path:      {}", self.path)?;
        writeln!(
            f,
            "  Prune:     {}",
            if self.prune { "yes" } else { "no" }
        )?;

        match &self.hooks {
            Some(hooks) if hooks.pre_remove.is_some() || hooks.post_remove.is_some() => {
                writeln!(f, "  Hooks:")?;
                if let Some(h) = &hooks.pre_remove {
                    writeln!(f, "    pre_remove:")?;
                    format_hook_def(f, h)?;
                }
                if let Some(h) = &hooks.post_remove {
                    writeln!(f, "    post_remove:")?;
                    format_hook_def(f, h)?;
                }
            }
            _ => {
                writeln!(f, "  Hooks:     (none)")?;
            }
        }

        Ok(())
    }
}

fn format_hook_def(f: &mut fmt::Formatter<'_>, hook: &crate::config::HookDef) -> fmt::Result {
    if let Some(copy) = &hook.copy {
        writeln!(f, "      copy: {}", copy.join(", "))?;
    }
    if let Some(run) = &hook.run {
        writeln!(f, "      run:  {}", run.join(", "))?;
    }
    if let Some(shell) = &hook.shell {
        writeln!(f, "      shell: {shell}")?;
    }
    if let Some(timeout) = &hook.timeout_secs {
        writeln!(f, "      timeout: {timeout}s")?;
    }
    Ok(())
}

/// Hook definitions included in a remove dry-run plan.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoveDryRunHooks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_remove: Option<crate::config::HookDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_remove: Option<crate::config::HookDef>,
}

/// Execute a dry-run of `trench remove <identifier>`.
///
/// Resolves the worktree and builds a plan, but performs no git operations,
/// no DB writes, and no hook execution.
pub fn execute_dry_run(
    identifier: &str,
    cwd: &Path,
    db: Option<&Database>,
    prune: bool,
    hooks_config: Option<&HooksConfig>,
    no_hooks: bool,
) -> Result<RemoveDryRunPlan> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let (_repo, wt) = crate::adopt::resolve_only(identifier, &repo_info, db)?;

    let hooks = if no_hooks {
        None
    } else {
        hooks_config.and_then(|h| {
            let hooks = RemoveDryRunHooks {
                pre_remove: h.pre_remove.clone(),
                post_remove: h.post_remove.clone(),
            };
            if hooks.pre_remove.is_none() && hooks.post_remove.is_none() {
                None
            } else {
                Some(hooks)
            }
        })
    };

    Ok(RemoveDryRunPlan {
        dry_run: true,
        name: wt.name.clone(),
        branch: wt.branch.clone(),
        path: wt.path.clone(),
        prune,
        hooks,
    })
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
    hook_tx: Option<&std::sync::mpsc::Sender<crate::tui::screens::hook_log::HookOutputMessage>>,
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
        if worktree_path.exists() {
            hooks::runner::execute_hook(
                &HookEvent::PreRemove,
                pre_remove,
                &env_ctx,
                &repo_info.path,
                worktree_path,
                db,
                repo.id,
                Some(wt.id),
                hook_tx,
            )
            .await
            .map_err(RemoveError::PreRemoveHookFailed)?;
        } else {
            eprintln!(
                "warning: skipping pre_remove hook because the worktree directory is already gone"
            );
        }
    }

    // Step 2: remove worktree from disk
    // Inlined from execute_resolved so that post_remove fires immediately after
    // disk deletion, regardless of whether DB bookkeeping succeeds.
    let worktree_path = Path::new(&wt.path);
    if worktree_path.exists() {
        git::remove_worktree(&repo_info.path, worktree_path)?;
    } else {
        eprintln!("warning: worktree directory already removed from disk");
    }

    // Step 3: post_remove hook fires IMMEDIATELY after disk deletion (FR-22)
    let post_remove_warning = if let Some(post_remove) = &hooks.post_remove {
        match hooks::runner::execute_hook(
            &HookEvent::PostRemove,
            post_remove,
            &env_ctx,
            &repo_info.path,
            &repo_info.path, // cwd = repo path (worktree is gone)
            db,
            repo.id,
            Some(wt.id),
            hook_tx,
        )
        .await
        {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    } else {
        None
    };

    // Step 4: DB bookkeeping — cannot prevent post_remove from running
    let now = crate::state::unix_epoch_secs() as i64;
    db.update_worktree(
        wt.id,
        &crate::state::WorktreeUpdate {
            removed_at: Some(Some(now)),
            ..Default::default()
        },
    )
    .context("failed to update worktree record")?;

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

    Ok(RemoveWithHooksResult {
        result: RemoveResult {
            name: wt.name.clone(),
            pruned_remote,
        },
        hooks_status: RemoveHooksStatus::Ran,
        post_remove_warning,
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
            None,
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
            None,
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
            None,
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
            None,
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

    #[tokio::test(flavor = "current_thread")]
    async fn post_remove_hook_runs_after_deletion_with_repo_cwd() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "post-rm-test",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("post-rm-test", &repo_info, &db).unwrap();

        // post_remove hook creates a marker file in repo dir to prove cwd = repo path
        let marker = repo_dir.path().join("post_remove_marker.txt");
        let hooks = crate::config::HooksConfig {
            post_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec![format!("echo done > {}", marker.display())]),
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
            None,
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Ran);
        assert!(!create_result.path.exists(), "worktree dir should be deleted");
        assert!(outcome.post_remove_warning.is_none());

        // Verify post_remove hook ran with cwd = repo path
        assert!(
            marker.exists(),
            "post_remove marker should exist (proves cwd = repo path)"
        );

        // Verify hook event logged
        let hook_events = db.count_events(wt.id, Some("hook:post_remove")).unwrap();
        assert_eq!(hook_events, 1, "post_remove hook event should be logged");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_remove_hook_skipped_when_worktree_dir_missing() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "gone-dir",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("gone-dir", &repo_info, &db).unwrap();

        // Manually delete the worktree directory to simulate user deletion
        std::fs::remove_dir_all(&create_result.path).unwrap();
        assert!(!create_result.path.exists());

        // pre_remove hook configured — should be skipped, not error
        let hooks = crate::config::HooksConfig {
            pre_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec!["echo should_not_run".to_string()]),
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
            None,
        )
        .await
        .expect("remove should succeed even when worktree dir is gone");

        assert_eq!(outcome.result.name, "gone-dir");
        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Ran);

        // DB record should have removed_at set
        let wt_record = db.get_worktree(wt.id).unwrap().unwrap();
        assert!(wt_record.removed_at.is_some(), "removed_at should be set");

        // No pre_remove hook event should be logged (hook was skipped)
        let hook_events = db.count_events(wt.id, Some("hook:pre_remove")).unwrap();
        assert_eq!(hook_events, 0, "pre_remove hook should not have run");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_remove_failure_is_warning_only() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "post-fail",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let (repo, wt) =
            crate::adopt::resolve_or_adopt("post-fail", &repo_info, &db).unwrap();

        // post_remove hook that fails
        let hooks = crate::config::HooksConfig {
            post_remove: Some(crate::config::HookDef {
                copy: None,
                run: Some(vec!["exit 42".to_string()]),
                shell: None,
                timeout_secs: Some(30),
            }),
            ..Default::default()
        };

        // Should succeed despite post_remove failure (FR-24: WarnOnly)
        let outcome = execute_resolved_with_hooks(
            &repo,
            &wt,
            &repo_info,
            &db,
            false,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("remove should succeed even if post_remove fails");

        assert_eq!(outcome.result.name, "post-fail");
        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Ran);
        assert!(!create_result.path.exists(), "worktree should be deleted");

        // Post_remove failure should be captured as warning
        assert!(
            outcome.post_remove_warning.is_some(),
            "post_remove warning should be captured"
        );

        // DB should still have removed_at set
        let wt_record = db.get_worktree(wt.id).unwrap().unwrap();
        assert!(wt_record.removed_at.is_some(), "removed_at should be set");
    }

    // ── Dry-run tests ──────────────────────────────────────────────────

    fn create_worktree_for_dry_run(
        branch: &str,
    ) -> (
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
        Database,
    ) {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        crate::cli::commands::create::execute(
            branch,
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        (repo_dir, wt_root, db_dir, db)
    }

    #[test]
    fn dry_run_returns_plan_with_worktree_details_and_hooks() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("dry-run-test");

        let hooks = sample_hooks_config();

        let plan = execute_dry_run(
            "dry-run-test",
            repo_dir.path(),
            Some(&db),
            false, // prune
            Some(&hooks),
            false, // no_hooks
        )
        .expect("dry-run should succeed");

        assert!(plan.dry_run);
        assert_eq!(plan.name, "dry-run-test");
        assert_eq!(plan.branch, "dry-run-test");
        assert!(!plan.prune);
        assert!(plan.hooks.is_some());

        let plan_hooks = plan.hooks.unwrap();
        assert!(plan_hooks.pre_remove.is_some());
        assert!(plan_hooks.post_remove.is_some());
    }

    #[test]
    fn dry_run_with_no_hooks_excludes_hooks() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("no-hooks-dry");

        let hooks = sample_hooks_config();

        let plan = execute_dry_run(
            "no-hooks-dry",
            repo_dir.path(),
            Some(&db),
            false,
            Some(&hooks),
            true, // no_hooks = true
        )
        .expect("dry-run should succeed");

        assert!(plan.dry_run);
        assert_eq!(plan.name, "no-hooks-dry");
        assert!(plan.hooks.is_none(), "hooks should be None when --no-hooks");
    }

    #[test]
    fn dry_run_display_shows_human_readable_plan() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("display-test");

        let hooks = sample_hooks_config();

        let plan = execute_dry_run(
            "display-test",
            repo_dir.path(),
            Some(&db),
            true,
            Some(&hooks),
            false,
        )
        .expect("dry-run should succeed");

        let output = format!("{plan}");
        assert!(output.contains("Dry run"), "should contain 'Dry run' header");
        assert!(output.contains("display-test"), "should contain worktree name");
        assert!(output.contains("pre_remove"), "should show pre_remove hook");
        assert!(output.contains("post_remove"), "should show post_remove hook");
        assert!(output.contains("Prune:"), "should mention prune status");
    }

    #[test]
    fn dry_run_json_serialization_includes_all_fields() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("json-test");

        let hooks = sample_hooks_config();

        let plan = execute_dry_run(
            "json-test",
            repo_dir.path(),
            Some(&db),
            false,
            Some(&hooks),
            false,
        )
        .expect("dry-run should succeed");

        let json_str = serde_json::to_string_pretty(&plan).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["name"], "json-test");
        assert_eq!(parsed["branch"], "json-test");
        assert_eq!(parsed["prune"], false);
        assert!(parsed["hooks"].is_object(), "hooks should be an object");
        assert!(parsed["hooks"]["pre_remove"].is_object());
        assert!(parsed["hooks"]["post_remove"].is_object());
    }

    #[test]
    fn dry_run_with_prune_shows_prune_status() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("prune-dry");

        let plan = execute_dry_run(
            "prune-dry",
            repo_dir.path(),
            Some(&db),
            true, // prune
            None,
            false,
        )
        .expect("dry-run should succeed");

        assert!(plan.prune, "prune should be true");
        assert!(plan.hooks.is_none(), "no hooks configured");
    }

    #[test]
    fn dry_run_empty_hooks_config_normalizes_to_none() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("empty-hooks");

        // HooksConfig exists but both pre_remove and post_remove are None
        let empty_hooks = crate::config::HooksConfig {
            pre_create: None,
            post_create: None,
            pre_remove: None,
            post_remove: None,
            pre_sync: None,
            post_sync: None,
        };

        let plan = execute_dry_run(
            "empty-hooks",
            repo_dir.path(),
            Some(&db),
            false,
            Some(&empty_hooks),
            false,
        )
        .expect("dry-run should succeed");

        assert!(
            plan.hooks.is_none(),
            "empty hooks config should normalize to None, got: {:?}",
            plan.hooks
        );
    }
}
