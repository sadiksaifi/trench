use std::path::{Path, PathBuf};

/// Information about a discovered git repository.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoInfo {
    pub name: String,
    pub path: PathBuf,
    pub remote_url: Option<String>,
    pub default_branch: String,
}

/// Errors specific to git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("not a git repository: {path}")]
    NotAGitRepo { path: PathBuf },

    #[error("branch already exists: {branch}")]
    BranchAlreadyExists { branch: String },

    #[error("base branch not found: {base}")]
    BaseBranchNotFound { base: String },

    #[error("{0}")]
    Git(#[from] git2::Error),
}

/// Map a git2 error to the appropriate `GitError`.
///
/// Returns `NotAGitRepo` when the error code is `NotFound`, preserving the
/// original `git2::Error` for all other failure modes.
fn map_repo_open_error(e: git2::Error, path: &Path) -> GitError {
    if e.code() == git2::ErrorCode::NotFound {
        GitError::NotAGitRepo {
            path: path.to_path_buf(),
        }
    } else {
        GitError::Git(e)
    }
}

/// Discover a git repository by walking up from the given path.
///
/// Returns a `RepoInfo` with the repo name (derived from the working directory),
/// the canonical repo path, optional origin remote URL, and the default branch.
pub fn discover_repo(path: &Path) -> Result<RepoInfo, GitError> {
    let repo =
        git2::Repository::discover(path).map_err(|e| map_repo_open_error(e, path))?;

    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::NotAGitRepo {
            path: path.to_path_buf(),
        })?
        .canonicalize()
        .map_err(|_| GitError::NotAGitRepo {
            path: path.to_path_buf(),
        })?;

    let name = workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("repo"));

    // Extract origin remote URL if present
    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(String::from));

    // Extract default branch from HEAD
    let default_branch = repo
        .head()
        .ok()
        .and_then(|r| r.shorthand().map(String::from))
        .unwrap_or_else(|| String::from("main"));

    Ok(RepoInfo {
        name,
        path: workdir,
        remote_url,
        default_branch,
    })
}

/// Create a new git worktree at `target_path` for the given branch.
///
/// Opens the repository at `repo_path`, creates the branch from `base` if it
/// doesn't exist locally, and adds a worktree at `target_path`.
///
/// Returns `GitError::BranchAlreadyExists` if the branch already exists.
pub fn create_worktree(
    repo_path: &Path,
    branch: &str,
    base: &str,
    target_path: &Path,
) -> Result<(), GitError> {
    let repo =
        git2::Repository::open(repo_path).map_err(|e| map_repo_open_error(e, repo_path))?;

    // Check if branch already exists locally
    if repo
        .find_branch(branch, git2::BranchType::Local)
        .is_ok()
    {
        return Err(GitError::BranchAlreadyExists {
            branch: branch.to_string(),
        });
    }

    // Resolve base branch to a commit (try local, then remote tracking)
    let base_commit = if let Ok(local) = repo.find_branch(base, git2::BranchType::Local) {
        local.get().peel_to_commit()?
    } else {
        // Try remote tracking branch: origin/<base>
        let remote_name = format!("origin/{base}");
        match repo.find_branch(&remote_name, git2::BranchType::Remote) {
            Ok(remote) => remote.get().peel_to_commit()?,
            Err(_) => {
                return Err(GitError::BaseBranchNotFound {
                    base: base.to_string(),
                });
            }
        }
    };

    // Create the new branch from base and add the worktree.
    // If worktree creation fails, clean up the orphaned branch.
    let worktree_result = {
        let new_branch = repo.branch(branch, &base_commit, false)?;
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(new_branch.get()));
        repo.worktree(branch, target_path, Some(&opts))
    };

    if let Err(e) = worktree_result {
        if let Ok(mut orphan) = repo.find_branch(branch, git2::BranchType::Local) {
            let _ = orphan.delete();
        }
        return Err(GitError::Git(e));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Helper: get the default branch name from HEAD.
    fn head_branch(repo: &git2::Repository) -> String {
        repo.head().unwrap().shorthand().unwrap().to_string()
    }

    #[test]
    fn repo_info_supports_equality() {
        let a = RepoInfo {
            name: "repo".into(),
            path: PathBuf::from("/tmp/repo"),
            remote_url: Some("https://github.com/test/repo.git".into()),
            default_branch: "main".into(),
        };
        let b = RepoInfo {
            name: "repo".into(),
            path: PathBuf::from("/tmp/repo"),
            remote_url: Some("https://github.com/test/repo.git".into()),
            default_branch: "main".into(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn discover_repo_name_is_nonempty() {
        let tmp = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(tmp.path());

        let info = discover_repo(tmp.path()).expect("should discover repo");

        assert!(!info.name.is_empty(), "repo name must never be empty");
    }

    #[test]
    fn discover_repo_on_nonexistent_path_returns_not_a_git_repo() {
        let result = discover_repo(Path::new("/tmp/nonexistent_path_xyz_abc"));

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), GitError::NotAGitRepo { .. }),
            "nonexistent path should yield NotAGitRepo"
        );
    }

    #[test]
    fn create_worktree_on_nonexistent_repo_returns_not_a_git_repo() {
        let result = create_worktree(
            Path::new("/tmp/nonexistent_repo_xyz_abc"),
            "branch",
            "main",
            Path::new("/tmp/wt"),
        );

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), GitError::NotAGitRepo { .. }),
            "nonexistent repo path should yield NotAGitRepo"
        );
    }

    #[test]
    fn discover_repo_finds_repo_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(tmp.path());

        let info = discover_repo(tmp.path()).expect("should discover repo");

        assert_eq!(info.path, tmp.path().canonicalize().unwrap());
        // Name is derived from directory name
        let expected_name = tmp.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(info.name, expected_name);
    }

    #[test]
    fn discover_repo_finds_repo_from_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(tmp.path());

        // Create a nested subdirectory
        let subdir = tmp.path().join("src").join("deep");
        std::fs::create_dir_all(&subdir).unwrap();

        let info = discover_repo(&subdir).expect("should discover repo from subdir");

        assert_eq!(info.path, tmp.path().canonicalize().unwrap());
        let expected_name = tmp.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(info.name, expected_name);
    }

    #[test]
    fn discover_repo_extracts_remote_url() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());
        repo.remote("origin", "https://github.com/test/repo.git")
            .unwrap();

        let info = discover_repo(tmp.path()).expect("should discover repo");

        assert_eq!(
            info.remote_url.as_deref(),
            Some("https://github.com/test/repo.git")
        );
    }

    #[test]
    fn discover_repo_remote_url_is_none_without_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(tmp.path());

        let info = discover_repo(tmp.path()).expect("should discover repo");

        assert_eq!(info.remote_url, None);
    }

    #[test]
    fn discover_repo_extracts_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(tmp.path());
        // git init defaults to "master" in git2 unless configured otherwise

        let info = discover_repo(tmp.path()).expect("should discover repo");

        // The default branch should be whatever HEAD points to
        assert!(
            !info.default_branch.is_empty(),
            "default_branch should not be empty"
        );
    }

    #[test]
    fn discover_repo_fails_for_non_git_directory() {
        let tmp = tempfile::tempdir().unwrap();
        // No git init — just a plain directory

        let result = discover_repo(tmp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GitError::NotAGitRepo { .. }),
            "expected NotAGitRepo, got: {err:?}"
        );
    }

    #[test]
    fn create_worktree_creates_directory_on_disk() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("my-feature");

        create_worktree(repo_dir.path(), "my-feature", &base, &target)
            .expect("should create worktree");

        assert!(target.exists(), "worktree directory should exist on disk");
    }

    #[test]
    fn create_worktree_creates_branch_from_base() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let base_oid = repo
            .find_branch(&base, git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .id();
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("new-branch");

        create_worktree(repo_dir.path(), "new-branch", &base, &target)
            .expect("should create worktree");

        // The new branch should exist in the repo and point to the same commit as base
        let new_branch = repo
            .find_branch("new-branch", git2::BranchType::Local)
            .expect("branch should exist");
        let new_oid = new_branch.get().peel_to_commit().unwrap().id();
        assert_eq!(new_oid, base_oid, "new branch should point to base commit");

        // The worktree should have a .git file (worktrees use a .git file, not directory)
        assert!(
            target.join(".git").exists(),
            "worktree should have .git entry"
        );
    }

    #[test]
    fn create_worktree_cleans_up_branch_on_worktree_failure() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("will-fail");

        // Place a regular file at the target path so worktree creation fails
        std::fs::write(&target, "blocker").unwrap();

        let result = create_worktree(repo_dir.path(), "will-fail", &base, &target);

        assert!(result.is_err(), "should fail when target path is occupied");

        // The orphaned branch must have been cleaned up
        assert!(
            repo.find_branch("will-fail", git2::BranchType::Local)
                .is_err(),
            "branch should be deleted after worktree creation failure"
        );
    }

    #[test]
    fn create_worktree_resolves_base_from_remote_tracking_branch() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();

        // Create a distinct commit to use as the remote tracking branch tip
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let release_oid = repo
            .commit(None, &sig, &sig, "release commit", &tree, &[&head])
            .unwrap();

        // Manually create a remote tracking ref (origin/release) without an actual remote
        repo.reference(
            "refs/remotes/origin/release",
            release_oid,
            false,
            "fake remote tracking branch for test",
        )
        .unwrap();

        // Verify: "release" does NOT exist locally, only as remote tracking
        assert!(
            repo.find_branch("release", git2::BranchType::Local).is_err(),
            "release should not exist as a local branch"
        );
        assert!(
            repo.find_branch("origin/release", git2::BranchType::Remote).is_ok(),
            "origin/release should exist as a remote tracking branch"
        );

        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("my-feature");

        // Use "release" as base — should resolve via remote tracking
        let result = create_worktree(repo_dir.path(), "my-feature", "release", &target);
        assert!(
            result.is_ok(),
            "should resolve base from remote tracking branch, got: {:?}",
            result.unwrap_err()
        );
        assert!(target.exists(), "worktree directory should exist");

        // Verify the new branch's commit matches origin/release
        let feature_oid = repo
            .find_branch("my-feature", git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap()
            .id();
        assert_eq!(
            feature_oid, release_oid,
            "new branch should point to the same commit as origin/release"
        );
    }

    #[test]
    fn create_worktree_errors_when_base_branch_does_not_exist() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("feature");

        let result = create_worktree(repo_dir.path(), "feature", "nonexistent-base", &target);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GitError::BaseBranchNotFound { ref base } if base == "nonexistent-base"),
            "missing base branch should yield BaseBranchNotFound, got: {err:?}"
        );
    }

    #[test]
    fn create_worktree_errors_when_target_path_already_exists() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("occupied");

        // Create a directory at the target path
        std::fs::create_dir_all(&target).unwrap();

        let result = create_worktree(repo_dir.path(), "occupied", &base, &target);

        assert!(result.is_err(), "should fail when target path already exists");
    }

    #[test]
    fn create_worktree_errors_when_branch_already_exists() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);

        // Create a branch that already exists
        let base_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("existing-branch", &base_commit, false).unwrap();

        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("existing-branch");

        let result = create_worktree(repo_dir.path(), "existing-branch", &base, &target);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GitError::BranchAlreadyExists { ref branch } if branch == "existing-branch"),
            "expected BranchAlreadyExists, got: {err:?}"
        );
        assert!(
            !target.exists(),
            "worktree directory should NOT be created"
        );
    }
}
