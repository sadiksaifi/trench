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

    let dirty = crate::git::dirty_count(Path::new(&wt.path))?;
    if dirty > 0 {
        anyhow::bail!(
            "worktree '{}' has {} uncommitted change(s); commit or stash before syncing",
            wt.name,
            dirty
        );
    }

    let base_branch = wt
        .base_branch
        .as_deref()
        .or(repo.default_base.as_deref())
        .unwrap_or(repo_info.default_branch.as_str());

    // Fetch from remote before capturing the baseline counts
    if let Err(e) = crate::git::fetch_remote(Path::new(&repo_info.path)) {
        eprintln!("warning: fetch failed, using local refs: {e}");
    }

    let (before_ahead, before_behind) =
        crate::git::ahead_behind(Path::new(&repo_info.path), &wt.branch, Some(base_branch))?
            .unwrap_or((0, 0));

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
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test").unwrap();
            config.set_str("user.email", "test@test.com").unwrap();
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

    struct DivergentRepoFixture {
        _git_repo: git2::Repository,
        wt_path: std::path::PathBuf,
        db: Database,
        _repo_dir: tempfile::TempDir,
        _wt_dir: tempfile::TempDir,
        repo_path_str: String,
    }

    /// Set up a test scenario with a main repo, a worktree branch behind main.
    fn setup_diverged_repo() -> DivergentRepoFixture {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap().to_string();

        // Rename HEAD branch to "main" for consistency
        git_repo
            .find_branch(
                git_repo.head().unwrap().shorthand().unwrap(),
                git2::BranchType::Local,
            )
            .unwrap()
            .rename("main", true)
            .unwrap();

        // Create feature branch at current main
        {
            let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
            git_repo.branch("feature", &head_commit, false).unwrap();
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
            git_repo.worktree("feature", &wt_path, Some(&opts)).unwrap();
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
        commit_file(
            &git_repo,
            "upstream.txt",
            "upstream change",
            "upstream commit on main",
        );

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

        DivergentRepoFixture {
            _git_repo: git_repo,
            wt_path,
            db,
            _repo_dir: repo_dir,
            _wt_dir: wt_dir,
            repo_path_str,
        }
    }

    #[test]
    fn sync_rebase_rebases_branch_onto_main() {
        let f = setup_diverged_repo();

        // Before sync: feature should be 1 behind main
        let result = execute("feature", f._repo_dir.path(), &f.db, Strategy::Rebase)
            .expect("rebase sync should succeed");

        assert_eq!(result.name, "feature");
        assert_eq!(result.strategy, Strategy::Rebase);
        assert_eq!(result.before_behind, 1, "should be 1 behind before sync");

        // After rebase, behind should be 0
        assert_eq!(result.after_behind, 0, "should be 0 behind after rebase");

        // Feature branch should still have its commit + upstream file should exist
        let wt_repo = git2::Repository::open(&f.wt_path).unwrap();
        let head = wt_repo.head().unwrap().peel_to_commit().unwrap();
        assert!(
            head.message().unwrap().contains("feature commit"),
            "feature commit should be on top after rebase"
        );
        assert!(
            f.wt_path.join("upstream.txt").exists(),
            "upstream file should exist after rebase"
        );
        assert!(
            f.wt_path.join("feature.txt").exists(),
            "feature file should still exist after rebase"
        );
    }

    #[test]
    fn sync_merge_merges_base_into_branch() {
        let f = setup_diverged_repo();

        let result = execute("feature", f._repo_dir.path(), &f.db, Strategy::Merge)
            .expect("merge sync should succeed");

        assert_eq!(result.name, "feature");
        assert_eq!(result.strategy, Strategy::Merge);
        assert_eq!(result.before_behind, 1, "should be 1 behind before sync");

        // After merge, behind should be 0
        assert_eq!(result.after_behind, 0, "should be 0 behind after merge");

        // Both files should exist
        assert!(
            f.wt_path.join("upstream.txt").exists(),
            "upstream file should exist after merge"
        );
        assert!(
            f.wt_path.join("feature.txt").exists(),
            "feature file should still exist after merge"
        );

        // Should have a merge commit (the HEAD commit should have 2 parents)
        let wt_repo = git2::Repository::open(&f.wt_path).unwrap();
        let head = wt_repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_count(), 2, "merge commit should have 2 parents");
    }

    #[test]
    fn sync_writes_synced_event_to_db() {
        let f = setup_diverged_repo();

        execute("feature", f._repo_dir.path(), &f.db, Strategy::Rebase)
            .expect("sync should succeed");

        // Find the worktree and check for "synced" event
        let db_repo = f.db.get_repo_by_path(&f.repo_path_str).unwrap().unwrap();
        let wt =
            f.db.find_worktree_by_identifier(db_repo.id, "feature")
                .unwrap()
                .unwrap();

        let events = f.db.list_events(wt.id, 10).unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "synced"),
            "should have a 'synced' event in DB"
        );

        // Verify payload contains strategy and counts
        let synced_event = events.iter().find(|e| e.event_type == "synced").unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(synced_event.payload.as_deref().unwrap()).unwrap();
        assert_eq!(payload["strategy"], "rebase");
        assert_eq!(payload["base_branch"], "main");
        assert!(payload["before"].is_object());
        assert!(payload["after"].is_object());
    }

    #[test]
    fn sync_rebase_conflict_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();

        // Rename HEAD to "main"
        {
            let name = git_repo.head().unwrap().shorthand().unwrap().to_string();
            git_repo
                .find_branch(&name, git2::BranchType::Local)
                .unwrap()
                .rename("main", true)
                .unwrap();
        }

        // Create feature branch
        {
            let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
            git_repo
                .branch("conflict-feat", &head_commit, false)
                .unwrap();
        }

        // Create worktree
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("conflict-feat");
        {
            let branch_ref = git_repo
                .find_branch("conflict-feat", git2::BranchType::Local)
                .unwrap();
            let mut opts = git2::WorktreeAddOptions::new();
            opts.reference(Some(branch_ref.get()));
            git_repo
                .worktree("conflict-feat", &wt_path, Some(&opts))
                .unwrap();
        }

        // Create conflicting changes on the SAME file in both branches
        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        commit_file(
            &wt_repo,
            "conflict.txt",
            "feature version",
            "feature: edit conflict.txt",
        );

        // Switch main repo back to main and edit the same file
        {
            let main_obj = git_repo.revparse_single("refs/heads/main").unwrap();
            git_repo.checkout_tree(&main_obj, None).unwrap();
            git_repo.set_head("refs/heads/main").unwrap();
        }
        commit_file(
            &git_repo,
            "conflict.txt",
            "main version",
            "main: edit conflict.txt",
        );

        // Register in DB
        db.insert_repo("test-repo", repo_path_str, Some("main"))
            .unwrap();
        let db_repo = db.get_repo_by_path(repo_path_str).unwrap().unwrap();
        let wt_path_str = wt_path.canonicalize().unwrap_or(wt_path.clone());
        db.insert_worktree(
            db_repo.id,
            "conflict-feat",
            "conflict-feat",
            wt_path_str.to_str().unwrap(),
            Some("main"),
        )
        .unwrap();

        // Attempt sync — should fail with merge conflict
        let err = execute("conflict-feat", repo_dir.path(), &db, Strategy::Rebase)
            .expect_err("sync should fail on conflict");

        let msg = err.to_string();
        assert!(
            msg.contains("merge conflict") || msg.contains("conflict"),
            "error should mention conflict, got: {msg}"
        );

        // Verify it's a GitError::MergeConflict
        assert!(
            err.downcast_ref::<crate::git::GitError>().is_some(),
            "should be a GitError"
        );
    }

    #[test]
    fn sync_merge_conflict_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();

        {
            let name = git_repo.head().unwrap().shorthand().unwrap().to_string();
            git_repo
                .find_branch(&name, git2::BranchType::Local)
                .unwrap()
                .rename("main", true)
                .unwrap();
        }

        {
            let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
            git_repo
                .branch("merge-conflict", &head_commit, false)
                .unwrap();
        }

        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("merge-conflict");
        {
            let branch_ref = git_repo
                .find_branch("merge-conflict", git2::BranchType::Local)
                .unwrap();
            let mut opts = git2::WorktreeAddOptions::new();
            opts.reference(Some(branch_ref.get()));
            git_repo
                .worktree("merge-conflict", &wt_path, Some(&opts))
                .unwrap();
        }

        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        commit_file(
            &wt_repo,
            "shared.txt",
            "feature text",
            "feature: edit shared.txt",
        );

        {
            let main_obj = git_repo.revparse_single("refs/heads/main").unwrap();
            git_repo.checkout_tree(&main_obj, None).unwrap();
            git_repo.set_head("refs/heads/main").unwrap();
        }
        commit_file(
            &git_repo,
            "shared.txt",
            "main text",
            "main: edit shared.txt",
        );

        db.insert_repo("test-repo", repo_path_str, Some("main"))
            .unwrap();
        let db_repo = db.get_repo_by_path(repo_path_str).unwrap().unwrap();
        let wt_path_str = wt_path.canonicalize().unwrap_or(wt_path.clone());
        db.insert_worktree(
            db_repo.id,
            "merge-conflict",
            "merge-conflict",
            wt_path_str.to_str().unwrap(),
            Some("main"),
        )
        .unwrap();

        let err = execute("merge-conflict", repo_dir.path(), &db, Strategy::Merge)
            .expect_err("merge sync should fail on conflict");

        let msg = err.to_string();
        assert!(
            msg.contains("merge conflict") || msg.contains("conflict"),
            "error should mention conflict, got: {msg}"
        );

        // After merge conflict error, MERGE_HEAD should be preserved so users can resolve
        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        let merge_head_path = wt_repo.path().join("MERGE_HEAD");
        assert!(
            merge_head_path.exists(),
            "MERGE_HEAD should be preserved after merge conflict so users can run `git merge --continue`"
        );
    }

    #[test]
    fn sync_result_to_json_has_expected_structure() {
        let result = SyncResult {
            name: "my-feature".to_string(),
            strategy: Strategy::Rebase,
            before_ahead: 2,
            before_behind: 3,
            after_ahead: 2,
            after_behind: 0,
        };

        let json = result.to_json();
        let serialized = serde_json::to_value(&json).unwrap();

        assert_eq!(serialized["name"], "my-feature");
        assert_eq!(serialized["strategy"], "rebase");
        assert_eq!(serialized["before"]["ahead"], 2);
        assert_eq!(serialized["before"]["behind"], 3);
        assert_eq!(serialized["after"]["ahead"], 2);
        assert_eq!(serialized["after"]["behind"], 0);
    }

    #[test]
    fn sync_result_json_strategy_merge() {
        let result = SyncResult {
            name: "feat".to_string(),
            strategy: Strategy::Merge,
            before_ahead: 1,
            before_behind: 1,
            after_ahead: 2,
            after_behind: 0,
        };

        let json = result.to_json();
        let serialized = serde_json::to_value(&json).unwrap();
        assert_eq!(serialized["strategy"], "merge");
    }

    #[test]
    fn strategy_display_rebase() {
        assert_eq!(Strategy::Rebase.to_string(), "rebase");
    }

    #[test]
    fn strategy_display_merge() {
        assert_eq!(Strategy::Merge.to_string(), "merge");
    }

    #[test]
    fn sync_rebase_shows_correct_ahead_counts() {
        let f = setup_diverged_repo();

        let result = execute("feature", f._repo_dir.path(), &f.db, Strategy::Rebase)
            .expect("sync should succeed");

        // Feature has 1 commit ahead of main (the "feature commit")
        assert_eq!(result.before_ahead, 1, "should be 1 ahead before sync");
        // After rebase, still 1 ahead (the rebased feature commit)
        assert_eq!(
            result.after_ahead, 1,
            "should still be 1 ahead after rebase"
        );
        // Before: 1 behind (main has upstream commit)
        assert_eq!(result.before_behind, 1);
        // After: 0 behind
        assert_eq!(result.after_behind, 0);
    }

    #[test]
    fn sync_merge_shows_correct_ahead_counts() {
        let f = setup_diverged_repo();

        let result = execute("feature", f._repo_dir.path(), &f.db, Strategy::Merge)
            .expect("sync should succeed");

        // Before: 1 ahead (feature commit), 1 behind (upstream commit)
        assert_eq!(result.before_ahead, 1);
        assert_eq!(result.before_behind, 1);
        // After merge: ahead increases (feature commit + merge commit), behind = 0
        assert!(
            result.after_ahead >= 2,
            "should be at least 2 ahead after merge (feature + merge commit)"
        );
        assert_eq!(result.after_behind, 0);
    }

    #[test]
    fn sync_adopts_unmanaged_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Rename HEAD branch to "main"
        {
            let head_branch_name = git_repo.head().unwrap().shorthand().unwrap().to_string();
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
            git_repo.branch("sync-feat", &head_commit, false).unwrap();
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

    #[test]
    fn sync_falls_back_to_discovered_default_branch() {
        // Create repo WITHOUT renaming to "main" — git2 defaults to "master"
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap().to_string();

        // Get the actual default branch name (likely "master")
        let default_branch = git_repo.head().unwrap().shorthand().unwrap().to_string();

        // Create feature branch from current HEAD
        {
            let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
            git_repo.branch("feat-master", &head_commit, false).unwrap();
        }

        // Create worktree
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("feat-master");
        {
            let branch_ref = git_repo
                .find_branch("feat-master", git2::BranchType::Local)
                .unwrap();
            let mut opts = git2::WorktreeAddOptions::new();
            opts.reference(Some(branch_ref.get()));
            git_repo
                .worktree("feat-master", &wt_path, Some(&opts))
                .unwrap();
        }

        // Add a commit on the feature branch so rebase has work to do
        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        commit_file(&wt_repo, "feature.txt", "feature work", "feature commit");

        // Register repo WITHOUT default_base
        db.insert_repo("test-repo", &repo_path_str, None).unwrap();
        let db_repo = db.get_repo_by_path(&repo_path_str).unwrap().unwrap();
        // Register worktree WITHOUT base_branch
        let wt_path_str = wt_path.canonicalize().unwrap_or(wt_path.clone());
        db.insert_worktree(
            db_repo.id,
            "feat-master",
            "feat-master",
            wt_path_str.to_str().unwrap(),
            None,
        )
        .unwrap();

        // Add a commit on the default branch to create divergence
        {
            let head_ref = format!("refs/heads/{default_branch}");
            let main_obj = git_repo.revparse_single(&head_ref).unwrap();
            git_repo.checkout_tree(&main_obj, None).unwrap();
            git_repo.set_head(&head_ref).unwrap();
        }
        commit_file(
            &git_repo,
            "upstream.txt",
            "upstream change",
            "upstream commit",
        );

        // Should succeed using discovered default branch (not hard-coded "main")
        let result = execute("feat-master", repo_dir.path(), &db, Strategy::Rebase)
            .expect("sync should succeed using discovered default branch");
        assert_eq!(result.name, "feat-master");
        assert_eq!(
            result.after_behind, 0,
            "should be 0 behind after rebase onto discovered default branch"
        );
    }

    #[test]
    fn sync_rebase_rejects_dirty_worktree() {
        let f = setup_diverged_repo();

        // Write an uncommitted file to the worktree
        std::fs::write(f.wt_path.join("dirty.txt"), "uncommitted change").unwrap();

        let err = execute("feature", f._repo_dir.path(), &f.db, Strategy::Rebase)
            .expect_err("sync should reject dirty worktree");

        let msg = err.to_string();
        assert!(
            msg.contains("uncommitted"),
            "error should mention uncommitted changes, got: {msg}"
        );
    }

    #[test]
    fn sync_rebase_uses_repo_configured_identity() {
        let f = setup_diverged_repo();

        // Set custom user identity in repo config
        let main_repo = git2::Repository::open(f._repo_dir.path()).unwrap();
        let mut config = main_repo.config().unwrap();
        config.set_str("user.name", "Custom User").unwrap();
        config.set_str("user.email", "custom@example.com").unwrap();

        let _result = execute("feature", f._repo_dir.path(), &f.db, Strategy::Rebase)
            .expect("rebase sync should succeed");

        // The HEAD commit in the worktree should use the repo-configured identity
        let wt_repo = git2::Repository::open(&f.wt_path).unwrap();
        let head = wt_repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(
            head.committer().name().unwrap(),
            "Custom User",
            "committer name should match repo config"
        );
        assert_eq!(
            head.committer().email().unwrap(),
            "custom@example.com",
            "committer email should match repo config"
        );
    }

    #[test]
    fn sync_continues_when_fetch_fails() {
        let f = setup_diverged_repo();

        // Add a broken remote so fetch_remote will fail
        let main_repo = git2::Repository::open(f._repo_dir.path()).unwrap();
        main_repo
            .remote("origin", "https://invalid.example.com/nonexistent.git")
            .unwrap();

        // Sync should still succeed despite the fetch failure
        let result = execute("feature", f._repo_dir.path(), &f.db, Strategy::Rebase)
            .expect("sync should succeed even when fetch fails");

        assert_eq!(result.name, "feature");
        assert_eq!(result.after_behind, 0, "should still rebase successfully");
    }
}
