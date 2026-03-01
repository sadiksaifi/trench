use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::git;
use crate::output::json::format_json;
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::table::Table;
use crate::state::{Database, Worktree};

/// A unified worktree entry for list output, combining managed (DB) and
/// unmanaged (git-only) worktrees.
struct ListEntry {
    name: String,
    branch: String,
    path: String,
    base_branch: Option<String>,
    managed: bool,
    tags: Vec<String>,
}

/// Discover the git repo from `cwd` and fetch worktrees from the DB,
/// optionally filtered by tag. Returns the repo path alongside worktrees.
fn fetch_worktrees(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
) -> Result<(PathBuf, Vec<Worktree>)> {
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

    Ok((repo_info.path, worktrees))
}

/// Fetch all worktrees (managed + unmanaged) for the repo at `cwd`.
///
/// Merges DB-tracked worktrees with git-discovered worktrees. Worktrees
/// found via git but not in the DB are marked as unmanaged. When a `tag`
/// filter is active, only managed worktrees matching the tag are returned
/// (unmanaged worktrees cannot have tags).
fn fetch_all_worktrees(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
) -> Result<(PathBuf, Vec<ListEntry>)> {
    let (repo_path, db_worktrees) = fetch_worktrees(cwd, db, tag)?;

    // When filtering by tag, only return managed worktrees (unmanaged can't have tags)
    if tag.is_some() {
        let entries = db_worktrees
            .iter()
            .map(|wt| {
                let tags = db.list_tags(wt.id).unwrap_or_default();
                ListEntry {
                    name: wt.name.clone(),
                    branch: wt.branch.clone(),
                    path: wt.path.clone(),
                    base_branch: wt.base_branch.clone(),
                    managed: true,
                    tags,
                }
            })
            .collect();
        return Ok((repo_path, entries));
    }

    // Build a set of known (managed) worktree paths for cross-referencing
    let managed_paths: HashSet<PathBuf> = db_worktrees
        .iter()
        .filter_map(|wt| {
            Path::new(&wt.path)
                .canonicalize()
                .ok()
        })
        .collect();

    let mut entries: Vec<ListEntry> = Vec::new();

    // Add managed worktrees first
    for wt in &db_worktrees {
        let tags = db.list_tags(wt.id).unwrap_or_default();
        entries.push(ListEntry {
            name: wt.name.clone(),
            branch: wt.branch.clone(),
            path: wt.path.clone(),
            base_branch: wt.base_branch.clone(),
            managed: true,
            tags,
        });
    }

    // Discover git worktrees and add unmanaged ones
    if let Ok(git_worktrees) = git::list_worktrees(&repo_path) {
        for gw in git_worktrees {
            if !managed_paths.contains(&gw.path) {
                entries.push(ListEntry {
                    name: gw.name.clone(),
                    branch: gw.branch.unwrap_or_default(),
                    path: gw.path.to_string_lossy().into_owned(),
                    base_branch: None,
                    managed: false,
                    tags: Vec::new(),
                });
            }
        }
    }

    Ok((repo_path, entries))
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
    managed: bool,
    tags: Vec<String>,
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
            self.managed.to_string(),
        ]
    }
}

/// Execute the `trench list` command.
///
/// Discovers the git repo from `cwd`, queries managed worktrees from the DB,
/// merges with git-discovered unmanaged worktrees, and returns a formatted
/// string for display. Optionally filters by tag.
pub fn execute(cwd: &Path, db: &Database, tag: Option<&str>) -> Result<String> {
    let max_width = crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| cols as usize);
    render_table(cwd, db, tag, max_width)
}

fn render_table(
    cwd: &Path,
    db: &Database,
    tag: Option<&str>,
    max_width: Option<usize>,
) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db, tag)?;

    if entries.is_empty() {
        return Ok("No worktrees. Use `trench create` to get started.\n".to_string());
    }

    let mut table = Table::new(vec![
        "Name",
        "Branch",
        "Path",
        "Status",
        "Ahead/Behind",
        "Tags",
    ]);
    let mut unmanaged_rows: Vec<bool> = Vec::new();
    for entry in &entries {
        let tags_str = entry.tags.join(", ");
        let status = compute_git_status(&repo_path, entry);
        let dirty_str = format_dirty(status.dirty);
        let ab_str = format_ahead_behind(status.ahead, status.behind);
        let display_name = if entry.managed {
            entry.name.clone()
        } else {
            format!("{} [unmanaged]", entry.name)
        };
        table = table.row(vec![
            &display_name,
            &entry.branch,
            &entry.path,
            &dirty_str,
            &ab_str,
            &tags_str,
        ]);
        unmanaged_rows.push(!entry.managed);
    }

    if let Some(width) = max_width {
        table = table.max_width(width);
    }

    let rendered = table.render();

    // Apply dimmed styling to unmanaged worktree rows
    let lines: Vec<&str> = rendered.lines().collect();
    let mut out = String::new();
    if let Some(header) = lines.first() {
        out.push_str(header);
        out.push('\n');
    }
    for (i, line) in lines.iter().skip(1).enumerate() {
        if i < unmanaged_rows.len() && unmanaged_rows[i] {
            out.push_str("\x1b[2m");
            out.push_str(line);
            out.push_str("\x1b[0m");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    Ok(out)
}

/// Build a `WorktreeJson` from a list entry and computed git status.
fn build_worktree_json(entry: &ListEntry, status: GitStatus) -> WorktreeJson {
    WorktreeJson {
        name: entry.name.clone(),
        branch: entry.branch.clone(),
        path: entry.path.clone(),
        status: format_dirty(status.dirty),
        ahead: status.ahead,
        behind: status.behind,
        dirty: status.dirty,
        managed: entry.managed,
        tags: entry.tags.clone(),
    }
}

/// Execute the `trench list --json` command.
///
/// Returns JSON array of worktree objects including tags.
pub fn execute_json(cwd: &Path, db: &Database, tag: Option<&str>) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db, tag)?;

    let mut json_items = Vec::new();
    for entry in &entries {
        let status = compute_git_status(&repo_path, entry);
        json_items.push(build_worktree_json(entry, status));
    }

    format_json(&json_items)
}

/// Execute the `trench list --porcelain` command.
///
/// Returns colon-separated lines: `name:branch:path:status:ahead:behind:dirty:managed`.
pub fn execute_porcelain(cwd: &Path, db: &Database, tag: Option<&str>) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db, tag)?;

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

        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

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

        // Should have header + 2 managed rows + 1 main worktree row
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4, "expected header + 2 managed + 1 main worktree");
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

        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

        assert!(
            output.contains("feature-one"),
            "list should show first worktree, got: {output}"
        );
        assert!(
            output.contains("feature-two"),
            "list should show second worktree, got: {output}"
        );

        // header + 2 managed + 1 main worktree
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4, "expected header + 2 managed + 1 main worktree");
    }

    #[test]
    fn shows_main_worktree_when_no_managed_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // With no managed worktrees, the main worktree still appears
        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

        assert!(
            output.contains("[unmanaged]"),
            "main worktree should show [unmanaged] badge, got: {output}"
        );
        // header + 1 main worktree row
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "expected header + 1 main worktree");
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

        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

        assert!(
            output.contains("active-feature"),
            "output should contain the active worktree, got: {output}"
        );
        assert!(
            !output.contains("removed-feature"),
            "output should NOT contain the removed worktree, got: {output}"
        );

        // header + 1 managed row + 1 main worktree row
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "expected header + 1 managed + 1 main worktree, got: {output}");
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

        remove::execute("ephemeral", repo_dir.path(), &db)
            .expect("remove should succeed");

        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

        // After removing all managed worktrees, the main worktree still appears
        assert!(
            output.contains("[unmanaged]"),
            "main worktree should still appear after removal, got: {output}"
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
        // 1 managed + 1 main worktree
        assert!(worktrees.len() >= 2, "should have at least 2 entries (managed + main)");
        let tagged_wt = worktrees.iter().find(|w| w["name"] == "my-wt")
            .expect("should find managed worktree");
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

        // JSON output should include tags (includes main worktree too)
        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().unwrap();
        // 2 managed + 1 main worktree + 2 git worktrees for the created branches
        assert!(items.len() >= 3, "should have at least 3 entries, got: {}", items.len());

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

        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let items = parsed.as_array().expect("should be an array");
        let wt = items.iter().find(|i| i["name"] == "feature-json-fields")
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

        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let wt_json = parsed.as_array().unwrap().iter()
            .find(|i| i["name"] == "feature-e2e")
            .expect("should find feature-e2e in JSON");

        assert_eq!(wt_json["ahead"], serde_json::json!(1), "should be 1 ahead");
        assert_eq!(wt_json["behind"], serde_json::json!(0), "should be 0 behind");
        assert_eq!(wt_json["dirty"], serde_json::json!(1), "should have 1 dirty file");
        assert_eq!(wt_json["status"], serde_json::json!("~1"), "status should show ~1");
    }

    #[test]
    fn list_json_shows_null_ahead_behind_when_no_upstream() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Create a real local branch with no upstream tracking
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("orphan-branch", &head_commit, false).unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        // Insert worktree pointing to the real branch and repo path, no base_branch
        db.insert_worktree(
            db_repo.id,
            "orphan-wt",
            "orphan-branch",
            repo_path.to_str().unwrap(),
            None, // no base_branch
        )
        .unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let wt = parsed.as_array().unwrap().iter()
            .find(|i| i["name"] == "orphan-wt")
            .expect("should find orphan-wt in JSON");
        assert!(
            wt["ahead"].is_null(),
            "ahead should be null when no upstream, got: {}",
            wt["ahead"]
        );
        assert!(
            wt["behind"].is_null(),
            "behind should be null when no upstream, got: {}",
            wt["behind"]
        );
    }

    #[test]
    fn list_table_shows_dash_for_no_upstream() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Create a real local branch with no upstream tracking
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("no-upstream-branch", &head_commit, false).unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        db.insert_worktree(
            db_repo.id,
            "no-upstream-wt",
            "no-upstream-branch",
            repo_path.to_str().unwrap(),
            None,
        )
        .unwrap();

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

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

        let output = execute(repo_dir.path(), &db, None).expect("list should succeed");

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

        let output = execute_porcelain(repo_dir.path(), &db, None).unwrap();
        let lines: Vec<&str> = output.lines().collect();

        // 2 managed + 1 main worktree
        assert_eq!(lines.len(), 3);
        // Porcelain format: name:branch:path:status:ahead:behind:dirty:managed
        assert_eq!(
            lines[0],
            "feature-auth:feature/auth:/home/user/.worktrees/proj/feature-auth:clean:-:-:0:true"
        );
        assert_eq!(
            lines[1],
            "fix-bug:fix/bug:/home/user/.worktrees/proj/fix-bug:clean:-:-:0:true"
        );
        // Third line is the main worktree (unmanaged)
        assert!(
            lines[2].ends_with(":false"),
            "main worktree should have managed=false, got: {}",
            lines[2]
        );
    }

    #[test]
    fn list_porcelain_shows_main_worktree_when_no_managed() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output = execute_porcelain(repo_dir.path(), &db, None).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        // Main worktree should appear
        assert_eq!(lines.len(), 1, "should have 1 line for main worktree");
        assert!(
            lines[0].ends_with(":false"),
            "main worktree should have managed=false, got: {}",
            lines[0]
        );
    }

    #[test]
    fn list_json_includes_managed_field() {
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
            "my-wt",
            "my-branch",
            "/wt/my-wt",
            Some("main"),
        )
        .unwrap();

        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();

        let worktrees = parsed.as_array().expect("should be an array");
        let managed_wt = worktrees.iter().find(|w| w["name"] == "my-wt")
            .expect("should find managed worktree");
        assert_eq!(
            managed_wt["managed"],
            serde_json::json!(true),
            "managed worktree should have managed=true"
        );
        // Main worktree should also be present with managed=false
        let main_wt = worktrees.iter().find(|w| w["managed"] == false)
            .expect("should find unmanaged worktree");
        assert_eq!(main_wt["managed"], serde_json::json!(false));
    }

    #[test]
    fn list_porcelain_shows_unmanaged_worktree_with_managed_false() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree via git directly
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("porcelain-external");
        git::create_worktree(repo_dir.path(), "porcelain-external", &base, &target)
            .expect("should create worktree via git");

        let output = execute_porcelain(repo_dir.path(), &db, None).unwrap();
        let lines: Vec<&str> = output.lines().collect();

        // Should have at least 2 entries (main + the external worktree)
        assert!(lines.len() >= 2, "should have at least 2 porcelain lines, got: {}", lines.len());

        // Find the line for the unmanaged worktree
        let external_line = lines.iter()
            .find(|l| l.starts_with("porcelain-external:"))
            .expect("should find porcelain-external in porcelain output");

        let fields: Vec<&str> = external_line.split(':').collect();
        assert_eq!(fields.len(), 8, "should have 8 fields");
        assert_eq!(fields[7], "false", "managed field should be 'false'");

        // All lines should have 8 fields and no ANSI codes
        for line in &lines {
            let f: Vec<&str> = line.split(':').collect();
            assert_eq!(f.len(), 8, "each porcelain line should have 8 fields");
            assert!(!line.contains('\x1b'), "porcelain should not contain ANSI codes");
        }
    }

    #[test]
    fn list_json_shows_unmanaged_worktree_with_managed_false() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let base = repo.head().unwrap().shorthand().unwrap().to_string();

        // Create a worktree via git directly (not through trench)
        let wt_dir = tempfile::tempdir().unwrap();
        let target = wt_dir.path().join("git-only-wt");
        git::create_worktree(repo_dir.path(), "git-only-wt", &base, &target)
            .expect("should create worktree via git");

        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        let items = parsed.as_array().expect("should be an array");

        // Find the unmanaged worktree
        let unmanaged = items.iter().find(|i| i["name"] == "git-only-wt")
            .expect("should find unmanaged worktree in JSON");
        assert_eq!(
            unmanaged["managed"],
            serde_json::json!(false),
            "unmanaged worktree should have managed=false"
        );
        // Should still have all required fields
        assert!(unmanaged["branch"].is_string());
        assert!(unmanaged["path"].is_string());
        assert!(unmanaged["status"].is_string());
        assert!(unmanaged["dirty"].is_number());
        assert!(unmanaged["tags"].is_array());

        // Main worktree should also be unmanaged
        let main_wt = items.iter().find(|i| {
            i["managed"] == false && i["name"] != "git-only-wt"
        }).expect("should find main worktree as unmanaged");
        assert_eq!(main_wt["managed"], serde_json::json!(false));
    }

    #[test]
    fn list_shows_unmanaged_worktree_with_badge() {
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
        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

        assert!(
            output.contains("external-wt"),
            "output should contain the unmanaged worktree, got: {output}"
        );
        assert!(
            output.contains("[unmanaged]"),
            "output should show [unmanaged] badge, got: {output}"
        );
    }

    #[test]
    fn list_shows_main_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        // Use render_table with no max_width to avoid terminal truncation
        let output = render_table(repo_dir.path(), &db, None, None).expect("list should succeed");

        // Main worktree should appear (it's unmanaged by trench)
        let repo_name = repo_dir.path().canonicalize().unwrap()
            .file_name().unwrap().to_str().unwrap().to_string();
        assert!(
            output.contains(&repo_name),
            "output should contain the main worktree name '{repo_name}', got: {output}"
        );
        assert!(
            output.contains("[unmanaged]"),
            "main worktree should show [unmanaged] badge, got: {output}"
        );
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
        let json_output = execute_json(repo_dir.path(), &db, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_output)
            .expect("JSON output must be valid JSON");

        let items = parsed.as_array().expect("JSON should be an array");
        // 2 managed + main worktree (+ possibly git worktrees for created branches)
        assert!(items.len() >= 3, "should have at least 3 entries");

        // All items should have required fields
        for item in items {
            assert!(item["name"].is_string(), "name should be a string");
            assert!(item["branch"].is_string(), "branch should be a string");
            assert!(item["path"].is_string(), "path should be a string");
            assert!(item["status"].is_string(), "status should be a string");
            assert!(item["managed"].is_boolean(), "managed should be a boolean");
            assert!(item["tags"].is_array(), "tags should be an array");
        }

        // Verify managed is true for worktrees created by trench
        let first = items.iter().find(|i| i["name"] == "feature-json").unwrap();
        assert_eq!(first["managed"], serde_json::json!(true));

        // Verify porcelain output
        let porcelain_output = execute_porcelain(repo_dir.path(), &db, None).unwrap();
        let lines: Vec<&str> = porcelain_output.lines().collect();
        assert!(lines.len() >= 3, "should have at least 3 porcelain lines");

        // Each line should have exactly 8 colon-separated fields
        // (name:branch:path:status:ahead:behind:dirty:managed)
        for line in &lines {
            let fields: Vec<&str> = line.split(':').collect();
            assert_eq!(
                fields.len(), 8,
                "porcelain line should have 8 fields, got {}: {:?}",
                fields.len(), line
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
}
