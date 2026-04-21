use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::HooksConfig;
use crate::git::{self, GitWorktreeEntry, RepoInfo};
use crate::hooks::{self, HookEnvContext, HookEvent};
use crate::live_worktree::LiveWorktree;
use crate::state::{Database, Repo, Worktree};

/// Typed errors for the `remove` command.
#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("pre_remove hook failed")]
    PreRemoveHookFailed(#[source] anyhow::Error),
}

/// Hook execution status for the remove operation.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
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
    /// The raw branch name associated with the removed worktree, when known.
    pub branch: Option<String>,
    /// Whether local branch deletion was requested.
    pub delete_branch_requested: bool,
    /// Whether the local branch was deleted.
    pub branch_deleted: bool,
    /// Whether branch deletion used force mode.
    pub branch_delete_forced: bool,
    /// Error from local branch deletion, if requested but not completed.
    pub branch_delete_error: Option<String>,
}

/// JSON-serializable output for `trench remove --json`.
#[derive(Debug, serde::Serialize)]
pub struct RemoveJsonOutput {
    pub worktree: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub hooks: RemoveHooksStatus,
    pub delete_branch_requested: bool,
    pub branch_deleted: bool,
    pub branch_delete_forced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_delete_error: Option<String>,
}

impl RemoveResult {
    pub fn to_json_output(self, hooks: RemoveHooksStatus) -> RemoveJsonOutput {
        RemoveJsonOutput {
            worktree: self.name,
            branch: self.branch,
            hooks,
            delete_branch_requested: self.delete_branch_requested,
            branch_deleted: self.branch_deleted,
            branch_delete_forced: self.branch_delete_forced,
            branch_delete_error: self.branch_delete_error,
        }
    }
}

/// Plan produced by `--dry-run` showing what `trench remove` would do.
#[derive(Debug, serde::Serialize)]
pub struct RemoveDryRunPlan {
    /// Always `true` — signals this is a preview, not a real operation.
    pub dry_run: bool,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub delete_branch_requested: bool,
    pub force: bool,
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
            "  Delete branch: {}",
            if self.delete_branch_requested {
                "yes"
            } else {
                "no"
            }
        )?;
        writeln!(f, "  Force:     {}", if self.force { "yes" } else { "no" })?;

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

fn archived_path(live_path: &Path, removed_at: i64) -> String {
    format!("{}#removed-{removed_at}", live_path.to_string_lossy())
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
    delete_branch_requested: bool,
    force: bool,
    hooks_config: Option<&HooksConfig>,
    no_hooks: bool,
) -> Result<RemoveDryRunPlan> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let live = crate::live_worktree::resolve_read_only(identifier, &repo_info, db)?;
    let branch = live
        .entry
        .branch
        .clone()
        .unwrap_or_else(|| live.entry.name.clone());

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
        name: live.entry.name.clone(),
        branch,
        path: live.entry.path.to_string_lossy().to_string(),
        delete_branch_requested,
        force,
        hooks,
    })
}

/// Execute the `trench remove <identifier>` command.
///
/// Resolves the worktree from live git state, removes it from disk, and
/// purges trench metadata for the path when present.
pub fn execute(
    identifier: &str,
    cwd: &Path,
    db: &Database,
    delete_branch: bool,
) -> Result<RemoveResult> {
    let repo_info = git::discover_repo(cwd)?;
    let live = crate::live_worktree::resolve(identifier, &repo_info, db)?;
    execute_live_resolved(&live, &repo_info, db, delete_branch, false)
}

/// Execute removal with pre-resolved worktree data.
///
/// Use this when the caller has already resolved the worktree (e.g. for
/// the confirmation prompt) to avoid a redundant DB/git round-trip.
pub fn execute_live_resolved(
    live: &LiveWorktree,
    repo_info: &RepoInfo,
    db: &Database,
    delete_branch: bool,
    force_delete_branch: bool,
) -> Result<RemoveResult> {
    let worktree_path = live.entry.path.as_path();

    // Remove worktree from disk and prune git references
    if worktree_path.exists() {
        git::remove_worktree(&repo_info.path, worktree_path)?;
    } else {
        eprintln!("warning: worktree directory already removed from disk");
    }

    if let Some(metadata) = live.metadata.as_ref() {
        let now = crate::state::unix_epoch_secs() as i64;
        db.archive_removed_worktree(metadata.id, &archived_path(worktree_path, now), now)
            .context("failed to archive removed worktree metadata")?;
        let repo = db.get_repo(metadata.repo_id)?.ok_or_else(|| {
            anyhow::anyhow!("repo metadata missing for worktree '{}'", metadata.name)
        })?;
        db.insert_event(repo.id, Some(metadata.id), "removed", None)
            .context("failed to insert removed event")?;
    }

    let branch = live.entry.branch.clone();
    let mut branch_deleted = false;
    let mut branch_delete_error = None;
    if delete_branch {
        if let Some(branch_name) = branch.as_deref() {
            match git::delete_local_branch(&repo_info.path, branch_name, force_delete_branch) {
                Ok(()) => branch_deleted = true,
                Err(git::GitError::LocalBranchNotFound { .. }) => {}
                Err(e) => branch_delete_error = Some(e.to_string()),
            }
        }
    }

    Ok(RemoveResult {
        name: live.entry.name.clone(),
        branch,
        delete_branch_requested: delete_branch,
        branch_deleted,
        branch_delete_forced: delete_branch && force_delete_branch,
        branch_delete_error,
    })
}

pub fn execute_resolved(
    _repo: &Repo,
    wt: &Worktree,
    repo_info: &RepoInfo,
    db: &Database,
    delete_branch: bool,
    force_delete_branch: bool,
) -> Result<RemoveResult> {
    let live = LiveWorktree {
        entry: GitWorktreeEntry {
            name: wt.name.clone(),
            path: Path::new(&wt.path).to_path_buf(),
            branch: Some(wt.branch.clone()),
            is_main: false,
        },
        metadata: Some(wt.clone()),
    };
    execute_live_resolved(&live, repo_info, db, delete_branch, force_delete_branch)
}

/// Execute `trench remove` with lifecycle hooks.
///
/// Orchestrates: pre_remove hook → removal → post_remove hook.
/// - If `no_hooks` is true or no hooks configured, hooks are skipped.
/// - Pre_remove failure cancels the operation (worktree not removed).
/// - Post_remove failure: worktree already gone, warning only (FR-24).
pub async fn execute_live_resolved_with_hooks(
    live: &LiveWorktree,
    repo_info: &RepoInfo,
    db: &Database,
    delete_branch: bool,
    force_delete_branch: bool,
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
        let result =
            execute_live_resolved(live, repo_info, db, delete_branch, force_delete_branch)?;
        return Ok(RemoveWithHooksResult {
            result,
            hooks_status,
            post_remove_warning: None,
        });
    }

    let hooks = hooks_config.unwrap(); // safe: has_hooks is true
    let (repo, wt) = crate::live_worktree::ensure_metadata(db, repo_info, &live.entry)?;
    let base_branch = crate::live_worktree::base_branch(repo_info, live);

    let env_ctx = HookEnvContext {
        worktree_path: wt.path.clone(),
        worktree_name: wt.name.clone(),
        branch: wt.branch.clone(),
        repo_name: repo.name.clone(),
        repo_path: repo_info.path.to_string_lossy().to_string(),
        base_branch,
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

    // Step 4: archive metadata after hook execution
    let now = crate::state::unix_epoch_secs() as i64;
    db.archive_removed_worktree(wt.id, &archived_path(worktree_path, now), now)
        .context("failed to archive removed worktree metadata")?;
    db.insert_event(repo.id, Some(wt.id), "removed", None)
        .context("failed to insert removed event")?;

    let mut branch_deleted = false;
    let mut branch_delete_error = None;
    if delete_branch {
        match git::delete_local_branch(&repo_info.path, &wt.branch, force_delete_branch) {
            Ok(()) => branch_deleted = true,
            Err(git::GitError::LocalBranchNotFound { .. }) => {}
            Err(e) => {
                branch_delete_error = Some(e.to_string());
            }
        }
    }

    Ok(RemoveWithHooksResult {
        result: RemoveResult {
            name: wt.name.clone(),
            branch: Some(wt.branch.clone()),
            delete_branch_requested: delete_branch,
            branch_deleted,
            branch_delete_forced: delete_branch && force_delete_branch,
            branch_delete_error,
        },
        hooks_status: RemoveHooksStatus::Ran,
        post_remove_warning,
    })
}

pub async fn execute_resolved_with_hooks(
    repo: &Repo,
    wt: &Worktree,
    repo_info: &RepoInfo,
    db: &Database,
    delete_branch: bool,
    force_delete_branch: bool,
    hooks_config: Option<&HooksConfig>,
    no_hooks: bool,
    hook_tx: Option<&std::sync::mpsc::Sender<crate::tui::screens::hook_log::HookOutputMessage>>,
) -> Result<RemoveWithHooksResult> {
    let live = LiveWorktree {
        entry: GitWorktreeEntry {
            name: wt.name.clone(),
            path: Path::new(&wt.path).to_path_buf(),
            branch: Some(wt.branch.clone()),
            is_main: false,
        },
        metadata: Some(wt.clone()),
    };
    let _ = repo;
    execute_live_resolved_with_hooks(
        &live,
        repo_info,
        db,
        delete_branch,
        force_delete_branch,
        hooks_config,
        no_hooks,
        hook_tx,
    )
    .await
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

    fn commit_file(repo: &git2::Repository, filename: &str, content: &str, message: &str) {
        let workdir = repo.workdir().unwrap();
        std::fs::write(workdir.join(filename), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(filename)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
            .unwrap();
    }

    #[test]
    fn remove_with_delete_branch_deletes_local_branch() {
        let (clone_dir, _remote_dir) = setup_repo_with_remote();
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a worktree
        let create_result = crate::cli::commands::create::execute(
            "delete-me",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(create_result.path.exists());

        let clone = git2::Repository::open(clone_dir.path()).unwrap();
        assert!(
            clone
                .find_branch("delete-me", git2::BranchType::Local)
                .is_ok(),
            "local branch should exist before deletion"
        );

        let result = execute("delete-me", clone_dir.path(), &db, true)
            .expect("remove with local branch deletion should succeed");
        assert_eq!(result.name, "delete-me");
        assert_eq!(result.branch.as_deref(), Some("delete-me"));
        assert!(result.delete_branch_requested);
        assert!(result.branch_deleted, "branch should be deleted");
        assert!(!result.branch_delete_forced);
        assert!(result.branch_delete_error.is_none());

        // Verify: worktree directory is gone
        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );

        assert!(
            clone
                .find_branch("delete-me", git2::BranchType::Local)
                .is_err(),
            "local branch should be deleted after remove"
        );
    }

    #[test]
    fn remove_with_delete_branch_reports_unmerged_failure() {
        let (clone_dir, _remote_dir) = setup_repo_with_remote();
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "feature-unmerged",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        assert!(create_result.path.exists());
        let wt_repo = git2::Repository::open(&create_result.path).unwrap();
        commit_file(&wt_repo, "feature.txt", "feature work", "feature commit");

        let result = execute("feature-unmerged", clone_dir.path(), &db, true)
            .expect("remove should still succeed when branch delete fails");
        assert_eq!(result.name, "feature-unmerged");
        assert_eq!(result.branch.as_deref(), Some("feature-unmerged"));
        assert!(result.delete_branch_requested);
        assert!(
            !result.branch_deleted,
            "branch should remain when safe delete rejects it"
        );
        assert_eq!(result.branch_delete_forced, false);
        assert!(
            result
                .branch_delete_error
                .as_deref()
                .is_some_and(|error| error.contains("not fully merged")),
            "unmerged delete should report a merge-safety error"
        );

        assert!(
            !create_result.path.exists(),
            "worktree directory should be deleted"
        );
        let clone = git2::Repository::open(clone_dir.path()).unwrap();
        assert!(
            clone
                .find_branch("feature-unmerged", git2::BranchType::Local)
                .is_ok(),
            "branch should be preserved after safe-delete failure"
        );
    }

    #[test]
    fn remove_with_force_delete_branch_removes_unmerged_branch() {
        let (clone_dir, _remote_dir) = setup_repo_with_remote();
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "feature-force",
            None,
            clone_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");
        let wt_repo = git2::Repository::open(&create_result.path).unwrap();
        commit_file(&wt_repo, "force.txt", "force work", "force commit");

        let repo_info = git::discover_repo(clone_dir.path()).unwrap();
        let live = crate::live_worktree::resolve("feature-force", &repo_info, &db).unwrap();
        let result = execute_live_resolved(&live, &repo_info, &db, true, true)
            .expect("force delete should succeed");

        assert!(result.branch_deleted);
        assert!(result.branch_delete_forced);
        assert!(result.branch_delete_error.is_none());

        let clone = git2::Repository::open(clone_dir.path()).unwrap();
        assert!(
            clone
                .find_branch("feature-force", git2::BranchType::Local)
                .is_err(),
            "branch should be removed after force delete"
        );
    }

    #[test]
    fn remove_unmanaged_worktree_without_persisting_metadata() {
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

        // Remove the unmanaged worktree — should not persist metadata
        let result = execute("unmanaged-rm", repo_dir.path(), &db, false)
            .expect("remove of unmanaged worktree should succeed");
        assert_eq!(result.name, "unmanaged-rm");

        // Verify DB stayed clean for the unmanaged worktree path
        let db_repo = db.get_repo_by_path(repo_path_str).unwrap().unwrap();
        let wt_count: i64 = db
            .conn_for_test()
            .query_row(
                "SELECT COUNT(*) FROM worktrees WHERE repo_id = ?1 AND name = 'unmanaged-rm'",
                rusqlite::params![db_repo.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wt_count, 0, "worktree should not be inserted into DB");
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
            execute_resolved(&repo, &wt, &repo_info, &db, false, false).expect("should succeed");
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

    #[test]
    fn external_git_delete_returns_not_found_not_unique_constraint() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let create_result = crate::cli::commands::create::execute(
            "deleted-outside",
            None,
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        crate::git::remove_worktree(repo_dir.path(), &create_result.path)
            .expect("external git delete should succeed");
        assert!(!create_result.path.exists(), "worktree should be gone");

        let err = execute("deleted-outside", repo_dir.path(), &db, false)
            .expect_err("remove should not resolve stale db ghost");
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "expected not found error, got: {msg}"
        );
        assert!(
            !msg.contains("UNIQUE constraint"),
            "should not fail via stale DB adoption: {msg}"
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
        let (repo, wt) = crate::adopt::resolve_or_adopt("hooks-none", &repo_info, &db).unwrap();

        // Remove with no hooks configured
        let outcome = execute_resolved_with_hooks(
            &repo, &wt, &repo_info, &db, false, false, None,  // no hooks
            false, // no_hooks flag irrelevant
            None,
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.result.name, "hooks-none");
        assert_eq!(outcome.hooks_status, RemoveHooksStatus::None);
        assert!(outcome.post_remove_warning.is_none());
        assert!(
            !create_result.path.exists(),
            "worktree dir should be deleted"
        );
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
        let (repo, wt) = crate::adopt::resolve_or_adopt("skip-hooks", &repo_info, &db).unwrap();

        let hooks = sample_hooks_config();

        // Remove with --no-hooks
        let outcome = execute_resolved_with_hooks(
            &repo,
            &wt,
            &repo_info,
            &db,
            false,
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
        assert!(
            !create_result.path.exists(),
            "worktree dir should be deleted"
        );

        // Verify no hook events were recorded
        let wt_record = db.get_worktree(wt.id).unwrap().unwrap();
        let hook_events = db
            .count_events(wt_record.id, Some("hook:pre_remove"))
            .unwrap();
        assert_eq!(
            hook_events, 0,
            "no hook events should be recorded when --no-hooks"
        );
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
        let (repo, wt) = crate::adopt::resolve_or_adopt("pre-rm-test", &repo_info, &db).unwrap();

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
            false,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Ran);
        assert!(
            !create_result.path.exists(),
            "worktree dir should be deleted after hooks"
        );

        // Verify hook event was logged
        let hook_events = db.count_events(wt.id, Some("hook:pre_remove")).unwrap();
        assert_eq!(hook_events, 1, "pre_remove hook event should be logged");

        // Verify hook output was captured in logs
        let events = db.list_events(wt.id, 10).unwrap();
        let hook_event = events
            .iter()
            .find(|e| e.event_type == "hook:pre_remove")
            .unwrap();
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
        let (repo, wt) = crate::adopt::resolve_or_adopt("fail-pre-rm", &repo_info, &db).unwrap();

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
        let (repo, wt) = crate::adopt::resolve_or_adopt("post-rm-test", &repo_info, &db).unwrap();

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
            false,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("remove should succeed");

        assert_eq!(outcome.hooks_status, RemoveHooksStatus::Ran);
        assert!(
            !create_result.path.exists(),
            "worktree dir should be deleted"
        );
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
        let (repo, wt) = crate::adopt::resolve_or_adopt("gone-dir", &repo_info, &db).unwrap();

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
        let (repo, wt) = crate::adopt::resolve_or_adopt("post-fail", &repo_info, &db).unwrap();

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
            false, // delete_branch_requested
            false, // force
            Some(&hooks),
            false, // no_hooks
        )
        .expect("dry-run should succeed");

        assert!(plan.dry_run);
        assert_eq!(plan.name, "dry-run-test");
        assert_eq!(plan.branch, "dry-run-test");
        assert!(!plan.delete_branch_requested);
        assert!(!plan.force);
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
            true,
            Some(&hooks),
            false,
        )
        .expect("dry-run should succeed");

        let output = format!("{plan}");
        assert!(
            output.contains("Dry run"),
            "should contain 'Dry run' header"
        );
        assert!(
            output.contains("display-test"),
            "should contain worktree name"
        );
        assert!(output.contains("pre_remove"), "should show pre_remove hook");
        assert!(
            output.contains("post_remove"),
            "should show post_remove hook"
        );
        assert!(
            output.contains("Delete branch:"),
            "should mention branch delete status"
        );
        assert!(output.contains("Force:"), "should mention force status");
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
        assert_eq!(parsed["delete_branch_requested"], false);
        assert_eq!(parsed["force"], false);
        assert!(parsed["hooks"].is_object(), "hooks should be an object");
        assert!(parsed["hooks"]["pre_remove"].is_object());
        assert!(parsed["hooks"]["post_remove"].is_object());
    }

    #[test]
    fn dry_run_with_delete_branch_and_force_shows_requested_status() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("delete-branch-dry");

        let plan = execute_dry_run(
            "delete-branch-dry",
            repo_dir.path(),
            Some(&db),
            true, // delete_branch_requested
            true, // force
            None,
            false,
        )
        .expect("dry-run should succeed");

        assert!(
            plan.delete_branch_requested,
            "delete_branch_requested should be true"
        );
        assert!(plan.force, "force should be true");
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

    #[test]
    fn dry_run_does_not_resolve_stale_db_worktree() {
        let (repo_dir, _wt_root, _db_dir, db) = create_worktree_for_dry_run("stale-dry-run");
        let repo_info = crate::git::discover_repo(repo_dir.path()).unwrap();
        let live = crate::live_worktree::resolve("stale-dry-run", &repo_info, &db).unwrap();
        crate::git::remove_worktree(repo_dir.path(), &live.entry.path)
            .expect("external git delete should succeed");

        let err = execute_dry_run(
            "stale-dry-run",
            repo_dir.path(),
            Some(&db),
            false,
            false,
            None,
            false,
        )
        .expect_err("dry run should ignore stale DB-only worktree");
        assert!(
            err.to_string().contains("not found"),
            "unexpected error: {err:#}"
        );
    }
}
