use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::git;
use crate::output::table::Table;
use crate::state::Database;

#[derive(Serialize)]
struct WorktreeJson {
    name: String,
    branch: String,
    path: String,
    status: String,
    tags: Vec<String>,
}

/// Execute the `trench list` command.
///
/// Discovers the git repo from `cwd`, queries managed worktrees from the DB,
/// and returns a formatted string for display. Optionally filters by tag.
pub fn execute(cwd: &Path, db: &Database, tag: Option<&str>) -> Result<String> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db.get_repo_by_path(repo_path_str)?;

    let worktrees = match repo {
        Some(ref r) => match tag {
            Some(t) => db.list_worktrees_by_tag(r.id, t)?,
            None => db.list_worktrees(r.id)?,
        },
        None => Vec::new(),
    };

    if worktrees.is_empty() {
        return Ok("No worktrees. Use `trench create` to get started.\n".to_string());
    }

    let mut table = Table::new(vec!["Name", "Branch", "Path", "Status", "Tags"]);
    for wt in &worktrees {
        let tags = db.list_tags(wt.id)?;
        let tags_str = tags.join(", ");
        table = table.row(vec![&wt.name, &wt.branch, &wt.path, "clean", &tags_str]);
    }

    if let Ok((cols, _)) = crossterm::terminal::size() {
        table = table.max_width(cols as usize);
    }

    Ok(table.render())
}

/// Execute the `trench list --json` command.
///
/// Returns JSON array of worktree objects including tags.
pub fn execute_json(cwd: &Path, db: &Database, tag: Option<&str>) -> Result<String> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db.get_repo_by_path(repo_path_str)?;

    let worktrees = match repo {
        Some(ref r) => match tag {
            Some(t) => db.list_worktrees_by_tag(r.id, t)?,
            None => db.list_worktrees(r.id)?,
        },
        None => Vec::new(),
    };

    let mut json_items = Vec::new();
    for wt in &worktrees {
        let tags = db.list_tags(wt.id)?;
        json_items.push(WorktreeJson {
            name: wt.name.clone(),
            branch: wt.branch.clone(),
            path: wt.path.clone(),
            status: "clean".to_string(),
            tags,
        });
    }

    Ok(serde_json::to_string_pretty(&json_items)?)
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

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

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

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

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

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

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

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

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

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

        assert!(
            output.contains("No worktrees"),
            "list should show empty state after removal, got: {output}"
        );
    }

    #[test]
    fn list_with_tag_filter_shows_only_matching() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        let wt1 = db
            .insert_worktree(
                db_repo.id,
                "tagged-wt",
                "feature/tagged",
                "/wt/tagged",
                Some("main"),
            )
            .unwrap();
        db.insert_worktree(
            db_repo.id,
            "untagged-wt",
            "feature/untagged",
            "/wt/untagged",
            Some("main"),
        )
        .unwrap();

        db.add_tag(wt1.id, "wip").unwrap();

        let output = execute(repo_dir.path(), &db, Some("wip")).unwrap();

        assert!(
            output.contains("tagged-wt"),
            "output should contain tagged worktree, got: {output}"
        );
        assert!(
            !output.contains("untagged-wt"),
            "output should NOT contain untagged worktree, got: {output}"
        );

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "expected header + 1 data row");
    }

    #[test]
    fn list_with_tag_filter_shows_empty_when_no_match() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        db.insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        let output = execute(repo_dir.path(), &db, Some("nonexistent")).unwrap();
        assert!(output.contains("No worktrees"));
    }

    #[test]
    fn list_shows_tags_column() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        let wt = db
            .insert_worktree(
                db_repo.id,
                "my-wt",
                "my-branch",
                "/wt/my-wt",
                Some("main"),
            )
            .unwrap();

        db.add_tag(wt.id, "wip").unwrap();
        db.add_tag(wt.id, "review").unwrap();

        let output = execute(repo_dir.path(), &db, None).unwrap();

        assert!(output.contains("Tags"), "output should have Tags header");
        assert!(
            output.contains("review, wip"),
            "output should show tags, got: {output}"
        );
    }

    #[test]
    fn list_json_includes_tags() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        let wt = db
            .insert_worktree(
                db_repo.id,
                "my-wt",
                "my-branch",
                "/wt/my-wt",
                Some("main"),
            )
            .unwrap();

        db.add_tag(wt.id, "wip").unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let worktrees = parsed.as_array().expect("should be an array");
        assert_eq!(worktrees.len(), 1);
        let tags = worktrees[0]["tags"].as_array().expect("tags should be array");
        assert_eq!(tags, &[serde_json::json!("wip")]);
    }

    #[test]
    fn integration_tag_filter_verify_lifecycle() {
        use crate::cli::commands::{create, tag};
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        // Create two worktrees
        create::execute(
            "feature-alpha",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .unwrap();
        create::execute(
            "feature-beta",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .unwrap();

        // Tag alpha with wip and review, beta with wip only
        tag::execute(
            "feature-alpha",
            &["+wip".to_string(), "+review".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();
        tag::execute(
            "feature-beta",
            &["+wip".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        // List all — both should appear with tags
        let all_output = execute(repo_dir.path(), &db, None).unwrap();
        assert!(all_output.contains("feature-alpha"));
        assert!(all_output.contains("feature-beta"));
        assert!(all_output.contains("Tags"), "should have Tags header");

        // Filter by wip — both should appear
        let wip_output = execute(repo_dir.path(), &db, Some("wip")).unwrap();
        assert!(wip_output.contains("feature-alpha"));
        assert!(wip_output.contains("feature-beta"));

        // Filter by review — only alpha
        let review_output = execute(repo_dir.path(), &db, Some("review")).unwrap();
        assert!(review_output.contains("feature-alpha"));
        assert!(!review_output.contains("feature-beta"));

        // Remove wip from alpha
        tag::execute(
            "feature-alpha",
            &["-wip".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        // Filter by wip — only beta now
        let wip_after = execute(repo_dir.path(), &db, Some("wip")).unwrap();
        assert!(!wip_after.contains("feature-alpha"));
        assert!(wip_after.contains("feature-beta"));

        // JSON output should include tags
        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 2);

        // Find alpha in JSON and check tags
        let alpha = items
            .iter()
            .find(|i| i["name"] == "feature-alpha")
            .expect("alpha should be in JSON");
        let alpha_tags = alpha["tags"].as_array().unwrap();
        assert_eq!(alpha_tags, &[serde_json::json!("review")]);

        // Find beta in JSON and check tags
        let beta = items
            .iter()
            .find(|i| i["name"] == "feature-beta")
            .expect("beta should be in JSON");
        let beta_tags = beta["tags"].as_array().unwrap();
        assert_eq!(beta_tags, &[serde_json::json!("wip")]);
    }

    #[test]
    fn empty_state_output_ends_with_newline() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

        assert!(
            output.ends_with('\n'),
            "empty-state output must end with newline, got: {output:?}"
        );
    }
}
