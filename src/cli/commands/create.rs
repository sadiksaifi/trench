use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::HooksConfig;
use crate::git;
use crate::paths;
use crate::state::Database;

/// Plan produced by `--dry-run` showing what `trench create` would do.
#[derive(Debug, serde::Serialize)]
pub struct DryRunPlan {
    /// Always `true` — signals this is a preview, not a real operation.
    pub dry_run: bool,
    pub branch: String,
    pub base_branch: String,
    pub worktree_path: String,
    pub repo_name: String,
    pub hooks: Option<HooksConfig>,
}

impl fmt::Display for DryRunPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Dry run — no changes will be made\n")?;
        writeln!(f, "  Branch:    {}", self.branch)?;
        writeln!(f, "  Base:      {}", self.base_branch)?;
        writeln!(f, "  Worktree:  {}", self.worktree_path)?;

        match &self.hooks {
            Some(hooks) => {
                writeln!(f, "  Hooks:")?;
                if let Some(h) = &hooks.pre_create {
                    writeln!(f, "    pre_create:")?;
                    format_hook_def(f, h)?;
                }
                if let Some(h) = &hooks.post_create {
                    writeln!(f, "    post_create:")?;
                    format_hook_def(f, h)?;
                }
            }
            None => {
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
    Ok(())
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
    template: &str,
    hooks: Option<&HooksConfig>,
) -> Result<DryRunPlan> {
    let repo_info = git::discover_repo(cwd)?;
    let relative_path = paths::render_worktree_path(template, &repo_info.name, branch)?;
    let worktree_path = worktree_root.join(relative_path);
    let base = from.unwrap_or(&repo_info.default_branch);

    Ok(DryRunPlan {
        dry_run: true,
        branch: branch.to_string(),
        base_branch: base.to_string(),
        worktree_path: worktree_path.to_string_lossy().to_string(),
        repo_name: repo_info.name.clone(),
        hooks: hooks.cloned(),
    })
}

fn path_to_utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
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
) -> Result<PathBuf> {
    let repo_info = git::discover_repo(cwd)?;
    let relative_path = paths::render_worktree_path(template, &repo_info.name, branch)?;
    let worktree_path = worktree_root.join(relative_path);
    let base = from.unwrap_or(&repo_info.default_branch);

    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create worktree parent directory: {}", parent.display()))?;
    }

    git::create_worktree(&repo_info.path, branch, base, &worktree_path)?;

    let repo_path_str = path_to_utf8(&repo_info.path)?;
    let repo = match db.get_repo_by_path(repo_path_str)? {
        Some(r) => r,
        None => db.insert_repo(&repo_info.name, repo_path_str, Some(&repo_info.default_branch))?,
    };

    let sanitized_name = paths::sanitize_branch(branch);
    let worktree_path_str = path_to_utf8(&worktree_path)?;
    let wt = db.insert_worktree(repo.id, &sanitized_name, branch, worktree_path_str, Some(base))?;

    db.insert_event(repo.id, Some(wt.id), "created", None)?;

    Ok(worktree_path)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let path = execute(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        // Worktree exists on disk
        assert!(path.exists(), "worktree directory should exist on disk");
        assert!(path.join(".git").exists(), "worktree should have .git entry");

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
        assert_eq!(path, expected_path);

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
        let event_count = db
            .count_events(worktrees[0].id, Some("created"))
            .unwrap();
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
            let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
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

        let path = execute(
            "my-feature",
            Some("develop"),
            repo_dir.path(),
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create with --from develop should succeed");

        // Open the worktree as a repo and verify its HEAD commit matches develop's tip
        let wt_repo = git2::Repository::open(&path).unwrap();
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
        let local = git2::Repository::clone(
            origin_dir.path().to_str().unwrap(),
            local_dir.path(),
        )
        .unwrap();

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
        let wt_root = tempfile::tempdir().unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();

        let plan = execute_dry_run(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            None,
        )
        .expect("dry-run should succeed");

        // Plan fields are correct
        assert_eq!(plan.branch, "my-feature");
        assert!(!plan.base_branch.is_empty(), "base_branch should be set");
        assert!(
            plan.worktree_path.contains("my-feature"),
            "worktree_path should contain branch name"
        );
        assert!(!plan.repo_name.is_empty(), "repo_name should be set");

        // No side effects: no worktree on disk
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
        assert!(
            !expected_path.exists(),
            "worktree directory should NOT be created on disk during dry-run"
        );

        // No side effects: no DB records
        let repo_path_str = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let db_repo = db.get_repo_by_path(&repo_path_str).unwrap();
        assert!(
            db_repo.is_none(),
            "no repo record should be inserted during dry-run"
        );
    }

    #[test]
    fn dry_run_plan_formats_as_readable_text() {
        let plan = DryRunPlan {
            dry_run: true,
            branch: "my-feature".to_string(),
            base_branch: "main".to_string(),
            worktree_path: "/home/.worktrees/repo/my-feature".to_string(),
            repo_name: "repo".to_string(),
            hooks: None,
        };

        let text = plan.to_string();
        assert!(text.contains("my-feature"), "should contain branch name");
        assert!(text.contains("main"), "should contain base branch");
        assert!(
            text.contains("/home/.worktrees/repo/my-feature"),
            "should contain worktree path"
        );
        assert!(
            text.contains("dry run") || text.contains("Dry run") || text.contains("DRY RUN"),
            "should indicate this is a dry run"
        );
    }

    #[test]
    fn dry_run_plan_serializes_to_json_with_expected_fields() {
        let plan = DryRunPlan {
            dry_run: true,
            branch: "my-feature".to_string(),
            base_branch: "main".to_string(),
            worktree_path: "/home/.worktrees/repo/my-feature".to_string(),
            repo_name: "repo".to_string(),
            hooks: None,
        };

        let json: serde_json::Value =
            serde_json::to_value(&plan).expect("should serialize to JSON");

        assert_eq!(json["dry_run"], true);
        assert_eq!(json["branch"], "my-feature");
        assert_eq!(json["base_branch"], "main");
        assert_eq!(json["worktree_path"], "/home/.worktrees/repo/my-feature");
        assert!(json["hooks"].is_null() || json["hooks"].is_object());
    }

    #[test]
    fn dry_run_includes_hooks_when_configured() {
        use crate::config::{HookDef, HooksConfig};

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();

        let hooks = HooksConfig {
            post_create: Some(HookDef {
                copy: Some(vec![".env*".to_string()]),
                run: Some(vec!["bun install".to_string()]),
                ..HookDef::default()
            }),
            pre_create: Some(HookDef {
                run: Some(vec!["echo pre".to_string()]),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };

        let plan = execute_dry_run(
            "my-feature",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            Some(&hooks),
        )
        .expect("dry-run should succeed");

        let plan_hooks = plan.hooks.expect("hooks should be present in plan");
        let post_create = plan_hooks
            .post_create
            .expect("post_create should be present");
        assert_eq!(
            post_create.run,
            Some(vec!["bun install".to_string()])
        );
        assert_eq!(
            post_create.copy,
            Some(vec![".env*".to_string()])
        );

        let pre_create = plan_hooks
            .pre_create
            .expect("pre_create should be present");
        assert_eq!(
            pre_create.run,
            Some(vec!["echo pre".to_string()])
        );
    }

    #[test]
    fn dry_run_includes_hooks_in_text_output() {
        use crate::config::{HookDef, HooksConfig};

        let plan = DryRunPlan {
            dry_run: true,
            branch: "foo".to_string(),
            base_branch: "main".to_string(),
            worktree_path: "/tmp/wt/foo".to_string(),
            repo_name: "repo".to_string(),
            hooks: Some(HooksConfig {
                post_create: Some(HookDef {
                    copy: Some(vec![".env*".to_string()]),
                    run: Some(vec!["bun install".to_string()]),
                    ..HookDef::default()
                }),
                ..HooksConfig::default()
            }),
        };

        let text = plan.to_string();
        assert!(text.contains("post_create"), "should mention post_create hook");
        assert!(text.contains("bun install"), "should list run commands");
        assert!(text.contains(".env*"), "should list copy patterns");
    }

    #[test]
    fn dry_run_includes_hooks_in_json_output() {
        use crate::config::{HookDef, HooksConfig};

        let plan = DryRunPlan {
            dry_run: true,
            branch: "foo".to_string(),
            base_branch: "main".to_string(),
            worktree_path: "/tmp/wt/foo".to_string(),
            repo_name: "repo".to_string(),
            hooks: Some(HooksConfig {
                post_create: Some(HookDef {
                    run: Some(vec!["bun install".to_string()]),
                    ..HookDef::default()
                }),
                ..HooksConfig::default()
            }),
        };

        let json: serde_json::Value = serde_json::to_value(&plan).unwrap();
        let hooks = &json["hooks"];
        assert!(hooks.is_object(), "hooks should be an object");
        let post_create = &hooks["post_create"];
        assert_eq!(post_create["run"][0], "bun install");
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

        let _path = execute(
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
}
