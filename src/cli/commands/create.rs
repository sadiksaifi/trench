use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::HooksConfig;
use crate::create_plan::{CreatePlan, CreatePlanner, HookPolicy};
use crate::git;
use crate::hooks::{self, HookEnvContext, HookEvent};
use crate::paths;
use crate::state::Database;

/// Typed errors for the `create` command.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    #[error("pre_create hook failed")]
    PreCreateHookFailed(#[source] anyhow::Error),
}

/// Result of a successful `trench create` operation.
#[derive(Debug)]
pub struct CreateResult {
    /// Sanitized worktree name (e.g. `feature-auth` for branch `feature/auth`).
    pub name: String,
    /// Original branch name as provided by the user.
    pub branch: String,
    /// Absolute path to the created worktree on disk.
    pub path: PathBuf,
    /// Base branch the worktree was created from.
    pub base_branch: String,
}

impl CreateResult {
    /// Convert to a JSON-serializable output struct.
    pub fn to_json_output(self, hooks: HooksStatus) -> CreateJsonOutput {
        CreateJsonOutput {
            worktree: self.name,
            branch: self.branch,
            path: self.path.to_string_lossy().to_string(),
            base_branch: self.base_branch,
            hooks,
        }
    }
}

/// JSON-serializable output for `trench create --json` (FR-35, US-4).
#[derive(Debug, serde::Serialize)]
pub struct CreateJsonOutput {
    pub worktree: String,
    pub branch: String,
    pub path: String,
    pub base_branch: String,
    pub hooks: HooksStatus,
}

/// Hook execution status included in JSON output.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum HooksStatus {
    /// No hooks were configured for this operation.
    None,
    /// Hooks were configured and executed successfully.
    Ran,
    /// Hooks were configured but skipped (e.g. `--no-hooks`).
    Skipped,
}

/// Execute a dry-run of `trench create <branch>`.
///
/// Discovers the repo and resolves the worktree path, but performs no git
/// operations, no DB writes, and no hook execution.
pub fn execute_dry_run(
    branch: &str,
    from: Option<&str>,
    cwd: &Path,
    worktree_root: &Path,
    configured_base: Option<&str>,
    no_hooks: bool,
) -> Result<CreatePlan> {
    let hook_policy = if no_hooks {
        HookPolicy::Skip
    } else {
        HookPolicy::Run
    };
    CreatePlanner::discover(cwd, worktree_root, configured_base, hook_policy)?
        .plan(branch, from)
        .map_err(Into::into)
}

fn path_to_utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
}

/// Result of `execute_with_hooks` — includes the create result, hooks status,
/// and any post_create hook error (worktree stays on post_create failure).
#[derive(Debug)]
pub struct CreateWithHooksResult {
    pub result: CreateResult,
    pub hooks_status: HooksStatus,
    /// If post_create hook failed, this contains the error.
    /// The worktree was still created successfully.
    pub post_create_error: Option<anyhow::Error>,
}

/// Execute `trench create <branch>` with lifecycle hooks.
///
/// Orchestrates: pre_create hook → worktree creation → post_create hook.
/// - If `no_hooks` is true or no hooks configured, hooks are skipped.
/// - Pre_create failure cancels the operation (worktree not created).
/// - Post_create failure: worktree stays, error captured in result.
pub async fn execute_with_hooks(
    branch: &str,
    from: Option<&str>,
    cwd: &Path,
    worktree_root: &Path,
    template: &str,
    db: &Database,
    hooks_config: Option<&HooksConfig>,
    no_hooks: bool,
    hook_tx: Option<&std::sync::mpsc::Sender<crate::tui::screens::hook_log::HookOutputMessage>>,
) -> Result<CreateWithHooksResult> {
    let has_hooks = hooks_config
        .map(|h| h.pre_create.is_some() || h.post_create.is_some())
        .unwrap_or(false);

    // Fast path: no hooks to run
    if no_hooks || !has_hooks {
        let hooks_status = if no_hooks && has_hooks {
            HooksStatus::Skipped
        } else {
            HooksStatus::None
        };
        let result = execute(branch, from, cwd, worktree_root, template, db)?;
        return Ok(CreateWithHooksResult {
            result,
            hooks_status,
            post_create_error: None,
        });
    }

    let hooks = hooks_config.unwrap(); // safe: has_hooks is true

    // Pre-compute info needed for hooks
    let repo_info = git::discover_repo(cwd)?;
    let relative_path = paths::render_worktree_path(template, &repo_info.name, branch)?;
    let worktree_path = worktree_root.join(relative_path);
    let base = from.unwrap_or(&repo_info.default_branch);
    let sanitized_name = paths::sanitize_branch(branch);

    let env_ctx = HookEnvContext {
        worktree_path: worktree_path.to_string_lossy().to_string(),
        worktree_name: sanitized_name,
        branch: branch.to_string(),
        repo_name: repo_info.name.clone(),
        repo_path: repo_info.path.to_string_lossy().to_string(),
        base_branch: base.to_string(),
    };

    // Step 1: pre_create hook (cwd = repo path, no worktree_id yet)
    if let Some(pre_create) = &hooks.pre_create {
        let emitter = hooks::types::LegacyHookEmitter::new(hook_tx);
        hooks::runner::execute_hook(
            &HookEvent::PreCreate,
            pre_create,
            &env_ctx,
            &repo_info.path,
            &repo_info.path,
            &emitter,
        )
        .await
        .map_err(CreateError::PreCreateHookFailed)?;
    }

    // Step 2: create worktree
    let result = execute(branch, from, cwd, worktree_root, template, db)?;

    // Step 3: post_create hook (cwd = worktree path)
    let post_create_error = if let Some(post_create) = &hooks.post_create {
        let emitter = hooks::types::LegacyHookEmitter::new(hook_tx);
        match hooks::runner::execute_hook(
            &HookEvent::PostCreate,
            post_create,
            &env_ctx,
            &repo_info.path,
            &result.path,
            &emitter,
        )
        .await
        {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    } else {
        None
    };

    Ok(CreateWithHooksResult {
        result,
        hooks_status: HooksStatus::Ran,
        post_create_error,
    })
}

/// Execute the `trench create <branch>` command.
///
/// Discovers the git repo, resolves the worktree path, creates the worktree
/// on disk, persists the record to SQLite, and returns the created path.
pub fn execute(
    branch: &str,
    from: Option<&str>,
    cwd: &Path,
    worktree_root: &Path,
    template: &str,
    db: &Database,
) -> Result<CreateResult> {
    let repo_info = git::discover_repo(cwd)?;
    let relative_path = paths::render_worktree_path(template, &repo_info.name, branch)?;
    let worktree_path = worktree_root.join(relative_path);
    let base = from.unwrap_or(&repo_info.default_branch);

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create worktree parent directory: {}",
                parent.display()
            )
        })?;
    }

    git::create_worktree(&repo_info.path, branch, base, &worktree_path)?;

    let repo_path_str = path_to_utf8(&repo_info.path)?;
    let repo = match db.get_repo_by_path(repo_path_str)? {
        Some(r) => r,
        None => db.insert_repo(
            &repo_info.name,
            repo_path_str,
            Some(&repo_info.default_branch),
        )?,
    };

    let sanitized_name = paths::sanitize_branch(branch);
    let canonical_worktree_path = worktree_path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", worktree_path.display()))?;
    let worktree_path_str = path_to_utf8(&canonical_worktree_path)?;
    let wt = db.insert_worktree(
        repo.id,
        &sanitized_name,
        branch,
        worktree_path_str,
        Some(base),
    )?;

    db.insert_event(repo.id, Some(wt.id), "created", None)?;

    Ok(CreateResult {
        name: sanitized_name,
        branch: branch.to_string(),
        path: canonical_worktree_path,
        base_branch: base.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HookDef, HooksConfig};

    #[test]
    fn path_to_utf8_succeeds_for_valid_utf8() {
        let p = Path::new("/tmp/some/valid/path");
        let result = path_to_utf8(p);
        assert_eq!(result.unwrap(), "/tmp/some/valid/path");
    }

    #[cfg(unix)]
    #[test]
    fn path_to_utf8_errors_on_non_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let bad = OsStr::from_bytes(&[0xff, 0xfe]);
        let p = Path::new(bad);
        let err = path_to_utf8(p).expect_err("should reject non-UTF8 path");
        let msg = err.to_string();
        assert!(
            msg.contains("not valid UTF-8"),
            "error should mention 'not valid UTF-8', got: {msg}"
        );
    }

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
    fn create_worktree_happy_path_end_to_end() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let result = execute(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let path = &result.path;

        // Worktree exists on disk
        assert!(path.exists(), "worktree directory should exist on disk");
        assert!(
            path.join(".git").exists(),
            "worktree should have .git entry"
        );

        // Path is under worktree root at expected location
        let repo_name = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let expected_path = wt_root.path().join(&repo_name).join("my-feature");
        assert_eq!(*path, expected_path.canonicalize().unwrap());

        // DB: repo record exists
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db
            .get_repo_by_path(&repo_path_str)
            .unwrap()
            .expect("repo should be persisted in DB");
        assert_eq!(db_repo.name, repo_name);

        // DB: worktree record exists with correct fields
        let worktrees = db.list_worktrees(db_repo.id).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, "my-feature");
        assert_eq!(worktrees[0].path, path.to_str().unwrap());
        assert!(worktrees[0].managed);
        assert!(worktrees[0].base_branch.is_some());
        assert!(worktrees[0].created_at > 0);

        // DB: "created" event written
        let event_count = db.count_events(worktrees[0].id, Some("created")).unwrap();
        assert_eq!(event_count, 1, "exactly one 'created' event should exist");
    }

    #[test]
    fn create_errors_when_branch_already_exists() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Pre-create a branch so it already exists
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("existing-branch", &head_commit, false).unwrap();

        let result = execute(
            "existing-branch",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        );

        let err = result.expect_err("should fail when branch exists");
        let git_err = err
            .downcast_ref::<git::GitError>()
            .expect("error should be GitError");
        assert!(
            matches!(git_err, git::GitError::BranchAlreadyExists { ref branch } if branch == "existing-branch"),
            "expected BranchAlreadyExists, got: {git_err:?}"
        );
    }

    #[test]
    fn create_errors_when_branch_exists_on_remote() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a remote tracking ref (origin/remote-branch)
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let remote_oid = repo
            .commit(None, &sig, &sig, "remote commit", &tree, &[&head])
            .unwrap();
        repo.reference(
            "refs/remotes/origin/remote-branch",
            remote_oid,
            false,
            "fake remote tracking branch",
        )
        .unwrap();

        let result = execute(
            "remote-branch",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        );

        let err = result.expect_err("should fail when branch exists on remote");
        let git_err = err
            .downcast_ref::<git::GitError>()
            .expect("error should be GitError");
        assert!(
            matches!(git_err, git::GitError::RemoteBranchAlreadyExists { ref branch, .. } if branch == "remote-branch"),
            "expected RemoteBranchAlreadyExists, got: {git_err:?}"
        );
    }

    #[test]
    fn two_worktrees_in_same_repo_share_one_repo_record() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        execute(
            "feature-a",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("first create should succeed");

        execute(
            "feature-b",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("second create should succeed");

        // Only one repo record in DB
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db
            .get_repo_by_path(&repo_path_str)
            .unwrap()
            .expect("repo should exist");

        // Two worktree records under the same repo
        let worktrees = db.list_worktrees(db_repo.id).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch, "feature-a");
        assert_eq!(worktrees[1].branch, "feature-b");
    }

    #[test]
    fn create_from_nondefault_base_has_correct_commit_ancestry() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create a "develop" branch with an extra commit so it diverges from HEAD
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        let develop_branch = repo.branch("develop", &head_commit, false).unwrap();
        let develop_oid = {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree = repo
                .find_tree(repo.index().unwrap().write_tree().unwrap())
                .unwrap();
            // Commit on develop — now develop is 1 commit ahead of HEAD
            let develop_tip = develop_branch.get().peel_to_commit().unwrap();
            repo.commit(
                Some("refs/heads/develop"),
                &sig,
                &sig,
                "develop commit",
                &tree,
                &[&develop_tip],
            )
            .unwrap()
        };

        let result = execute(
            "my-feature",
            Some("develop"),
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create with --from develop should succeed");

        // Open the worktree as a repo and verify its HEAD commit matches develop's tip
        let wt_repo = git2::Repository::open(&result.path).unwrap();
        let wt_head_oid = wt_repo.head().unwrap().peel_to_commit().unwrap().id();
        assert_eq!(
            wt_head_oid, develop_oid,
            "worktree HEAD should match the develop branch's tip commit"
        );
    }

    #[test]
    fn create_errors_when_branch_exists_on_real_remote() {
        // Set up a bare "origin" repo with a commit created directly in it
        let origin_dir = tempfile::tempdir().unwrap();
        let origin = git2::Repository::init_bare(origin_dir.path()).unwrap();
        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = origin.treebuilder(None).unwrap().write().unwrap();
            let tree = origin.find_tree(tree_id).unwrap();
            let oid = origin
                .commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
            origin.set_head("refs/heads/main").unwrap();

            // Create a branch on origin that will conflict
            origin
                .reference("refs/heads/taken-remote", oid, true, "conflicting branch")
                .unwrap();
        }

        // Clone origin into a local working repo
        let local_dir = tempfile::tempdir().unwrap();
        let local =
            git2::Repository::clone(origin_dir.path().to_str().unwrap(), local_dir.path()).unwrap();

        // Verify the remote tracking branch exists locally
        assert!(
            local
                .find_branch("origin/taken-remote", git2::BranchType::Remote)
                .is_ok(),
            "origin/taken-remote should exist as a remote tracking branch after clone"
        );

        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let result = execute(
            "taken-remote",
            None,
            local_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        );

        let err = result.expect_err("should fail when branch exists on real remote");
        let git_err = err
            .downcast_ref::<git::GitError>()
            .expect("error should be GitError");
        assert!(
            matches!(git_err, git::GitError::RemoteBranchAlreadyExists { ref branch, ref remote }
                if branch == "taken-remote" && remote == "origin"),
            "expected RemoteBranchAlreadyExists for 'taken-remote', got: {git_err:?}"
        );

        // Verify no worktree was created
        let expected_wt_path = wt_root
            .path()
            .join(
                local_dir
                    .path()
                    .canonicalize()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap(),
            )
            .join("taken-remote");
        assert!(
            !expected_wt_path.exists(),
            "worktree directory should NOT be created"
        );
    }

    #[test]
    fn dry_run_returns_plan_with_correct_fields_and_no_side_effects() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let outside = tempfile::tempdir().unwrap();
        let wt_root = outside.path().join("missing-worktree-root");

        let plan = execute_dry_run("feature/auth", None, repo_dir.path(), &wt_root, None, false)
            .expect("dry-run should succeed");

        assert_eq!(plan.branch, "feature/auth");
        assert_eq!(plan.worktree, "feature-auth");
        assert_eq!(plan.base.as_deref(), Some("main"));
        assert!(!wt_root.exists(), "dry-run must not create its root");
    }

    #[test]
    fn execute_returns_create_result_with_correct_fields() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let result = execute(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        assert_eq!(result.branch, "my-feature");
        assert_eq!(result.name, "my-feature");
        assert!(result.path.exists(), "worktree path should exist on disk");
        assert!(!result.base_branch.is_empty(), "base_branch should be set");
    }

    #[test]
    fn create_result_serializes_to_json_with_all_required_fields() {
        use crate::output::json::format_json_value;

        let result = CreateResult {
            name: "my-feature".to_string(),
            branch: "my-feature".to_string(),
            path: std::path::PathBuf::from("/home/.worktrees/repo/my-feature"),
            base_branch: "main".to_string(),
        };

        let hooks = HooksStatus::None;
        let json_output = result.to_json_output(hooks);
        let json_str = format_json_value(&json_output).expect("should serialize to JSON");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("should be valid JSON");

        assert_eq!(parsed["worktree"], "my-feature");
        assert_eq!(parsed["branch"], "my-feature");
        assert_eq!(parsed["path"], "/home/.worktrees/repo/my-feature");
        assert_eq!(parsed["base_branch"], "main");
        assert_eq!(parsed["hooks"]["status"], "none");
    }

    #[test]
    fn integration_create_json_output_matches_real_worktree() {
        use crate::output::json::format_json_value;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let result = execute(
            "json-test",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let json_output = result.to_json_output(HooksStatus::None);
        let json_str = format_json_value(&json_output).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("valid JSON");

        // worktree name is the sanitized branch name
        assert_eq!(parsed["worktree"], "json-test");
        // branch is the original branch name
        assert_eq!(parsed["branch"], "json-test");
        // path is a real path on disk that exists
        let path_str = parsed["path"].as_str().unwrap();
        assert!(
            std::path::Path::new(path_str).exists(),
            "path should exist on disk"
        );
        // base_branch is set to the repo's default
        assert!(!parsed["base_branch"].as_str().unwrap().is_empty());
        // hooks status reflects no hooks configured
        assert_eq!(parsed["hooks"]["status"], "none");
    }

    #[test]
    fn dry_run_with_from_shows_custom_base() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();

        // Create a "develop" branch so --from has something valid
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("develop", &head_commit, false).unwrap();

        let plan = execute_dry_run(
            "my-feature",
            Some("develop"),
            repo_dir.path(),
            wt_root.path(),
            None,
            false,
        )
        .expect("dry-run with --from should succeed");

        assert_eq!(
            plan.base.as_deref(),
            Some("develop"),
            "base should reflect --from override"
        );
    }

    #[test]
    fn create_with_from_stores_default_branch_not_from_override() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Determine HEAD branch name (the repo's true default)
        let head_branch = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a second branch "develop" to use as --from
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("develop", &head_commit, false).unwrap();

        let _result = execute(
            "my-feature",
            Some("develop"),
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create with --from should succeed");

        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db
            .get_repo_by_path(&repo_path_str)
            .unwrap()
            .expect("repo should be in DB");

        assert_eq!(
            db_repo.default_base.as_deref(),
            Some(head_branch.as_str()),
            "repos.default_base should be the HEAD branch, not the --from override"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_with_hooks_no_hooks_configured_returns_none_status() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let result = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            None,  // no hooks configured
            false, // no_hooks flag = false
            None,
        )
        .await
        .expect("should succeed");

        assert!(matches!(result.hooks_status, HooksStatus::None));
        assert!(result.result.path.exists(), "worktree should be created");
        assert!(result.post_create_error.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_with_hooks_no_hooks_flag_returns_skipped_status() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let hooks = HooksConfig {
            post_create: Some(HookDef {
                run: Some(vec!["echo should-not-run".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let result = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            true, // no_hooks = true → skip
            None,
        )
        .await
        .expect("should succeed");

        assert!(matches!(result.hooks_status, HooksStatus::Skipped));
        assert!(
            result.result.path.exists(),
            "worktree should still be created"
        );

        // Verify no hook events were logged
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db
            .get_repo_by_path(&repo_path_str)
            .unwrap()
            .expect("repo in DB");
        let wts = db.list_worktrees(db_repo.id).unwrap();
        let hook_events = db
            .count_events(wts[0].id, Some("hook:post_create"))
            .unwrap();
        assert_eq!(
            hook_events, 0,
            "no hook events should be logged when --no-hooks"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_create_hook_runs_before_worktree_creation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // pre_create hook creates a marker file in repo dir
        let marker = repo_dir.path().join("pre_create_ran.marker");
        let hooks = HooksConfig {
            pre_create: Some(HookDef {
                run: Some(vec![format!("touch {}", marker.display())]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let result = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("should succeed");

        // Marker file should exist (pre_create hook ran)
        assert!(marker.exists(), "pre_create hook should have run");

        // Worktree should also exist
        assert!(result.result.path.exists(), "worktree should be created");
        assert!(matches!(result.hooks_status, HooksStatus::Ran));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_create_failure_cancels_worktree_creation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let hooks = HooksConfig {
            pre_create: Some(HookDef {
                run: Some(vec!["exit 1".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let err = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect_err("should fail when pre_create hook fails");

        // Error should be a typed CreateError::PreCreateHookFailed
        assert!(
            err.downcast_ref::<CreateError>().is_some(),
            "expected CreateError::PreCreateHookFailed, got: {err:?}"
        );

        // Worktree should NOT exist on disk
        let repo_name = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let expected_wt_path = wt_root.path().join(&repo_name).join("my-feature");
        assert!(
            !expected_wt_path.exists(),
            "worktree should NOT be created when pre_create fails"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_create_hook_runs_after_worktree_creation() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // post_create hook creates a marker file in the worktree dir
        let hooks = HooksConfig {
            post_create: Some(HookDef {
                run: Some(vec!["touch post_create_ran.marker".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let result = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("should succeed");

        // Worktree exists
        assert!(result.result.path.exists(), "worktree should be created");

        // Marker file in worktree dir proves post_create ran with cwd = worktree
        let marker = result.result.path.join("post_create_ran.marker");
        assert!(
            marker.exists(),
            "post_create hook should have run in worktree dir"
        );

        assert!(matches!(result.hooks_status, HooksStatus::Ran));
        assert!(result.post_create_error.is_none());

        // Hook event logged to DB
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db
            .get_repo_by_path(&repo_path_str)
            .unwrap()
            .expect("repo in DB");
        let wts = db.list_worktrees(db_repo.id).unwrap();
        let hook_events = db
            .count_events(wts[0].id, Some("hook:post_create"))
            .unwrap();
        assert_eq!(hook_events, 1, "post_create hook event should be logged");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_create_failure_keeps_worktree_and_reports_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let hooks = HooksConfig {
            post_create: Some(HookDef {
                run: Some(vec!["exit 1".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let result = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("should succeed (worktree stays despite hook failure)");

        // Worktree should still exist on disk
        assert!(
            result.result.path.exists(),
            "worktree should exist despite post_create failure"
        );

        // post_create_error should be set
        assert!(
            result.post_create_error.is_some(),
            "post_create_error should contain the hook failure"
        );

        assert!(matches!(result.hooks_status, HooksStatus::Ran));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn integration_create_with_hooks_copies_files_and_runs_commands() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        // Create files in repo that should be copied by post_create
        std::fs::write(
            repo_dir.path().join(".env"),
            "DATABASE_URL=postgres://localhost/mydb",
        )
        .unwrap();
        std::fs::write(repo_dir.path().join(".env.local"), "SECRET=abc123").unwrap();
        std::fs::write(repo_dir.path().join(".env.example"), "DATABASE_URL=").unwrap();
        std::fs::write(repo_dir.path().join("README.md"), "# readme").unwrap();

        let hooks = HooksConfig {
            pre_create: Some(HookDef {
                run: Some(vec!["echo pre_create_ok".to_string()]),
                ..HookDef::default()
            }),
            post_create: Some(HookDef {
                copy: Some(vec![".env*".to_string(), "!.env.example".to_string()]),
                run: Some(vec!["echo post_create_ok".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let result = execute_with_hooks(
            "integration-test",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect("should succeed");

        let wt_path = &result.result.path;

        // Worktree created
        assert!(wt_path.exists(), "worktree should exist");
        assert!(wt_path.join(".git").exists(), "worktree should have .git");

        // Copy: .env and .env.local copied, .env.example excluded, README.md not matched
        assert!(
            wt_path.join(".env").exists(),
            ".env should be copied to worktree"
        );
        assert_eq!(
            std::fs::read_to_string(wt_path.join(".env")).unwrap(),
            "DATABASE_URL=postgres://localhost/mydb"
        );
        assert!(
            wt_path.join(".env.local").exists(),
            ".env.local should be copied to worktree"
        );
        assert!(
            !wt_path.join(".env.example").exists(),
            ".env.example should be excluded by !.env.example pattern"
        );
        // README.md doesn't match .env* so it shouldn't be copied as an extra file
        // (it may exist from git checkout, which is fine — we just verify the copy step)

        // Status
        assert!(matches!(result.hooks_status, HooksStatus::Ran));
        assert!(result.post_create_error.is_none());

        // DB: hook events logged
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db
            .get_repo_by_path(&repo_path_str)
            .unwrap()
            .expect("repo in DB");
        let wts = db.list_worktrees(db_repo.id).unwrap();
        let post_hook_count = db
            .count_events(wts[0].id, Some("hook:post_create"))
            .unwrap();
        assert_eq!(
            post_hook_count, 1,
            "post_create hook event should be logged"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pre_create_failure_returns_typed_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let hooks = HooksConfig {
            pre_create: Some(HookDef {
                run: Some(vec!["exit 1".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let err = execute_with_hooks(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
            Some(&hooks),
            false,
            None,
        )
        .await
        .expect_err("should fail when pre_create hook fails");

        // The error must be a typed CreateError::PreCreateHookFailed, not a string
        assert!(
            err.downcast_ref::<CreateError>().is_some(),
            "expected CreateError::PreCreateHookFailed, got: {err:?}"
        );
    }
}
