use std::path::Path;

use super::GitError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusCounts {
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
}

pub fn counts(worktree_path: &Path) -> Result<StatusCounts, GitError> {
    let repo = git2::Repository::open(worktree_path)
        .map_err(|error| super::map_repo_open_error(error, worktree_path))?;
    let statuses = repo.statuses(Some(
        git2::StatusOptions::new()
            .include_untracked(true)
            .recurse_untracked_dirs(true),
    ))?;
    let mut counts = StatusCounts::default();
    for entry in statuses.iter() {
        let status = entry.status();
        if status.is_wt_new() && !status.is_index_new() {
            counts.untracked += 1;
            continue;
        }
        if status.intersects(
            git2::Status::INDEX_NEW
                | git2::Status::INDEX_MODIFIED
                | git2::Status::INDEX_DELETED
                | git2::Status::INDEX_RENAMED
                | git2::Status::INDEX_TYPECHANGE,
        ) {
            counts.staged += 1;
        }
        if status.intersects(
            git2::Status::WT_MODIFIED
                | git2::Status::WT_DELETED
                | git2::Status::WT_RENAMED
                | git2::Status::WT_TYPECHANGE,
        ) {
            counts.modified += 1;
        }
    }
    Ok(counts)
}

pub fn ahead_behind(
    worktree_path: &Path,
    head: Option<&str>,
    base: Option<&str>,
) -> Result<Option<(usize, usize)>, GitError> {
    let (Some(head), Some(base)) = (head, base) else {
        return Ok(None);
    };
    let repo = git2::Repository::open(worktree_path)
        .map_err(|error| super::map_repo_open_error(error, worktree_path))?;
    let head = git2::Oid::from_str(head)?;
    let base = resolve_base(&repo, base)?;
    Ok(base
        .map(|base| repo.graph_ahead_behind(head, base))
        .transpose()?)
}

pub fn detected_base(repo_path: &Path) -> Result<Option<String>, GitError> {
    let repo = git2::Repository::open(repo_path)
        .map_err(|error| super::map_repo_open_error(error, repo_path))?;
    let base = match repo.find_reference("refs/remotes/origin/HEAD") {
        Ok(reference) => Ok(reference
            .symbolic_target()
            .and_then(|target| target.strip_prefix("refs/remotes/"))
            .map(ToOwned::to_owned)),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    };
    base
}

fn resolve_base(repo: &git2::Repository, base: &str) -> Result<Option<git2::Oid>, git2::Error> {
    let candidates = if base.starts_with("refs/") || base.starts_with("origin/") {
        vec![base.to_string()]
    } else {
        vec![
            format!("refs/heads/{base}"),
            format!("refs/remotes/origin/{base}"),
        ]
    };
    for candidate in candidates {
        match repo.revparse_single(&candidate) {
            Ok(object) => return Ok(Some(object.id())),
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}
