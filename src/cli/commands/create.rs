use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::git;
use crate::paths;
use crate::state::Database;

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

    let repo_path_str = repo_info.path.to_str().unwrap_or_default();
    let repo = match db.get_repo_by_path(repo_path_str)? {
        Some(r) => r,
        None => db.insert_repo(&repo_info.name, repo_path_str, Some(base))?,
    };

    let sanitized_name = paths::sanitize_branch(branch);
    let worktree_path_str = worktree_path.to_str().unwrap_or_default();
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
}
