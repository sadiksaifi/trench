use std::path::Path;

use anyhow::Result;

use crate::git;
use crate::state::Database;

/// Execute the `trench list` command.
///
/// Discovers the git repo from `cwd`, queries managed worktrees from the DB,
/// and returns a formatted string for display.
pub fn execute(cwd: &Path, db: &Database) -> Result<String> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db.get_repo_by_path(repo_path_str)?;

    let worktrees = match repo {
        Some(r) => db.list_worktrees(r.id)?,
        None => Vec::new(),
    };

    if worktrees.is_empty() {
        return Ok("No worktrees. Use `trench create` to get started.".to_string());
    }

    todo!("table formatting")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn shows_empty_state_when_no_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output = execute(repo_dir.path(), &db).expect("list should succeed");

        assert!(
            output.contains("No worktrees"),
            "empty state should mention 'No worktrees', got: {output}"
        );
        assert!(
            output.contains("trench create"),
            "empty state should hint at 'trench create', got: {output}"
        );
    }
}
