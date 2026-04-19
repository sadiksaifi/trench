use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::git;
use crate::output::json::format_json;
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::table::Table;
use crate::state::Database;

/// A unified worktree entry for list output, joined from live git state plus
/// optional trench metadata.
struct ListEntry {
    name: String,
    branch: String,
    path: String,
    base_branch: Option<String>,
    tags: Vec<String>,
}

fn fetch_all_worktrees(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
    scan_paths: &[String],
) -> Result<(PathBuf, Vec<ListEntry>)> {
    let repo_info = git::discover_repo(cwd)?;
    let live_worktrees = crate::live_worktree::list(&repo_info, db, scan_paths)?;

    let mut entries = Vec::with_capacity(live_worktrees.len());
    for worktree in live_worktrees {
        let tags = worktree
            .metadata
            .as_ref()
            .map(|metadata| db.list_tags(metadata.id))
            .transpose()?
            .unwrap_or_default();

        if let Some(tag_name) = tag {
            if !tags.iter().any(|existing| existing == tag_name) {
                continue;
            }
        }

        entries.push(ListEntry {
            name: worktree.entry.name.clone(),
            branch: worktree
                .entry
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string()),
            path: worktree.entry.path.to_string_lossy().into_owned(),
            base_branch: Some(crate::live_worktree::base_branch(&repo_info, &worktree)),
            tags,
        });
    }

    Ok((repo_info.path, entries))
}

/// Git status metadata for a worktree.
struct GitStatus {
    ahead: Option<usize>,
    behind: Option<usize>,
    dirty: usize,
}

/// Compute git status for a worktree. Expected "no upstream" cases silently
/// yield `None`; unexpected errors print a warning and fall back to defaults.
fn compute_git_status(repo_path: &Path, entry: &ListEntry) -> GitStatus {
    let wt_path = Path::new(&entry.path);

    let (ahead, behind) =
        match git::ahead_behind(repo_path, &entry.branch, entry.base_branch.as_deref()) {
            Ok(Some((a, b))) => (Some(a), Some(b)),
            Ok(None) => (None, None),
            Err(e) => {
                eprintln!("warning: ahead/behind for '{}': {e}", entry.branch);
                (None, None)
            }
        };

    let dirty = match git::dirty_count(wt_path) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("warning: dirty count for '{}': {e}", wt_path.display());
            0
        }
    };

    GitStatus {
        ahead,
        behind,
        dirty,
    }
}

/// Format ahead/behind as a display string (e.g., "+3/-1" or "-").
fn format_ahead_behind(ahead: Option<usize>, behind: Option<usize>) -> String {
    match (ahead, behind) {
        (Some(a), Some(b)) => format!("+{a}/-{b}"),
        _ => "-".to_string(),
    }
}

/// Format dirty count as a display string (e.g., "~5" or "clean").
fn format_dirty(dirty: usize) -> String {
    if dirty == 0 {
        "clean".to_string()
    } else {
        format!("~{dirty}")
    }
}

#[derive(Serialize)]
struct WorktreeJson {
    name: String,
    branch: String,
    path: String,
    status: String,
    ahead: Option<usize>,
    behind: Option<usize>,
    dirty: usize,
    tags: Vec<String>,
    process_count: usize,
    processes: Vec<String>,
}

impl PorcelainRecord for WorktreeJson {
    fn porcelain_fields(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.branch.clone(),
            self.path.clone(),
            self.status.clone(),
            self.ahead.map_or("-".to_string(), |v| v.to_string()),
            self.behind.map_or("-".to_string(), |v| v.to_string()),
            self.dirty.to_string(),
        ]
    }
}

/// Execute the `trench list` command.
///
/// Discovers the git repo from `cwd`, joins optional trench metadata, and
/// returns a formatted string for display. Optionally filters by tag.
pub fn execute(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
    scan_paths: &[String],
) -> Result<String> {
    let max_width = crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| cols as usize);
    render_table(cwd, db, tag, max_width, scan_paths)
}

fn render_table(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
    max_width: Option<usize>,
    scan_paths: &[String],
) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db, tag, scan_paths)?;

    if entries.is_empty() {
        return Ok("No worktrees. Use `trench create` to get started.\n".to_string());
    }

    let mut table = Table::new(vec![
        "Name",
        "Branch",
        "Path",
        "Status",
        "Ahead/Behind",
        "Procs",
        "Tags",
    ]);
    for entry in &entries {
        let tags_str = entry.tags.join(", ");
        let status = compute_git_status(&repo_path, entry);
        let dirty_str = format_dirty(status.dirty);
        let ab_str = format_ahead_behind(status.ahead, status.behind);
        let procs = crate::process::detect_processes(&entry.path);
        let procs_str = if procs.is_empty() {
            "-".to_string()
        } else {
            procs.len().to_string()
        };
        table = table.row(vec![
            &entry.name,
            &entry.branch,
            &entry.path,
            &dirty_str,
            &ab_str,
            &procs_str,
            &tags_str,
        ]);
    }

    if let Some(width) = max_width {
        table = table.max_width(width);
    }

    let rendered = table.render();

    Ok(rendered + "\n")
}

/// Build a `WorktreeJson` from a list entry and computed git status.
fn build_worktree_json(entry: &ListEntry, status: GitStatus) -> WorktreeJson {
    let procs = crate::process::detect_processes(&entry.path);
    let process_names: Vec<String> = procs.iter().map(|p| p.name.clone()).collect();
    let process_count = procs.len();
    WorktreeJson {
        name: entry.name.clone(),
        branch: entry.branch.clone(),
        path: entry.path.clone(),
        status: format_dirty(status.dirty),
        ahead: status.ahead,
        behind: status.behind,
        dirty: status.dirty,
        tags: entry.tags.clone(),
        process_count,
        processes: process_names,
    }
}

/// Execute the `trench list --json` command.
///
/// Returns JSON array of worktree objects including tags.
pub fn execute_json(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
    scan_paths: &[String],
) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db, tag, scan_paths)?;

    let mut json_items = Vec::new();
    for entry in &entries {
        let status = compute_git_status(&repo_path, entry);
        json_items.push(build_worktree_json(entry, status));
    }

    format_json(&json_items)
}

/// Execute the `trench list --porcelain` command.
///
/// Returns colon-separated lines: `name:branch:path:status:ahead:behind:dirty`.
pub fn execute_porcelain(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
    scan_paths: &[String],
) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db, tag, scan_paths)?;

    let items: Vec<WorktreeJson> = entries
        .iter()
        .map(|entry| {
            let status = compute_git_status(&repo_path, entry);
            build_worktree_json(entry, status)
        })
        .collect();

    Ok(format_porcelain(&items))
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

    fn create_live_worktree(
        repo_dir: &Path,
        wt_root: &Path,
        db: &Database,
        branch: &str,
    ) -> std::path::PathBuf {
        crate::cli::commands::create::execute(
            branch,
            None,
            repo_dir,
            wt_root,
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            db,
        )
        .expect("create should succeed")
        .path
    }

    #[test]
    fn displays_worktrees_in_formatted_table() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "feature/auth");
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "fix/bug");

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        // Should contain column headers
        assert!(output.contains("Name"), "output should have Name header");
        assert!(
            output.contains("Branch"),
            "output should have Branch header"
        );
        assert!(output.contains("Path"), "output should have Path header");
        assert!(
            output.contains("Status"),
            "output should have Status header"
        );

        // Should contain both worktree names
        assert!(
            output.contains("feature-auth"),
            "output should contain first worktree"
        );
        assert!(
            output.contains("fix-bug"),
            "output should contain second worktree"
        );

        // Should have header + 2 linked worktree rows + 1 main worktree row
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 5, "expected header + separator + 3 rows");
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

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        assert!(
            output.contains("feature-one"),
            "list should show first worktree, got: {output}"
        );
        assert!(
            output.contains("feature-two"),
            "list should show second worktree, got: {output}"
        );

        // header + 2 linked worktrees + 1 main worktree
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 5, "expected header + separator + 3 rows");
    }

    #[test]
    fn shows_main_worktree_when_no_linked_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        assert!(output.contains(repo_name), "main worktree should appear");
        // header + separator + 1 main worktree row
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "expected header + separator + 1 main worktree"
        );
    }

    #[test]
    fn remove_prunes_deleted_worktree_from_list() {
        use crate::cli::commands::{create, remove};
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create::execute(
            "feature/active",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .unwrap();
        create::execute(
            "feature/removed",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .unwrap();
        remove::execute("feature-removed", repo_dir.path(), &db, false).unwrap();

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        assert!(
            output.contains("feature-active"),
            "output should contain the active worktree, got: {output}"
        );
        assert!(
            !output.contains("feature-removed"),
            "output should NOT contain the removed worktree, got: {output}"
        );

        // header + 1 linked worktree row + 1 main worktree row
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines.len(),
            4,
            "expected header + separator + 2 rows, got: {output}"
        );
    }

    #[test]
    fn create_remove_list_still_shows_main_worktree() {
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

        remove::execute("ephemeral", repo_dir.path(), &db, false).expect("remove should succeed");

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        assert!(
            output.contains(repo_name),
            "main worktree should still appear"
        );
    }

    #[test]
    fn external_git_delete_hides_worktree_from_list() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        let created = create::execute(
            "ephemeral",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        std::fs::remove_dir_all(&created.path).expect("manual delete should succeed");

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        assert!(
            !output.contains("ephemeral"),
            "externally deleted worktree should not appear, got: {output}"
        );
    }

    #[test]
    fn list_with_tag_filter_shows_only_matching() {
        use crate::cli::commands::tag;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "feature/tagged");
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "feature/untagged");
        tag::execute(
            "feature-tagged",
            &["+wip".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        let output = execute(repo_dir.path(), &db, Some("wip"), &[]).unwrap();

        assert!(
            output.contains("feature-tagged"),
            "output should contain tagged worktree, got: {output}"
        );
        assert!(
            !output.contains("feature-untagged"),
            "output should NOT contain untagged worktree, got: {output}"
        );

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "expected header + separator + 1 data row");
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

        let output = execute(repo_dir.path(), &db, Some("nonexistent"), &[]).unwrap();
        assert!(output.contains("No worktrees"));
    }

    #[test]
    fn list_shows_tags_column() {
        use crate::cli::commands::tag;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "my-branch");
        tag::execute(
            "my-branch",
            &["+wip".to_string(), "+review".to_string()],
            repo_dir.path(),
            &db,
        )
        .unwrap();

        let output = execute(repo_dir.path(), &db, None, &[]).unwrap();

        assert!(output.contains("Tags"), "output should have Tags header");
        assert!(
            output.contains("review, wip"),
            "output should show tags, got: {output}"
        );
    }

    #[test]
    fn list_json_includes_tags() {
        use crate::cli::commands::tag;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "my-branch");
        tag::execute("my-branch", &["+wip".to_string()], repo_dir.path(), &db).unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let worktrees = parsed.as_array().expect("should be an array");
        assert!(
            worktrees.len() >= 2,
            "should have at least 2 entries (linked + main)"
        );
        let tagged_wt = worktrees
            .iter()
            .find(|w| w["name"] == "my-branch")
            .expect("should find linked worktree");
        let tags = tagged_wt["tags"].as_array().expect("tags should be array");
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
        tag::execute("feature-beta", &["+wip".to_string()], repo_dir.path(), &db).unwrap();

        // List all — both should appear with tags
        let all_output = render_table(repo_dir.path(), &db, None, None, &[]).unwrap();
        assert!(all_output.contains("feature-alpha"));
        assert!(all_output.contains("feature-beta"));
        assert!(all_output.contains("Tags"), "should have Tags header");

        // Filter by wip — both should appear
        let wip_output = render_table(repo_dir.path(), &db, Some("wip"), None, &[]).unwrap();
        assert!(wip_output.contains("feature-alpha"));
        assert!(wip_output.contains("feature-beta"));

        // Filter by review — only alpha
        let review_output = render_table(repo_dir.path(), &db, Some("review"), None, &[]).unwrap();
        assert!(review_output.contains("feature-alpha"));
        assert!(!review_output.contains("feature-beta"));

        // Remove wip from alpha
        tag::execute("feature-alpha", &["-wip".to_string()], repo_dir.path(), &db).unwrap();

        // Filter by wip — only beta now
        let wip_after = render_table(repo_dir.path(), &db, Some("wip"), None, &[]).unwrap();
        assert!(!wip_after.contains("feature-alpha"));
        assert!(wip_after.contains("feature-beta"));

        // JSON output should include tags (includes main worktree too)
        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().unwrap();
        // 2 managed + 1 main worktree + 2 git worktrees for the created branches
        assert!(
            items.len() >= 3,
            "should have at least 3 entries, got: {}",
            items.len()
        );

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
    fn list_json_includes_ahead_behind_dirty_fields() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        create::execute(
            "feature-json-fields",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let items = parsed.as_array().expect("should be an array");
        let wt = items
            .iter()
            .find(|i| i["name"] == "feature-json-fields")
            .expect("should find managed worktree in JSON");

        // Should have ahead, behind, and dirty fields
        assert!(
            wt.get("ahead").is_some(),
            "JSON should have 'ahead' field, got: {wt}"
        );
        assert!(
            wt.get("behind").is_some(),
            "JSON should have 'behind' field, got: {wt}"
        );
        assert!(
            wt.get("dirty").is_some(),
            "JSON should have 'dirty' field, got: {wt}"
        );

        // For a freshly created worktree, dirty should be 0
        assert_eq!(wt["dirty"], serde_json::json!(0));
    }

    #[test]
    fn list_json_shows_correct_ahead_behind_and_dirty_values() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        create::execute(
            "feature-e2e",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        // Find the worktree path from DB
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let db_repo = db
            .get_repo_by_path(repo_path.to_str().unwrap())
            .unwrap()
            .unwrap();
        let wts = db.list_worktrees(db_repo.id).unwrap();
        let wt_path = std::path::Path::new(&wts[0].path);

        // Add a commit in the worktree (makes it 1 ahead)
        {
            let wt_repo = git2::Repository::open(wt_path).unwrap();
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let parent = wt_repo.head().unwrap().peel_to_commit().unwrap();
            let tree = wt_repo
                .find_tree(wt_repo.index().unwrap().write_tree().unwrap())
                .unwrap();
            wt_repo
                .commit(Some("HEAD"), &sig, &sig, "wt commit", &tree, &[&parent])
                .unwrap();
        }

        // Create an untracked file in the worktree (makes it dirty)
        std::fs::write(wt_path.join("untracked.txt"), "dirty").unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let wt_json = parsed
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "feature-e2e")
            .expect("should find feature-e2e in JSON");

        assert_eq!(wt_json["ahead"], serde_json::json!(1), "should be 1 ahead");
        assert_eq!(
            wt_json["behind"],
            serde_json::json!(0),
            "should be 0 behind"
        );
        assert_eq!(
            wt_json["dirty"],
            serde_json::json!(1),
            "should have 1 dirty file"
        );
        assert_eq!(
            wt_json["status"],
            serde_json::json!("~1"),
            "status should show ~1"
        );
    }

    #[test]
    fn list_json_falls_back_to_default_branch_when_no_upstream() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let wt_dir = tempfile::tempdir().unwrap();

        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("orphan-branch", &head_commit, false).unwrap();
        let wt_path = wt_dir.path().join("orphan-wt");
        let branch_ref = repo
            .find_branch("orphan-branch", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        repo.worktree("orphan-wt", &wt_path, Some(&opts)).unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let wt = parsed
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "orphan-wt")
            .expect("should find orphan-wt in JSON");
        assert_eq!(wt["ahead"], serde_json::json!(0));
        assert_eq!(wt["behind"], serde_json::json!(0));
    }

    #[test]
    fn list_table_shows_dash_for_no_upstream() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let wt_dir = tempfile::tempdir().unwrap();

        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("no-upstream-branch", &head_commit, false)
            .unwrap();
        let wt_path = wt_dir.path().join("no-upstream-wt");
        let branch_ref = repo
            .find_branch("no-upstream-branch", git2::BranchType::Local)
            .unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(branch_ref.get()));
        repo.worktree("no-upstream-wt", &wt_path, Some(&opts))
            .unwrap();

        let output = execute(repo_dir.path(), &db, None, &[]).expect("list should succeed");

        // The Ahead/Behind column should show "-" for no upstream
        let row = output
            .lines()
            .find(|line| line.contains("no-upstream-wt"))
            .expect("expected no-upstream-wt row");
        assert!(
            row.split_whitespace().any(|cell| cell == "-"),
            "Ahead/Behind cell should be '-', got row: {row}"
        );
    }

    #[test]
    fn list_table_shows_ahead_behind_and_dirty_columns() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        create::execute(
            "feature-status",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let output = execute(repo_dir.path(), &db, None, &[]).expect("list should succeed");

        assert!(
            output.contains("Ahead/Behind"),
            "table should have Ahead/Behind header, got: {output}"
        );
        assert!(
            output.contains("Status"),
            "table should have Status header, got: {output}"
        );
    }

    #[test]
    fn list_porcelain_outputs_colon_separated_lines() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let feature_auth =
            create_live_worktree(repo_dir.path(), wt_root.path(), &db, "feature/auth");
        let fix_bug = create_live_worktree(repo_dir.path(), wt_root.path(), &db, "fix/bug");

        let output = execute_porcelain(repo_dir.path(), &db, None, &[]).unwrap();
        let lines: Vec<&str> = output.lines().collect();

        // 2 linked + 1 main worktree
        assert_eq!(lines.len(), 3);
        let feature_auth_line = lines
            .iter()
            .find(|line| line.starts_with("feature-auth:"))
            .expect("feature-auth should appear in porcelain");
        let feature_auth_fields: Vec<&str> = feature_auth_line.split(':').collect();
        assert_eq!(feature_auth_fields.len(), 7);
        assert_eq!(feature_auth_fields[1], "feature/auth");
        assert_eq!(feature_auth_fields[2], feature_auth.to_string_lossy());
        assert_eq!(feature_auth_fields[3], "clean");

        let fix_bug_line = lines
            .iter()
            .find(|line| line.starts_with("fix-bug:"))
            .expect("fix-bug should appear in porcelain");
        let fix_bug_fields: Vec<&str> = fix_bug_line.split(':').collect();
        assert_eq!(fix_bug_fields.len(), 7);
        assert_eq!(fix_bug_fields[1], "fix/bug");
        assert_eq!(fix_bug_fields[2], fix_bug.to_string_lossy());
        assert_eq!(fix_bug_fields[3], "clean");
    }

    #[test]
    fn list_porcelain_shows_main_worktree_when_no_linked_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output = execute_porcelain(repo_dir.path(), &db, None, &[]).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 1, "should have 1 line for main worktree");
        assert_eq!(lines[0].split(':').count(), 7);
    }

    #[test]
    fn list_json_omits_managed_field() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "my-branch");

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let worktrees = parsed.as_array().expect("should be an array");
        let linked_wt = worktrees
            .iter()
            .find(|w| w["name"] == "my-branch")
            .expect("should find linked worktree");
        assert!(linked_wt.get("managed").is_none());
    }

    #[test]
    fn integration_manual_git_worktree_appears_in_all_formats() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree via git directly (simulating manual `git worktree add`)
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("manually-added");
        git::create_worktree(repo_dir.path(), "manually-added", &base, &target)
            .expect("should create worktree via git");

        // Table output should include the manual worktree.
        let table_output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("table list should succeed");
        assert!(
            table_output.contains("manually-added"),
            "table should show manually-added worktree, got: {table_output}"
        );
        assert!(!table_output.contains("[unmanaged]"));

        let json_output =
            execute_json(repo_dir.path(), &db, None, &[]).expect("json list should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().unwrap();
        let manual_wt = items
            .iter()
            .find(|i| i["name"] == "manually-added")
            .expect("JSON should include manually-added worktree");
        assert!(manual_wt.get("managed").is_none());
        assert_eq!(manual_wt["branch"], serde_json::json!("manually-added"));
        assert!(manual_wt.get("dirty").is_some());
        assert!(manual_wt.get("status").is_some());

        let porcelain_output = execute_porcelain(repo_dir.path(), &db, None, &[])
            .expect("porcelain list should succeed");
        let manual_line = porcelain_output
            .lines()
            .find(|l| l.starts_with("manually-added:"))
            .expect("porcelain should include manually-added worktree");
        let fields: Vec<&str> = manual_line.split(':').collect();
        assert_eq!(fields.len(), 7, "porcelain should have 7 fields");
        assert_eq!(fields[0], "manually-added");

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap().to_string();
        assert!(
            table_output.contains(&repo_name),
            "table should include main worktree '{repo_name}'"
        );
        let main_json = items
            .iter()
            .find(|i| i["name"] == repo_name.as_str())
            .expect("JSON should include main worktree");
        assert!(main_json.get("managed").is_none());
    }

    #[test]
    fn list_table_omits_unmanaged_badge_and_dim_codes() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        create_live_worktree(repo_dir.path(), wt_root.path(), &db, "managed-wt");

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");
        assert!(!output.contains("[unmanaged]"));
        assert!(!output.contains("\x1b[2m"));
    }

    #[test]
    fn list_porcelain_shows_git_only_worktree_without_managed_field() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree via git directly
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("porcelain-external");
        git::create_worktree(repo_dir.path(), "porcelain-external", &base, &target)
            .expect("should create worktree via git");

        let output = execute_porcelain(repo_dir.path(), &db, None, &[]).unwrap();
        let lines: Vec<&str> = output.lines().collect();

        assert!(
            lines.len() >= 2,
            "should have at least 2 porcelain lines, got: {}",
            lines.len()
        );

        let external_line = lines
            .iter()
            .find(|l| l.starts_with("porcelain-external:"))
            .expect("should find porcelain-external in porcelain output");

        let fields: Vec<&str> = external_line.split(':').collect();
        assert_eq!(fields.len(), 7, "should have 7 fields");

        for line in &lines {
            let f: Vec<&str> = line.split(':').collect();
            assert_eq!(f.len(), 7, "each porcelain line should have 7 fields");
            assert!(
                !line.contains('\x1b'),
                "porcelain should not contain ANSI codes"
            );
        }
    }

    #[test]
    fn list_json_shows_git_only_worktree_without_managed_field() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree via git directly (not through trench)
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("git-only-wt");
        git::create_worktree(repo_dir.path(), "git-only-wt", &base, &target)
            .expect("should create worktree via git");

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().expect("should be an array");

        let worktree = items
            .iter()
            .find(|i| i["name"] == "git-only-wt")
            .expect("should find git-only worktree in JSON");
        assert!(worktree.get("managed").is_none());
        assert!(worktree["branch"].is_string());
        assert!(worktree["path"].is_string());
        assert!(worktree["status"].is_string());
        assert!(worktree["dirty"].is_number());
        assert!(worktree["tags"].is_array());

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let main_wt = items
            .iter()
            .find(|i| i["name"] == repo_name)
            .expect("should find main worktree");
        assert!(main_wt.get("managed").is_none());
    }

    #[test]
    fn list_shows_git_only_worktree_without_badge() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree via git directly (not through trench)
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("external-wt");
        git::create_worktree(repo_dir.path(), "external-wt", &base, &target)
            .expect("should create worktree via git");

        // Use render_table with no max_width to avoid terminal truncation
        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        assert!(
            output.contains("external-wt"),
            "output should contain the git-only worktree, got: {output}"
        );
        assert!(!output.contains("[unmanaged]"));
    }

    #[test]
    fn list_shows_main_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Use render_table with no max_width to avoid terminal truncation
        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap().to_string();
        assert!(
            output.contains(&repo_name),
            "output should contain the main worktree name '{repo_name}', got: {output}"
        );
        assert!(!output.contains("[unmanaged]"));
    }

    #[test]
    fn list_from_linked_worktree_still_shows_primary_checkout() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let base = repo.head().unwrap().shorthand().unwrap().to_string();
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("linked-wt");
        let db = Database::open_in_memory().unwrap();

        crate::git::create_worktree(repo_dir.path(), "linked-wt", &base, &target)
            .expect("should create linked worktree");

        let output = render_table(&target, &db, None, None, &[]).expect("list should succeed");
        let main_path = repo_dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();

        assert!(
            output.contains(&main_path),
            "output should contain primary checkout path '{main_path}', got: {output}"
        );
        assert!(
            output.contains("linked-wt"),
            "output should contain linked worktree row, got: {output}"
        );
    }

    #[test]
    fn empty_state_output_ends_with_newline() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output = execute(repo_dir.path(), &db, None, &[]).expect("list should succeed");

        assert!(
            output.ends_with('\n'),
            "empty-state output must end with newline, got: {output:?}"
        );
    }

    #[test]
    fn integration_create_worktrees_verify_json_and_porcelain() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        create::execute(
            "feature-json",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("first create should succeed");

        create::execute(
            "feature-porcelain",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("second create should succeed");

        // Verify JSON output
        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_output).expect("JSON output must be valid JSON");

        let items = parsed.as_array().expect("JSON should be an array");
        assert!(items.len() >= 3, "should have at least 3 entries");

        for item in items {
            assert!(item["name"].is_string(), "name should be a string");
            assert!(item["branch"].is_string(), "branch should be a string");
            assert!(item["path"].is_string(), "path should be a string");
            assert!(item["status"].is_string(), "status should be a string");
            assert!(item.get("managed").is_none(), "managed should be absent");
            assert!(item["tags"].is_array(), "tags should be an array");
        }

        let first = items.iter().find(|i| i["name"] == "feature-json").unwrap();
        assert!(first.get("managed").is_none());

        let porcelain_output = execute_porcelain(repo_dir.path(), &db, None, &[]).unwrap();
        let lines: Vec<&str> = porcelain_output.lines().collect();
        assert!(lines.len() >= 3, "should have at least 3 porcelain lines");

        for line in &lines {
            let fields: Vec<&str> = line.split(':').collect();
            assert_eq!(
                fields.len(),
                7,
                "porcelain line should have 7 fields, got {}: {:?}",
                fields.len(),
                line
            );
        }

        // Verify both worktrees appear in porcelain
        assert!(
            porcelain_output.contains("feature-json"),
            "porcelain should contain feature-json"
        );
        assert!(
            porcelain_output.contains("feature-porcelain"),
            "porcelain should contain feature-porcelain"
        );

        // Verify porcelain contains no ANSI escape codes
        assert!(
            !porcelain_output.contains('\x1b'),
            "porcelain output must not contain ANSI codes"
        );

        // Verify JSON contains no ANSI escape codes
        assert!(
            !json_output.contains('\x1b'),
            "JSON output must not contain ANSI codes"
        );
    }

    #[test]
    fn unborn_head_shows_detached_instead_of_empty_branch() {
        // A repo with no commits has an unborn HEAD → branch is None.
        // The branch column should show "(detached)" instead of an empty string.
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = git2::Repository::init(repo_dir.path()).unwrap();
        let db = Database::open_in_memory().unwrap();

        // JSON output: branch should be "(detached)", not ""
        let json_output = execute_json(repo_dir.path(), &db, None, &[])
            .expect("json list should succeed for unborn repo");
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().expect("should be an array");
        let main_wt = items
            .first()
            .expect("should have at least the main worktree");
        assert_eq!(
            main_wt["branch"],
            serde_json::json!("(detached)"),
            "unborn HEAD should show (detached) branch, got: {}",
            main_wt["branch"]
        );

        // Table output: should also show "(detached)"
        let table_output = render_table(repo_dir.path(), &db, None, None, &[])
            .expect("table list should succeed for unborn repo");
        assert!(
            table_output.contains("(detached)"),
            "table should show (detached) for unborn HEAD, got: {table_output}"
        );
    }

    #[test]
    fn scan_paths_worktrees_appear_in_list() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree in a custom scan directory (outside default root)
        let scan_dir = tempfile::tempdir().unwrap();
        let wt_path = scan_dir.path().join("scan-feature");
        git::create_worktree(repo_dir.path(), "scan-feature", &base, &wt_path)
            .expect("should create worktree");

        let scan_paths = vec![scan_dir.path().to_string_lossy().into_owned()];

        let output = render_table(repo_dir.path(), &db, None, None, &scan_paths)
            .expect("list with scan paths should succeed");

        assert!(
            output.contains("scan-feature"),
            "list should include worktree from scan path, got: {output}"
        );
        assert!(!output.contains("[unmanaged]"));
    }

    #[test]
    fn integration_scan_paths_discovered_in_all_formats() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create worktrees in a scan directory (simulating custom scan path)
        let scan_dir = tempfile::tempdir().unwrap();
        let wt_a = scan_dir.path().join("feature-alpha");
        let wt_b = scan_dir.path().join("feature-beta");
        git::create_worktree(repo_dir.path(), "feature-alpha", &base, &wt_a).expect("create alpha");
        git::create_worktree(repo_dir.path(), "feature-beta", &base, &wt_b).expect("create beta");

        let scan_paths = vec![scan_dir.path().to_string_lossy().into_owned()];

        // Table output should include both scanned worktrees
        let table_output = render_table(repo_dir.path(), &db, None, None, &scan_paths)
            .expect("table with scan paths should succeed");
        assert!(
            table_output.contains("feature-alpha"),
            "table should contain feature-alpha, got: {table_output}"
        );
        assert!(
            table_output.contains("feature-beta"),
            "table should contain feature-beta, got: {table_output}"
        );

        let json_output = execute_json(repo_dir.path(), &db, None, &scan_paths)
            .expect("json with scan paths should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().unwrap();

        let alpha = items
            .iter()
            .find(|i| i["name"] == "feature-alpha")
            .expect("JSON should contain feature-alpha");
        assert!(alpha.get("managed").is_none());
        assert!(alpha["branch"].is_string());

        let beta = items
            .iter()
            .find(|i| i["name"] == "feature-beta")
            .expect("JSON should contain feature-beta");
        assert!(beta.get("managed").is_none());

        // Porcelain output should include scanned worktrees
        let porcelain_output = execute_porcelain(repo_dir.path(), &db, None, &scan_paths)
            .expect("porcelain with scan paths should succeed");
        assert!(
            porcelain_output.contains("feature-alpha"),
            "porcelain should contain feature-alpha"
        );
        assert!(
            porcelain_output.contains("feature-beta"),
            "porcelain should contain feature-beta"
        );
        let alpha_line = porcelain_output
            .lines()
            .find(|l| l.starts_with("feature-alpha:"))
            .expect("should find feature-alpha in porcelain");
        assert_eq!(alpha_line.split(':').count(), 7);
    }

    #[test]
    fn scan_paths_deduplicates_with_git_discovered_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree in a scan dir — this is ALSO known to git
        let scan_dir = tempfile::tempdir().unwrap();
        let wt_path = scan_dir.path().join("known-wt");
        git::create_worktree(repo_dir.path(), "known-wt", &base, &wt_path)
            .expect("create known-wt");

        let scan_paths = vec![scan_dir.path().to_string_lossy().into_owned()];

        let json_output =
            execute_json(repo_dir.path(), &db, None, &scan_paths).expect("json should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().unwrap();

        // Count how many times known-wt appears
        let count = items.iter().filter(|i| i["name"] == "known-wt").count();
        assert_eq!(
            count, 1,
            "known-wt should appear exactly once (deduplicated), found: {count}"
        );
    }

    #[test]
    fn list_table_includes_processes_column() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output =
            render_table(repo_dir.path(), &db, None, None, &[]).expect("list should succeed");

        assert!(
            output.contains("Procs"),
            "table should have Procs header, got: {output}"
        );
    }

    #[test]
    fn list_json_includes_process_info() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None, &[]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let worktrees = parsed.as_array().expect("should be an array");
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let wt = worktrees
            .iter()
            .find(|w| w["name"] == repo_name)
            .expect("should find worktree");

        // Should have process_count and processes fields
        assert!(
            wt.get("process_count").is_some(),
            "JSON should have 'process_count' field, got: {wt}"
        );
        assert!(
            wt.get("processes").is_some(),
            "JSON should have 'processes' field, got: {wt}"
        );
        assert!(
            wt["process_count"].is_number(),
            "process_count should be a number"
        );
        assert!(wt["processes"].is_array(), "processes should be an array");
    }

    #[test]
    fn scan_paths_nonexistent_does_not_error() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let scan_paths = vec!["/nonexistent/scan/path/xyz".to_string()];

        // Should not error — non-existent paths are warnings
        let result = render_table(repo_dir.path(), &db, None, None, &scan_paths);
        assert!(
            result.is_ok(),
            "non-existent scan path should not cause error"
        );
    }
}
