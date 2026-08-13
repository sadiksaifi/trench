use std::path::Path;
use std::process::Command;

use super::GitError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateReceipt {
    /// Present only when this successful call itself created the branch.
    pub created_branch: Option<String>,
}

/// Add a worktree for a newly-created local branch at the exact planned path.
///
/// This operation is deliberately local-only: ref refresh belongs to planning,
/// and execution must consume the same immutable snapshot without fetching.
pub fn add_new_branch(
    repo_path: &Path,
    worktree: &str,
    branch: &str,
    base: &str,
    base_oid: git2::Oid,
    target_path: &Path,
) -> Result<CreateReceipt, GitError> {
    let repo = git2::Repository::open(repo_path)?;
    if repo.find_branch(branch, git2::BranchType::Local).is_ok() {
        return Err(GitError::BranchAlreadyExists {
            branch: branch.to_string(),
        });
    }

    if resolve_named_commit(&repo, base)? != base_oid {
        return Err(GitError::PreconditionsChanged);
    }
    let base_commit = repo.find_commit(base_oid)?;
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

    Ok(CreateReceipt {
        created_branch: Some(branch.to_string()),
    })
}

/// Add a worktree for an existing local branch without creating or rewriting it.
pub fn add_existing_local(
    repo_path: &Path,
    worktree: &str,
    branch: &str,
    expected_oid: git2::Oid,
    target_path: &Path,
) -> Result<CreateReceipt, GitError> {
    let repo = git2::Repository::open(repo_path)?;
    let local = repo
        .find_branch(branch, git2::BranchType::Local)
        .map_err(|error| {
            if error.code() == git2::ErrorCode::NotFound {
                GitError::LocalBranchNotFound {
                    branch: branch.to_string(),
                }
            } else {
                error.into()
            }
        })?;
    if local.get().peel_to_commit()?.id() != expected_oid {
        return Err(GitError::PreconditionsChanged);
    }
    let mut options = git2::WorktreeAddOptions::new();
    options.reference(Some(local.get()));
    repo.worktree(worktree, target_path, Some(&options))?;
    Ok(CreateReceipt {
        created_branch: None,
    })
}

/// Create a local branch from a remote-only ref, establish its upstream, and
/// add a worktree at the exact planned path.
pub fn add_tracking_branch(
    repo_path: &Path,
    worktree: &str,
    branch: &str,
    upstream: &str,
    upstream_oid: git2::Oid,
    target_path: &Path,
) -> Result<CreateReceipt, GitError> {
    let repo = git2::Repository::open(repo_path)?;
    if repo.find_branch(branch, git2::BranchType::Local).is_ok() {
        return Err(GitError::BranchAlreadyExists {
            branch: branch.to_string(),
        });
    }
    let remote = repo
        .find_branch(upstream, git2::BranchType::Remote)
        .map_err(|error| {
            if error.code() == git2::ErrorCode::NotFound {
                GitError::BaseBranchNotFound {
                    base: upstream.to_string(),
                }
            } else {
                error.into()
            }
        })?;
    let commit = remote.get().peel_to_commit()?;
    if commit.id() != upstream_oid {
        return Err(GitError::PreconditionsChanged);
    }
    let worktree_result = {
        let mut local = repo.branch(branch, &commit, false)?;
        if let Err(error) = local.set_upstream(Some(upstream)) {
            let _ = local.delete();
            return Err(error.into());
        }
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(local.get()));
        repo.worktree(worktree, target_path, Some(&options))
    };

    if let Err(error) = worktree_result {
        if let Ok(mut orphan) = repo.find_branch(branch, git2::BranchType::Local) {
            let _ = orphan.delete();
        }
        return Err(error.into());
    }
    Ok(CreateReceipt {
        created_branch: Some(branch.to_string()),
    })
}

/// Remove a just-created worktree through Git and optionally delete the branch
/// created for it. Existing-local branches must pass `None` and are preserved.
pub fn rollback_created_worktree(
    repo_path: &Path,
    target_path: &Path,
    created_branch: Option<&str>,
) -> Result<(), GitError> {
    let remove = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["worktree", "remove", "--force", "--force"])
        .arg(target_path)
        .output()?;
    if !remove.status.success() && target_path.exists() {
        return Err(GitError::CommandFailed {
            operation: "rolling back created worktree",
            message: String::from_utf8_lossy(&remove.stderr).trim().to_string(),
        });
    }

    let prune = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["worktree", "prune"])
        .output()?;
    if !prune.status.success() {
        return Err(GitError::CommandFailed {
            operation: "pruning rolled back worktree",
            message: String::from_utf8_lossy(&prune.stderr).trim().to_string(),
        });
    }

    if let Some(branch) = created_branch {
        let repo = git2::Repository::open(repo_path)?;
        match repo.find_branch(branch, git2::BranchType::Local) {
            Ok(mut local) => local.delete()?,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        };
    }
    Ok(())
}

fn resolve_named_commit(repo: &git2::Repository, name: &str) -> Result<git2::Oid, GitError> {
    let reference = if name.starts_with("origin/") {
        format!("refs/remotes/{name}")
    } else {
        format!("refs/heads/{name}")
    };
    Ok(repo.revparse_single(&reference)?.peel_to_commit()?.id())
}
