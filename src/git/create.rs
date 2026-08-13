use std::path::Path;

use super::GitError;

/// Add a worktree for a newly-created local branch at the exact planned path.
///
/// This operation is deliberately local-only: ref refresh belongs to planning,
/// and execution must consume the same immutable snapshot without fetching.
pub fn add_new_branch(
    repo_path: &Path,
    worktree: &str,
    branch: &str,
    base: &str,
    target_path: &Path,
) -> Result<(), GitError> {
    let repo = git2::Repository::open(repo_path)?;
    if repo.find_branch(branch, git2::BranchType::Local).is_ok() {
        return Err(GitError::BranchAlreadyExists {
            branch: branch.to_string(),
        });
    }

    let base_commit = resolve_commit(&repo, base)?;
    let worktree_result = {
        let new_branch = repo.branch(branch, &base_commit, false)?;
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(new_branch.get()));
        repo.worktree(worktree, target_path, Some(&options))
    };

    if let Err(error) = worktree_result {
        if let Ok(mut orphan) = repo.find_branch(branch, git2::BranchType::Local) {
            let _ = orphan.delete();
        }
        return Err(error.into());
    }

    Ok(())
}

fn resolve_commit<'repo>(
    repo: &'repo git2::Repository,
    name: &str,
) -> Result<git2::Commit<'repo>, GitError> {
    if let Ok(local) = repo.find_branch(name, git2::BranchType::Local) {
        return local.get().peel_to_commit().map_err(Into::into);
    }
    if let Ok(remote) = repo.find_branch(name, git2::BranchType::Remote) {
        return remote.get().peel_to_commit().map_err(Into::into);
    }
    let remote_name = format!("origin/{name}");
    repo.find_branch(&remote_name, git2::BranchType::Remote)
        .map_err(|error| {
            if error.code() == git2::ErrorCode::NotFound {
                GitError::BaseBranchNotFound {
                    base: name.to_string(),
                }
            } else {
                error.into()
            }
        })?
        .get()
        .peel_to_commit()
        .map_err(Into::into)
}
