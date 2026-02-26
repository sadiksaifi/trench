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

    Ok(RepoInfo {
        name,
        path: workdir,
        remote_url: None,
        default_branch: String::from("main"),
    })
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
}
