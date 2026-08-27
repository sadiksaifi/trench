#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::sync::Arc;

use super::GitError;

pub struct CreateTarget<'a> {
    pub planned_path: &'a Path,
    #[cfg(unix)]
    pub parent_directory: Arc<std::fs::File>,
}

#[derive(Debug, Clone)]
pub struct CreateReceipt {
    /// The canonical path Git recorded for this successful worktree add.
    pub recorded_path: Option<PathBuf>,
    /// Present only when this successful call itself created the branch.
    pub created_branch: Option<String>,
    #[cfg(unix)]
    parent_directory: Arc<std::fs::File>,
    #[cfg(unix)]
    leaf: OsString,
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
    target: CreateTarget<'_>,
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
        repo.branch(branch, &base_commit, false)?;
        add_worktree_descriptor_bound(&repo, worktree, branch, target)
    };

    let mut receipt = match worktree_result {
        Ok(receipt) => receipt,
        Err(error) => {
            if let Ok(mut orphan) = repo.find_branch(branch, git2::BranchType::Local) {
                let _ = orphan.delete();
            }
            return Err(error);
        }
    };
    receipt.created_branch = Some(branch.to_string());
    Ok(receipt)
}

/// Add a worktree for an existing local branch without creating or rewriting it.
pub fn add_existing_local(
    repo_path: &Path,
    worktree: &str,
    branch: &str,
    expected_oid: git2::Oid,
    target: CreateTarget<'_>,
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
    add_worktree_descriptor_bound(&repo, worktree, branch, target)
}

/// Create a local branch from a remote-only ref, establish its upstream, and
/// add a worktree at the exact planned path.
pub fn add_tracking_branch(
    repo_path: &Path,
    worktree: &str,
    branch: &str,
    upstream: &str,
    upstream_oid: git2::Oid,
    target: CreateTarget<'_>,
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
        add_worktree_descriptor_bound(&repo, worktree, branch, target)
    };

    let mut receipt = match worktree_result {
        Ok(receipt) => receipt,
        Err(error) => {
            if let Ok(mut orphan) = repo.find_branch(branch, git2::BranchType::Local) {
                let _ = orphan.delete();
            }
            return Err(error);
        }
    };
    receipt.created_branch = Some(branch.to_string());
    Ok(receipt)
}

#[cfg(unix)]
fn add_worktree_descriptor_bound(
    repo: &git2::Repository,
    worktree: &str,
    branch: &str,
    target: CreateTarget<'_>,
) -> Result<CreateReceipt, GitError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let leaf = target
        .planned_path
        .file_name()
        .ok_or(GitError::PreconditionsChanged)?
        .to_os_string();
    let parent_fd = target.parent_directory.as_raw_fd();
    let mut command = Command::new("git");
    command
        .arg(format!("--git-dir={}", repo.path().display()))
        .args(["-c", "core.hooksPath=/dev/null"])
        .args([
            "worktree",
            "add",
            "--no-guess-remote",
            "--no-relative-paths",
        ])
        .arg(&leaf)
        .arg(branch)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR");
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(parent_fd) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            operation: "creating descriptor-bound worktree",
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(CreateReceipt {
        recorded_path: repo
            .find_worktree(worktree)
            .ok()
            .map(|created| created.path().to_path_buf()),
        created_branch: None,
        parent_directory: target.parent_directory,
        leaf,
    })
}

#[cfg(not(unix))]
fn add_worktree_descriptor_bound(
    _repo: &git2::Repository,
    _worktree: &str,
    _branch: &str,
    _target: CreateTarget<'_>,
) -> Result<CreateReceipt, GitError> {
    Err(GitError::PreconditionsChanged)
}

/// Remove a just-created worktree through Git and optionally delete the branch
/// created for it. Existing-local branches retain `created_branch: None`.
pub fn rollback_created_worktree(
    repo_path: &Path,
    receipt: &CreateReceipt,
) -> Result<(), GitError> {
    let remove = remove_worktree_descriptor_bound(repo_path, receipt)?;
    if !remove.status.success() && descriptor_target_exists(receipt)? {
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

    if let Some(branch) = receipt.created_branch.as_deref() {
        let repo = git2::Repository::open(repo_path)?;
        match repo.find_branch(branch, git2::BranchType::Local) {
            Ok(mut local) => local.delete()?,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        };
    }
    Ok(())
}

#[cfg(unix)]
fn remove_worktree_descriptor_bound(
    repo_path: &Path,
    receipt: &CreateReceipt,
) -> Result<std::process::Output, GitError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let parent_fd = receipt.parent_directory.as_raw_fd();
    let repo = git2::Repository::open(repo_path)?;
    let mut command = Command::new("git");
    command
        .arg(format!("--git-dir={}", repo.path().display()))
        .args(["worktree", "remove", "--force", "--force"])
        .arg(&receipt.leaf)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR");
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(parent_fd) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    command.output().map_err(Into::into)
}

#[cfg(not(unix))]
fn remove_worktree_descriptor_bound(
    _repo_path: &Path,
    _receipt: &CreateReceipt,
) -> Result<std::process::Output, GitError> {
    Err(GitError::PreconditionsChanged)
}

#[cfg(unix)]
fn descriptor_target_exists(receipt: &CreateReceipt) -> Result<bool, GitError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let leaf = std::ffi::CString::new(receipt.leaf.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "worktree leaf contains a NUL byte",
        )
    })?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            receipt.parent_directory.as_raw_fd(),
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        Ok(true)
    } else {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error.into())
        }
    }
}

#[cfg(not(unix))]
fn descriptor_target_exists(_receipt: &CreateReceipt) -> Result<bool, GitError> {
    Err(GitError::PreconditionsChanged)
}

fn resolve_named_commit(repo: &git2::Repository, name: &str) -> Result<git2::Oid, GitError> {
    let reference = if name.starts_with("origin/") {
        format!("refs/remotes/{name}")
    } else {
        format!("refs/heads/{name}")
    };
    Ok(repo.revparse_single(&reference)?.peel_to_commit()?.id())
}
