use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::state::Database;

/// Sync strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Rebase,
    Merge,
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Strategy::Rebase => write!(f, "rebase"),
            Strategy::Merge => write!(f, "merge"),
        }
    }
}

/// Result of a sync operation.
#[derive(Debug)]
pub struct SyncResult {
    /// Name of the worktree that was synced.
    pub name: String,
    /// Strategy used.
    pub strategy: Strategy,
    /// Ahead count before sync.
    pub before_ahead: usize,
    /// Behind count before sync.
    pub before_behind: usize,
    /// Ahead count after sync.
    pub after_ahead: usize,
    /// Behind count after sync.
    pub after_behind: usize,
}

/// JSON representation of a sync result.
#[derive(Debug, Serialize)]
pub struct SyncResultJson {
    pub name: String,
    pub strategy: String,
    pub before: AheadBehind,
    pub after: AheadBehind,
}

#[derive(Debug, Serialize)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

impl SyncResult {
    pub fn to_json(&self) -> SyncResultJson {
        SyncResultJson {
            name: self.name.clone(),
            strategy: self.strategy.to_string(),
            before: AheadBehind {
                ahead: self.before_ahead,
                behind: self.before_behind,
            },
            after: AheadBehind {
                ahead: self.after_ahead,
                behind: self.after_behind,
            },
        }
    }
}

/// Execute the `trench sync <identifier>` command.
///
/// Resolves the worktree (adopting it if unmanaged), fetches from remote,
/// then rebases or merges with the base branch.
pub fn execute(
    identifier: &str,
    cwd: &Path,
    db: &Database,
    strategy: Strategy,
) -> Result<SyncResult> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let (repo, wt) = crate::adopt::resolve_or_adopt(identifier, &repo_info, db)?;

    let base_branch = wt
        .base_branch
        .as_deref()
        .or(repo.default_base.as_deref())
        .unwrap_or("main");

    // Get before counts
    let (before_ahead, before_behind) =
        crate::git::ahead_behind(Path::new(&repo_info.path), &wt.branch, Some(base_branch))?
            .unwrap_or((0, 0));

    // Fetch from remote
    crate::git::fetch_remote(Path::new(&repo_info.path))?;

    // Perform sync
    match strategy {
        Strategy::Rebase => {
            crate::git::sync_rebase(Path::new(&wt.path), &wt.branch, base_branch)?;
        }
        Strategy::Merge => {
            crate::git::sync_merge(Path::new(&wt.path), &wt.branch, base_branch)?;
        }
    }

    // Get after counts
    let (after_ahead, after_behind) =
        crate::git::ahead_behind(Path::new(&repo_info.path), &wt.branch, Some(base_branch))?
            .unwrap_or((0, 0));

    // Insert synced event
    let payload = serde_json::json!({
        "strategy": strategy.to_string(),
        "base_branch": base_branch,
        "before": { "ahead": before_ahead, "behind": before_behind },
        "after": { "ahead": after_ahead, "behind": after_behind },
    });
    db.insert_event(repo.id, Some(wt.id), "synced", Some(&payload))?;

    Ok(SyncResult {
        name: wt.name.clone(),
        strategy,
        before_ahead,
        before_behind,
        after_ahead,
        after_behind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;

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

    /// Helper: create a file, stage, and commit it in the given repo.
    fn commit_file(repo: &git2::Repository, filename: &str, content: &str, message: &str) {
        let workdir = repo.workdir().unwrap();
        std::fs::write(workdir.join(filename), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(filename)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
            .unwrap();
    }

    /// Set up a test scenario with a main repo, a worktree branch behind main.
    /// Returns (main_repo, worktree_path, db, repo_path_str).
    fn setup_diverged_repo() -> (
        git2::Repository,
        std::path::PathBuf,
        Database,
        tempfile::TempDir,
        tempfile::TempDir,
        String,
    ) {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap().to_string();

        // Rename HEAD branch to "main" for consistency
        git_repo
            .find_branch(
                git_repo
                    .head()
                    .unwrap()
                    .shorthand()
                    .unwrap(),
                git2::BranchType::Local,
            )
            .unwrap()
            .rename("main", true)
            .unwrap();

        // Create feature branch at current main
        {
            let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
            git_repo
                .branch("feature", &head_commit, false)
                .unwrap();
        }

        // Create worktree for the feature branch
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feature");
        {
            let branch_ref = git_repo
                .find_branch("feature", git2::BranchType::Local)
                .unwrap();
            let mut opts = git2::WorktreeAddOptions::new();
            opts.reference(Some(branch_ref.get()));
            git_repo
                .worktree("feature", &wt_path, Some(&opts))
                .unwrap();
        }

        // Add a commit on the feature branch (in worktree)
        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        commit_file(&wt_repo, "feature.txt", "feature work", "feature commit");

        // Add a commit on main (in main repo) to create divergence
        // First, switch main repo back to main
        {
            let main_obj = git_repo.revparse_single("refs/heads/main").unwrap();
            git_repo.checkout_tree(&main_obj, None).unwrap();
            git_repo.set_head("refs/heads/main").unwrap();
        }
        commit_file(&git_repo, "upstream.txt", "upstream change", "upstream commit on main");

        // Register in DB
        db.insert_repo("test-repo", &repo_path_str, Some("main"))
            .unwrap();
        let db_repo = db.get_repo_by_path(&repo_path_str).unwrap().unwrap();
        let wt_path_str = wt_path.canonicalize().unwrap_or(wt_path.clone());
        db.insert_worktree(
            db_repo.id,
            "feature",
            "feature",
            wt_path_str.to_str().unwrap(),
            Some("main"),
        )
        .unwrap();

        (git_repo, wt_path, db, repo_dir, wt_dir, repo_path_str)
    }

    #[test]
    fn sync_rebase_rebases_branch_onto_main() {
        let (_git_repo, wt_path, db, repo_dir, _wt_dir, _repo_path_str) = setup_diverged_repo();

        // Before sync: feature should be 1 behind main
        let result = execute("feature", repo_dir.path(), &db, Strategy::Rebase)
            .expect("rebase sync should succeed");

        assert_eq!(result.name, "feature");
        assert_eq!(result.strategy, Strategy::Rebase);
        assert_eq!(result.before_behind, 1, "should be 1 behind before sync");

        // After rebase, behind should be 0
        assert_eq!(result.after_behind, 0, "should be 0 behind after rebase");

        // Feature branch should still have its commit + upstream file should exist
        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        let head = wt_repo.head().unwrap().peel_to_commit().unwrap();
        assert!(
            head.message().unwrap().contains("feature commit"),
            "feature commit should be on top after rebase"
        );
        assert!(
            wt_path.join("upstream.txt").exists(),
            "upstream file should exist after rebase"
        );
        assert!(
            wt_path.join("feature.txt").exists(),
            "feature file should still exist after rebase"
        );
    }

    #[test]
    fn sync_adopts_unmanaged_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Rename HEAD branch to "main"
        {
            let head_branch_name = git_repo
                .head()
                .unwrap()
                .shorthand()
                .unwrap()
                .to_string();
            git_repo
                .find_branch(&head_branch_name, git2::BranchType::Local)
                .unwrap()
                .rename("main", true)
                .unwrap();
        }

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("sync-feat");
        {
            let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
            git_repo
                .branch("sync-feat", &head_commit, false)
                .unwrap();
        }
        {
            let branch_ref = git_repo
                .find_branch("sync-feat", git2::BranchType::Local)
                .unwrap();
            let mut opts = git2::WorktreeAddOptions::new();
            opts.reference(Some(branch_ref.get()));
            git_repo
                .worktree("sync-feat", &wt_path, Some(&opts))
                .unwrap();
        }

        // Sync the unmanaged worktree — should trigger adoption
        let result = execute("sync-feat", repo_dir.path(), &db, Strategy::Rebase)
            .expect("sync should succeed");
        assert_eq!(result.name, "sync-feat");

        // Verify worktree was adopted in DB
        let db_repo = db.get_repo_by_path(repo_path_str).unwrap().unwrap();
        let wt = db
            .find_worktree_by_identifier(db_repo.id, "sync-feat")
            .unwrap()
            .expect("adopted worktree should be in DB");
        assert!(wt.adopted_at.is_some(), "adopted_at should be set");
        assert!(wt.managed, "should be managed after adoption");
    }
}
