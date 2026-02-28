use std::path::Path;

use anyhow::Result;

use crate::state::Database;

/// Result of a successful switch operation.
#[derive(Debug)]
pub struct SwitchResult {
    /// Absolute path to the worktree.
    pub path: String,
    /// Sanitized name of the worktree.
    pub name: String,
}

/// Execute the `trench switch <identifier>` command.
///
/// Resolves the worktree by sanitized name or branch name, updates
/// `last_accessed` and session state, and returns the worktree path.
pub fn execute(identifier: &str, cwd: &Path, db: &Database) -> Result<SwitchResult> {
    let repo_info = crate::git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db
        .get_repo_by_path(repo_path_str)?
        .ok_or_else(|| anyhow::anyhow!("repository not tracked by trench"))?;

    let wt = db
        .find_worktree_by_identifier(repo.id, identifier)?
        .ok_or_else(|| anyhow::anyhow!("worktree not found: {identifier}"))?;

    Ok(SwitchResult {
        path: wt.path.clone(),
        name: wt.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;

    /// Helper: create a temp git repo with an initial commit.
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
    fn switch_resolves_by_branch_name() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        let db_repo = db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();
        db.insert_worktree(db_repo.id, "my-feature", "my-feature", "/wt/my-feature", Some("main"))
            .unwrap();

        let result = execute("my-feature", repo_dir.path(), &db);
        let switch = result.expect("switch should succeed");

        assert_eq!(switch.path, "/wt/my-feature");
        assert_eq!(switch.name, "my-feature");
    }

    #[test]
    fn switch_not_found_returns_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_path_str = repo_path.to_str().unwrap();
        db.insert_repo("my-project", repo_path_str, Some("main")).unwrap();

        let result = execute("nonexistent", repo_dir.path(), &db);
        let err = result.expect_err("should error for nonexistent worktree");
        let msg = err.to_string();
        assert!(
            msg.contains("not found"),
            "error should mention 'not found', got: {msg}"
        );
    }
}
