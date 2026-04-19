use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git::{self, GitWorktreeEntry, RepoInfo};
use crate::paths;
use crate::state::{Database, Repo, Worktree};

#[derive(Debug, Clone)]
pub struct LiveWorktree {
    pub entry: GitWorktreeEntry,
    pub metadata: Option<Worktree>,
}

fn repo_path_str(repo_info: &RepoInfo) -> Result<&str> {
    repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))
}

fn canonical_string(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn ensure_repo(db: &Database, repo_info: &RepoInfo) -> Result<Repo> {
    let repo_path = repo_path_str(repo_info)?;
    if let Some(repo) = db.get_repo_by_path(repo_path)? {
        return Ok(repo);
    }

    db.insert_repo(&repo_info.name, repo_path, Some(&repo_info.default_branch))
}

fn purge_stale_metadata(
    db: &Database,
    repo_id: i64,
    live_entries: &[GitWorktreeEntry],
) -> Result<()> {
    let live_paths: HashSet<String> = live_entries
        .iter()
        .map(|entry| canonical_string(&entry.path))
        .collect();

    for worktree in db.list_worktrees(repo_id)? {
        let stored_path = if Path::new(&worktree.path).exists() {
            canonical_string(Path::new(&worktree.path))
        } else {
            worktree.path.clone()
        };
        if !live_paths.contains(&stored_path) {
            db.delete_worktree_metadata(worktree.id)?;
        }
    }

    Ok(())
}

pub fn list(
    repo_info: &RepoInfo,
    db: &Database,
    scan_paths: &[String],
) -> Result<Vec<LiveWorktree>> {
    let mut entries = git::list_worktrees(&repo_info.path)?;
    let mut seen_paths: HashSet<PathBuf> = entries.iter().map(|entry| entry.path.clone()).collect();

    for scanned in git::scan_directories(scan_paths) {
        if seen_paths.insert(scanned.path.clone()) {
            entries.push(scanned);
        }
    }

    let repo = db.get_repo_by_path(repo_path_str(repo_info)?)?;
    if let Some(ref repo) = repo {
        purge_stale_metadata(db, repo.id, &entries)?;
    }

    let repo = db.get_repo_by_path(repo_path_str(repo_info)?)?;
    let mut live = Vec::with_capacity(entries.len());
    for entry in entries {
        let metadata = if let Some(ref repo) = repo {
            db.find_worktree_by_path(repo.id, &canonical_string(&entry.path))?
        } else {
            None
        };
        live.push(LiveWorktree { entry, metadata });
    }

    Ok(live)
}

pub fn resolve(identifier: &str, repo_info: &RepoInfo, db: &Database) -> Result<LiveWorktree> {
    let sanitized = paths::sanitize_branch(identifier);
    for worktree in list(repo_info, db, &[])? {
        let branch_match = worktree.entry.branch.as_deref() == Some(identifier);
        let name_match = worktree.entry.name == identifier || worktree.entry.name == sanitized;
        let sanitized_branch_match = worktree
            .entry
            .branch
            .as_deref()
            .is_some_and(|branch| paths::sanitize_branch(branch) == sanitized);

        if branch_match || name_match || sanitized_branch_match {
            return Ok(worktree);
        }
    }

    anyhow::bail!("worktree not found: {identifier}")
}

pub fn ensure_metadata(
    db: &Database,
    repo_info: &RepoInfo,
    worktree: &GitWorktreeEntry,
) -> Result<(Repo, Worktree)> {
    let repo = ensure_repo(db, repo_info)?;
    let path = canonical_string(&worktree.path);

    if let Some(metadata) = db.find_worktree_by_path(repo.id, &path)? {
        return Ok((repo, metadata));
    }

    let branch = worktree
        .branch
        .clone()
        .unwrap_or_else(|| worktree.name.clone());
    let name = paths::sanitize_branch(&branch);
    let metadata = db.adopt_worktree(repo.id, &name, &branch, &path, None)?;
    Ok((repo, metadata))
}

pub fn base_branch(repo_info: &RepoInfo, worktree: &LiveWorktree) -> String {
    if let Some(branch) = worktree.entry.branch.as_deref() {
        if let Ok(Some(upstream)) = git::upstream_branch_name(&worktree.entry.path, branch) {
            return upstream;
        }
    }

    worktree
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.base_branch.clone())
        .unwrap_or_else(|| repo_info.default_branch.clone())
}
