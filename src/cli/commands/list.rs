use std::path::Path;

use anyhow::Result;

use crate::git;
use crate::output::table::Table;
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
        return Ok("No worktrees. Use `trench create` to get started.\n".to_string());
    }

    let mut table = Table::new(vec!["Name", "Branch", "Path", "Status"]);
    for wt in &worktrees {
        table = table.row(vec![&wt.name, &wt.branch, &wt.path, "clean"]); // TODO: wire real git status
    }

    if let Ok((cols, _)) = crossterm::terminal::size() {
        table = table.max_width(cols as usize);
    }

    Ok(table.render())
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
    fn displays_worktrees_in_formatted_table() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        db.insert_worktree(
            db_repo.id,
            "feature-auth",
            "feature/auth",
            "/home/user/.worktrees/proj/feature-auth",
            Some("main"),
        )
        .unwrap();
        db.insert_worktree(
            db_repo.id,
            "fix-bug",
            "fix/bug",
            "/home/user/.worktrees/proj/fix-bug",
            Some("main"),
        )
        .unwrap();

        let output = execute(repo_dir.path(), &db).expect("list should succeed");

        // Should contain column headers
        assert!(output.contains("Name"), "output should have Name header");
        assert!(output.contains("Branch"), "output should have Branch header");
        assert!(output.contains("Path"), "output should have Path header");
        assert!(output.contains("Status"), "output should have Status header");

        // Should contain both worktree names
        assert!(
            output.contains("feature-auth"),
            "output should contain first worktree"
        );
        assert!(
            output.contains("fix-bug"),
            "output should contain second worktree"
        );

        // Should have header + 2 data rows
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "expected header + 2 data rows");
    }

    #[test]
    fn create_two_worktrees_then_list_shows_both() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        create::execute(
            "feature-one",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("first create should succeed");

        create::execute(
            "feature-two",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("second create should succeed");

        let output = execute(repo_dir.path(), &db).expect("list should succeed");

        assert!(
            output.contains("feature-one"),
            "list should show first worktree, got: {output}"
        );
        assert!(
            output.contains("feature-two"),
            "list should show second worktree, got: {output}"
        );

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "expected header + 2 data rows");
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

    #[test]
    fn excludes_removed_worktrees() {
        use crate::state::WorktreeUpdate;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        let _active_wt = db
            .insert_worktree(
                db_repo.id,
                "active-feature",
                "feature/active",
                "/home/user/.worktrees/proj/active-feature",
                Some("main"),
            )
            .unwrap();

        let removed_wt = db
            .insert_worktree(
                db_repo.id,
                "removed-feature",
                "feature/removed",
                "/home/user/.worktrees/proj/removed-feature",
                Some("main"),
            )
            .unwrap();

        // Mark the second worktree as removed
        db.update_worktree(
            removed_wt.id,
            &WorktreeUpdate {
                removed_at: Some(Some(1_700_000_000)),
                ..Default::default()
            },
        )
        .unwrap();

        let output = execute(repo_dir.path(), &db).expect("list should succeed");

        assert!(
            output.contains("active-feature"),
            "output should contain the active worktree, got: {output}"
        );
        assert!(
            !output.contains("removed-feature"),
            "output should NOT contain the removed worktree, got: {output}"
        );

        // Should have header + 1 data row only
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "expected header + 1 data row, got: {output}");
    }

    #[test]
    fn create_remove_list_shows_empty_state() {
        use crate::cli::commands::{create, remove};
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        create::execute(
            "ephemeral",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        remove::execute("ephemeral", repo_dir.path(), &db)
            .expect("remove should succeed");

        let output = execute(repo_dir.path(), &db).expect("list should succeed");

        assert!(
            output.contains("No worktrees"),
            "list should show empty state after removal, got: {output}"
        );
    }

    #[test]
    fn empty_state_output_ends_with_newline() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output = execute(repo_dir.path(), &db).expect("list should succeed");

        assert!(
            output.ends_with('\n'),
            "empty-state output must end with newline, got: {output:?}"
        );
    }
}
