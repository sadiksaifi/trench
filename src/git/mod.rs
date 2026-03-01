use std::path::{Path, PathBuf};

/// Information about a discovered git repository.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoInfo {
    pub name: String,
    pub path: PathBuf,
    pub remote_url: Option<String>,
    pub default_branch: String,
}

/// Count modified, staged, and untracked files in a worktree.
///
/// Opens the repository at `worktree_path` and counts all files with
/// non-clean status (modified, new, deleted, renamed, typechanged).
/// Returns 0 for a clean worktree.
pub fn dirty_count(worktree_path: &Path) -> Result<usize, GitError> {
    let repo = git2::Repository::open(worktree_path)
        .map_err(|e| map_repo_open_error(e, worktree_path))?;

    let statuses = repo.statuses(Some(
        git2::StatusOptions::new()
            .include_untracked(true)
            .recurse_untracked_dirs(true),
    ))?;

    Ok(statuses.len())
}

/// Calculate commits ahead/behind for a branch relative to its upstream.
///
/// Checks for an upstream tracking branch first, then falls back to
/// `base_branch`. Returns `None` if no reference point can be found.
pub fn ahead_behind(
    repo_path: &Path,
    branch: &str,
    base_branch: Option<&str>,
) -> Result<Option<(usize, usize)>, GitError> {
    let repo =
        git2::Repository::open(repo_path).map_err(|e| map_repo_open_error(e, repo_path))?;

    let local = match repo.find_branch(branch, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let local_oid = match local.get().target() {
        Some(oid) => oid,
        None => return Ok(None),
    };

    // Try upstream tracking branch first
    let upstream_oid = if let Ok(upstream) = local.upstream() {
        upstream.get().target()
    } else {
        // Fall back to base_branch
        base_branch.and_then(|base| {
            repo.find_branch(base, git2::BranchType::Local)
                .ok()
                .and_then(|b| b.get().target())
                .or_else(|| {
                    let remote = format!("origin/{base}");
                    repo.find_branch(&remote, git2::BranchType::Remote)
                        .ok()
                        .and_then(|b| b.get().target())
                })
        })
    };

    match upstream_oid {
        Some(oid) => {
            let (ahead, behind) = repo.graph_ahead_behind(local_oid, oid)?;
            Ok(Some((ahead, behind)))
        }
        None => Ok(None),
    }
}

/// A worktree discovered via git (includes both main and additional worktrees).
#[derive(Debug, Clone, PartialEq)]
pub struct GitWorktreeEntry {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_main: bool,
}

/// Errors specific to git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("not a git repository: {path}")]
    NotAGitRepo { path: PathBuf },

    #[error("branch already exists: {branch}")]
    BranchAlreadyExists { branch: String },

    #[error("Branch '{branch}' already exists on {remote}. Use a different name.")]
    RemoteBranchAlreadyExists { branch: String, remote: String },

    #[error("base branch not found: {base}")]
    BaseBranchNotFound { base: String },

    #[error("worktree not found: {name}")]
    WorktreeNotFound { name: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

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
/// Opens the repository at `repo_path`, resolves `base` as a local branch
/// first, then falls back to `origin/<base>` remote tracking branch.
/// Creates the new branch from the resolved base commit and adds a
/// worktree at `target_path`.
///
/// Returns `GitError::BranchAlreadyExists` if the branch already exists.
/// Returns `GitError::BaseBranchNotFound` if `base` is not found locally
/// or as `origin/<base>`.
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

    // Best-effort fetch to refresh remote-tracking refs.
    // If fetch fails (offline, no remote, auth), fall back to stale local refs.
    if let Ok(mut origin) = repo.find_remote("origin") {
        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.prune(git2::FetchPrune::On);
        let _ = origin.fetch(&[] as &[&str], Some(&mut fetch_opts), None);
    }

    // Check if branch already exists on remote
    let remote_name = format!("origin/{branch}");
    if repo
        .find_branch(&remote_name, git2::BranchType::Remote)
        .is_ok()
    {
        return Err(GitError::RemoteBranchAlreadyExists {
            branch: branch.to_string(),
            remote: "origin".to_string(),
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
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                return Err(GitError::BaseBranchNotFound {
                    base: base.to_string(),
                });
            }
            Err(e) => return Err(GitError::Git(e)),
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

/// Enumerate all git worktrees for a repository, including the main worktree.
///
/// Opens the repository at `repo_path` and discovers all worktrees: the main
/// working directory plus any additional worktrees created via `git worktree add`.
/// Returns each worktree's name, path, current branch, and whether it is the main worktree.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<GitWorktreeEntry>, GitError> {
    let repo =
        git2::Repository::open(repo_path).map_err(|e| map_repo_open_error(e, repo_path))?;
    let mut entries = Vec::new();

    // Main worktree
    if let Some(workdir) = repo.workdir() {
        let branch = repo
            .head()
            .ok()
            .and_then(|r| r.shorthand().map(String::from));
        let canonical = workdir
            .canonicalize()
            .unwrap_or_else(|_| workdir.to_path_buf());
        let name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "main".to_string());
        entries.push(GitWorktreeEntry {
            name,
            path: canonical,
            branch,
            is_main: true,
        });
    }

    // Additional worktrees
    if let Ok(worktrees) = repo.worktrees() {
        for wt_name in worktrees.iter().flatten() {
            if let Ok(wt) = repo.find_worktree(wt_name) {
                let wt_path = wt.path().to_path_buf();
                let canonical = wt_path
                    .canonicalize()
                    .unwrap_or_else(|_| wt_path.clone());
                // Open as repository to get HEAD branch
                let branch = if let Ok(wt_repo) = git2::Repository::open(&canonical) {
                    wt_repo.head().ok().and_then(|h| h.shorthand().map(String::from))
                } else {
                    None
                };
                entries.push(GitWorktreeEntry {
                    name: wt_name.to_string(),
                    path: canonical,
                    branch,
                    is_main: false,
                });
            }
        }
    }

    Ok(entries)
}

/// Remove a git worktree at the given path.
///
/// Removes the worktree directory from disk, then prunes stale worktree
/// bookkeeping from the repository. The branch itself is preserved.
pub fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), GitError> {
    if !worktree_path.exists() {
        return Err(GitError::WorktreeNotFound {
            name: worktree_path.to_string_lossy().into_owned(),
        });
    }

    // Remove the worktree directory
    std::fs::remove_dir_all(worktree_path)?;

    // Open repo and prune stale worktree references
    let repo =
        git2::Repository::open(repo_path).map_err(|e| map_repo_open_error(e, repo_path))?;

    // Iterate worktrees and prune any that point to missing directories
    if let Ok(worktrees) = repo.worktrees() {
        for name in worktrees.iter().flatten() {
            if let Ok(wt) = repo.find_worktree(name) {
                let _ = wt.prune(Some(
                    git2::WorktreePruneOptions::new()
                        .working_tree(false)
                        .valid(false)
                        .locked(false),
                ));
            }
        }
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
    fn create_worktree_propagates_non_not_found_git_errors() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());

        // Corrupt a remote tracking ref by writing invalid content directly to the
        // filesystem. This causes find_branch to fail with a non-NotFound error
        // (invalid OID parse), which the Err(_) arm must propagate instead of
        // swallowing as BaseBranchNotFound.
        let ref_dir = repo_dir.path().join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&ref_dir).unwrap();
        std::fs::write(ref_dir.join("corrupt"), "not-a-valid-oid\n").unwrap();

        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("feature");

        let result = create_worktree(repo_dir.path(), "feature", "corrupt", &target);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, GitError::Git(_)),
            "non-NotFound git2 errors should propagate as GitError::Git, got: {err:?}"
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
    fn create_worktree_errors_when_branch_exists_on_remote() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();

        // Create a distinct commit for the remote tracking branch
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let remote_oid = repo
            .commit(None, &sig, &sig, "remote commit", &tree, &[&head])
            .unwrap();

        // Manually create a remote tracking ref (origin/taken-branch) without an actual remote
        repo.reference(
            "refs/remotes/origin/taken-branch",
            remote_oid,
            false,
            "fake remote tracking branch for test",
        )
        .unwrap();

        // Verify: "taken-branch" does NOT exist locally, only as remote tracking
        assert!(
            repo.find_branch("taken-branch", git2::BranchType::Local)
                .is_err(),
            "taken-branch should not exist as a local branch"
        );
        assert!(
            repo.find_branch("origin/taken-branch", git2::BranchType::Remote)
                .is_ok(),
            "origin/taken-branch should exist as a remote tracking branch"
        );

        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("taken-branch");

        let result = create_worktree(repo_dir.path(), "taken-branch", &base, &target);

        assert!(result.is_err(), "should fail when branch exists on remote");
        let err = result.unwrap_err();
        assert!(
            matches!(err, GitError::RemoteBranchAlreadyExists { ref branch, ref remote } if branch == "taken-branch" && remote == "origin"),
            "expected RemoteBranchAlreadyExists, got: {err:?}"
        );
        assert!(
            !target.exists(),
            "worktree directory should NOT be created"
        );
    }

    #[test]
    fn create_worktree_succeeds_after_remote_branch_deleted() {
        // Setup: bare "remote" repo with a branch, clone it, delete the branch on remote.
        // The clone retains a stale remote-tracking ref (origin/stale-branch).
        // create_worktree should fetch+prune, clearing the stale ref, and succeed.

        let remote_dir = tempfile::tempdir().unwrap();
        let remote_repo = git2::Repository::init_bare(remote_dir.path()).unwrap();
        {
            // Need an initial commit in the bare repo — build tree + commit directly
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let empty_tree = remote_repo
                .treebuilder(None)
                .unwrap()
                .write()
                .unwrap();
            let tree = remote_repo.find_tree(empty_tree).unwrap();
            let oid = remote_repo
                .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
            // Create a branch that we'll later delete
            let commit = remote_repo.find_commit(oid).unwrap();
            remote_repo
                .branch("stale-branch", &commit, false)
                .unwrap();
        }

        // Clone (local file path)
        let clone_dir = tempfile::tempdir().unwrap();
        let clone = git2::build::RepoBuilder::new()
            .clone(
                remote_dir.path().to_str().unwrap(),
                clone_dir.path(),
            )
            .unwrap();

        // Verify the remote-tracking ref exists
        assert!(
            clone
                .find_branch("origin/stale-branch", git2::BranchType::Remote)
                .is_ok(),
            "stale-branch should exist as remote tracking before deletion"
        );

        // Delete the branch on the bare remote
        remote_repo
            .find_branch("stale-branch", git2::BranchType::Local)
            .unwrap()
            .delete()
            .unwrap();

        // The stale ref still exists locally (no fetch yet)
        assert!(
            clone
                .find_branch("origin/stale-branch", git2::BranchType::Remote)
                .is_ok(),
            "stale ref should still exist before fetch+prune"
        );

        let base = head_branch(&clone);
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("stale-branch");

        // This should succeed: fetch+prune clears the stale ref
        let result = create_worktree(clone_dir.path(), "stale-branch", &base, &target);

        assert!(
            result.is_ok(),
            "should succeed after remote branch deleted, got: {result:?}"
        );
        assert!(target.exists(), "worktree directory should exist on disk");
    }

    #[test]
    fn remove_worktree_deletes_directory_and_prunes() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("to-remove");

        create_worktree(repo_dir.path(), "to-remove", &base, &target)
            .expect("should create worktree");
        assert!(target.exists(), "worktree should exist before removal");

        remove_worktree(repo_dir.path(), &target).expect("should remove worktree");

        assert!(!target.exists(), "worktree directory should be deleted");

        // The branch should still exist (we only remove the worktree, not the branch)
        assert!(
            repo.find_branch("to-remove", git2::BranchType::Local).is_ok(),
            "branch should still exist after worktree removal"
        );
    }

    #[test]
    fn remove_worktree_errors_for_nonexistent_path() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let fake_path = repo_dir.path().join("nonexistent-worktree");

        let result = remove_worktree(repo_dir.path(), &fake_path);
        assert!(result.is_err(), "should error for nonexistent worktree path");
    }

    #[test]
    fn ahead_behind_counts_commits_ahead_of_base() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());
        let base = head_branch(&repo);
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();

        // Create feature branch at same point as base
        let base_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature-ahead", &base_commit, false).unwrap();

        // Add 2 commits on feature-ahead
        // Switch HEAD to feature-ahead to commit on it
        repo.set_head("refs/heads/feature-ahead").unwrap();
        for i in 0..2 {
            let parent = repo.head().unwrap().peel_to_commit().unwrap();
            let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("feature commit {i}"),
                &tree,
                &[&parent],
            )
            .unwrap();
        }

        let result = ahead_behind(tmp.path(), "feature-ahead", Some(&base))
            .expect("should succeed");

        assert_eq!(result, Some((2, 0)), "feature should be 2 ahead, 0 behind");
    }

    #[test]
    fn ahead_behind_counts_commits_behind_base() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());
        let base = head_branch(&repo);
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();

        // Create feature branch at current commit
        let base_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature-behind", &base_commit, false).unwrap();

        // Add 3 commits on the base branch (feature stays at original commit)
        for i in 0..3 {
            let parent = repo.head().unwrap().peel_to_commit().unwrap();
            let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("base commit {i}"),
                &tree,
                &[&parent],
            )
            .unwrap();
        }

        let result = ahead_behind(tmp.path(), "feature-behind", Some(&base))
            .expect("should succeed");

        assert_eq!(result, Some((0, 3)), "feature should be 0 ahead, 3 behind");
    }

    #[test]
    fn ahead_behind_returns_none_when_no_upstream_and_no_base() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());

        // Create a branch with no upstream and pass no base_branch
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("orphan-branch", &head_commit, false).unwrap();

        let result = ahead_behind(tmp.path(), "orphan-branch", None)
            .expect("should succeed");

        assert_eq!(result, None, "no upstream and no base should return None");
    }

    #[test]
    fn dirty_count_returns_zero_for_clean_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(tmp.path());

        let count = dirty_count(tmp.path()).expect("should succeed");
        assert_eq!(count, 0, "clean worktree should have 0 dirty files");
    }

    #[test]
    fn ahead_behind_returns_zero_zero_when_at_same_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());
        let base = head_branch(&repo);

        // Create a feature branch at the same commit as base
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head_commit, false).unwrap();

        let result = ahead_behind(tmp.path(), "feature", Some(&base))
            .expect("should succeed");

        assert_eq!(result, Some((0, 0)), "same commit should be (0, 0)");
    }

    #[test]
    fn dirty_count_counts_modified_and_untracked_files() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());

        // Create an untracked file
        std::fs::write(tmp.path().join("untracked.txt"), "new").unwrap();

        // Modify a tracked file: add a file to the index, then change it on disk
        let tracked_path = tmp.path().join("tracked.txt");
        std::fs::write(&tracked_path, "original").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(std::path::Path::new("tracked.txt")).unwrap();
            index.write().unwrap();
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "add tracked", &tree, &[&head])
                .unwrap();
        }
        // Now modify the tracked file
        std::fs::write(&tracked_path, "modified").unwrap();

        let count = dirty_count(tmp.path()).expect("should succeed");
        assert_eq!(count, 2, "should count 1 modified + 1 untracked = 2");
    }

    #[test]
    fn list_worktrees_includes_main_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(tmp.path());
        let base = head_branch(&repo);

        let worktrees = list_worktrees(tmp.path()).expect("should list worktrees");

        assert!(!worktrees.is_empty(), "should include at least the main worktree");
        let main_wt = worktrees.iter().find(|w| w.is_main).expect("should have main worktree");
        assert_eq!(main_wt.path, tmp.path().canonicalize().unwrap());
        assert_eq!(main_wt.branch.as_deref(), Some(base.as_str()));
    }

    #[test]
    fn list_worktrees_includes_additional_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = head_branch(&repo);
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("extra-wt");

        create_worktree(repo_dir.path(), "extra-wt", &base, &target)
            .expect("should create worktree");

        let worktrees = list_worktrees(repo_dir.path()).expect("should list worktrees");

        assert_eq!(worktrees.len(), 2, "should include main + additional worktree");

        let additional = worktrees.iter().find(|w| !w.is_main).expect("should have additional worktree");
        assert_eq!(additional.path, target.canonicalize().unwrap());
        assert_eq!(additional.branch.as_deref(), Some("extra-wt"));
        assert!(!additional.is_main);
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
