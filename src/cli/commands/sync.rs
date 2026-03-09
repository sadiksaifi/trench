use std::path::Path;

use anyhow::Result;

use crate::state::Database;

/// Result of a sync operation.
#[derive(Debug)]
pub struct SyncResult {
    /// Name of the worktree that was resolved.
    pub name: String,
}

/// Execute the `trench sync <identifier>` command.
///
/// Resolves the worktree (adopting it if unmanaged), then reports
/// that sync is not yet fully implemented.
pub fn execute(identifier: &str, cwd: &Path, db: &Database) -> Result<SyncResult> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let (_repo, wt) = crate::adopt::resolve_or_adopt(identifier, &repo_info, db)?;

    // TODO: implement actual sync (rebase/merge with base branch)

    Ok(SyncResult {
        name: wt.name.clone(),
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

    #[test]
    fn sync_adopts_unmanaged_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let git_repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main"))
            .unwrap();

        // Create a git worktree manually
        let wt_dir = tempfile::tempdir().unwrap();
        let wt_path = wt_dir.path().join("sync-feat");
        git_repo
            .branch(
                "sync-feat",
                &git_repo.head().unwrap().peel_to_commit().unwrap(),
                false,
            )
            .unwrap();
        let branch_ref = git_repo
            .find_branch("sync-feat", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        git_repo
            .worktree("sync-feat", &wt_path, Some(&opts))
            .unwrap();

        // Sync the unmanaged worktree — should trigger adoption
        let result = execute("sync-feat", repo_dir.path(), &db)
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
