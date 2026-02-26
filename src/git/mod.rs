use std::path::{Path, PathBuf};

/// Information about a discovered git repository.
#[derive(Debug, Clone)]
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

    #[error("{0}")]
    Git(#[from] git2::Error),
}

/// Discover a git repository by walking up from the given path.
///
/// Returns a `RepoInfo` with the repo name (derived from the working directory),
/// the canonical repo path, optional origin remote URL, and the default branch.
pub fn discover_repo(path: &Path) -> Result<RepoInfo, GitError> {
    let repo = git2::Repository::discover(path).map_err(|_| GitError::NotAGitRepo {
        path: path.to_path_buf(),
    })?;

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
        .unwrap_or_default();

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
    let repo = git2::Repository::open(repo_path).map_err(|_| GitError::NotAGitRepo {
        path: repo_path.to_path_buf(),
    })?;

    // Check if branch already exists locally
    if repo
        .find_branch(branch, git2::BranchType::Local)
        .is_ok()
    {
        return Err(GitError::BranchAlreadyExists {
            branch: branch.to_string(),
        });
    }

    // Resolve base branch to a commit
    let base_ref = repo.find_branch(base, git2::BranchType::Local)?;
    let base_commit = base_ref.get().peel_to_commit()?;

    // Create the new branch from base
    repo.branch(branch, &base_commit, false)?;

    // Create the worktree
    let branch_ref = repo.find_branch(branch, git2::BranchType::Local)?;
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(branch_ref.get()));
    repo.worktree(branch, target_path, Some(&opts))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a temp git repo with an initial commit.
    fn init_repo_with_commit(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).expect("failed to init repo");
        // Create an initial commit so HEAD is valid
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

    /// Helper: get the default branch name from HEAD.
    fn head_branch(repo: &git2::Repository) -> String {
        repo.head()
            .unwrap()
            .shorthand()
            .unwrap()
            .to_string()
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
}
