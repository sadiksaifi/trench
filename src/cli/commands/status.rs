use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::git;
use crate::output::json::{format_json, format_json_value};
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::table::Table;
use crate::state::Database;

/// A unified worktree entry for status output.
struct StatusEntry {
    name: String,
    branch: String,
    path: String,
    base_branch: Option<String>,
    db_id: Option<i64>,
}

fn fetch_all_worktrees(cwd: &Path, db: &Database) -> Result<(PathBuf, Vec<StatusEntry>)> {
    let repo_info = git::discover_repo(cwd)?;
    let live_worktrees = crate::live_worktree::list(&repo_info, db, &[])?;
    let mut entries = Vec::with_capacity(live_worktrees.len());

    for worktree in live_worktrees {
        let base_branch = crate::live_worktree::base_branch(&repo_info, &worktree);
        let db_id = worktree.metadata.as_ref().map(|metadata| metadata.id);
        entries.push(StatusEntry {
            name: worktree.entry.name.clone(),
            branch: worktree
                .entry
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string()),
            path: worktree.entry.path.to_string_lossy().into_owned(),
            base_branch: Some(base_branch),
            db_id,
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

fn compute_git_status(repo_path: &Path, entry: &StatusEntry) -> GitStatus {
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

fn format_ahead_behind(ahead: Option<usize>, behind: Option<usize>) -> String {
    match (ahead, behind) {
        (Some(a), Some(b)) => format!("+{a}/-{b}"),
        _ => "-".to_string(),
    }
}

fn format_dirty(dirty: usize) -> String {
    if dirty == 0 {
        "clean".to_string()
    } else {
        format!("~{dirty}")
    }
}

fn render_summary_table(
    cwd: &Path,
    db: &Database,
    max_width: Option<usize>,
    use_color: bool,
) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db)?;

    if entries.is_empty() {
        return Ok("No worktrees.\n".to_string());
    }

    let mut table = Table::new(vec!["Name", "Branch", "Status", "Ahead/Behind"]);

    for entry in &entries {
        let status = compute_git_status(&repo_path, entry);
        let dirty_str = format_dirty(status.dirty);
        let ab_str = format_ahead_behind(status.ahead, status.behind);
        table = table.row(vec![&entry.name, &entry.branch, &dirty_str, &ab_str]);
    }

    if let Some(width) = max_width {
        table = table.max_width(width);
    }

    let rendered = table.render();

    let _ = use_color;
    Ok(rendered + "\n")
}

fn resolve_worktree(cwd: &Path, db: &Database, identifier: &str) -> Result<(PathBuf, StatusEntry)> {
    let repo_info = git::discover_repo(cwd)?;
    let worktree = crate::live_worktree::resolve(identifier, &repo_info, db)?;
    let base_branch = crate::live_worktree::base_branch(&repo_info, &worktree);

    Ok((
        repo_info.path,
        StatusEntry {
            name: worktree.entry.name.clone(),
            branch: worktree
                .entry
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string()),
            path: worktree.entry.path.to_string_lossy().into_owned(),
            base_branch: Some(base_branch),
            db_id: worktree.metadata.as_ref().map(|metadata| metadata.id),
        },
    ))
}

fn render_deep(cwd: &Path, db: &Database, identifier: &str) -> Result<String> {
    let (repo_path, entry) = resolve_worktree(cwd, db, identifier)?;
    let status = compute_git_status(&repo_path, &entry);

    let mut out = String::new();
    out.push_str(&format!("Branch:       {}\n", entry.branch));
    out.push_str(&format!("Path:         {}\n", entry.path));
    if let Some(ref base) = entry.base_branch {
        out.push_str(&format!("Base:         {base}\n"));
    }
    let ab = format_ahead_behind(status.ahead, status.behind);
    out.push_str(&format!("Ahead/Behind: {ab}\n"));
    out.push_str(&format!("Status:       {}\n", format_dirty(status.dirty)));

    // Changed files
    let wt_path = Path::new(&entry.path);
    let changed = git::changed_files(wt_path).unwrap_or_default();
    if !changed.is_empty() {
        out.push_str("\nChanged files:\n");
        for f in &changed {
            out.push_str(&format!("  {} {}\n", f.status, f.path));
        }
    }

    // Recent commits
    let commits = git::recent_commits(wt_path, 10).unwrap_or_default();
    if !commits.is_empty() {
        out.push_str("\nRecent commits:\n");
        for c in &commits {
            out.push_str(&format!("  {} {}\n", c.hash, c.message));
        }
    }

    // Hook history
    if let Some(wt_id) = entry.db_id {
        let events = db.list_events(wt_id, 10).unwrap_or_default();
        if !events.is_empty() {
            out.push_str("\nHook history:\n");
            for ev in &events {
                out.push_str(&format!("  {}\n", ev.event_type));
            }
        }
    }

    Ok(out)
}

pub fn execute(cwd: &Path, db: &Database, branch: Option<&str>, use_color: bool) -> Result<String> {
    match branch {
        Some(id) => render_deep(cwd, db, id),
        None => render_summary_table(
            cwd,
            db,
            crossterm::terminal::size().ok().map(|(c, _)| c as usize),
            use_color,
        ),
    }
}

/// JSON output for summary mode.
#[derive(Serialize)]
struct SummaryJson {
    name: String,
    branch: String,
    path: String,
    status: String,
    ahead: Option<usize>,
    behind: Option<usize>,
    dirty: usize,
}

impl PorcelainRecord for SummaryJson {
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

fn build_summary_json(entry: &StatusEntry, status: GitStatus) -> SummaryJson {
    SummaryJson {
        name: entry.name.clone(),
        branch: entry.branch.clone(),
        path: entry.path.clone(),
        status: format_dirty(status.dirty),
        ahead: status.ahead,
        behind: status.behind,
        dirty: status.dirty,
    }
}

/// JSON output for deep mode.
#[derive(Serialize)]
struct DeepJson {
    name: String,
    branch: String,
    path: String,
    base_branch: Option<String>,
    ahead: Option<usize>,
    behind: Option<usize>,
    dirty: usize,
    status: String,
    changed_files: Vec<String>,
    recent_commits: Vec<String>,
    hook_history: Vec<String>,
}

fn build_deep_json(entry: &StatusEntry, status: GitStatus, db: &Database) -> DeepJson {
    let wt_path = Path::new(&entry.path);
    let changed = git::changed_files(wt_path)
        .unwrap_or_default()
        .into_iter()
        .map(|f| format!("{} {}", f.status, f.path))
        .collect();
    let commits = git::recent_commits(wt_path, 10)
        .unwrap_or_default()
        .into_iter()
        .map(|c| format!("{} {}", c.hash, c.message))
        .collect();
    let hook_history = entry
        .db_id
        .and_then(|id| db.list_events(id, 10).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|ev| ev.event_type)
        .collect();

    DeepJson {
        name: entry.name.clone(),
        branch: entry.branch.clone(),
        path: entry.path.clone(),
        base_branch: entry.base_branch.clone(),
        ahead: status.ahead,
        behind: status.behind,
        dirty: status.dirty,
        status: format_dirty(status.dirty),
        changed_files: changed,
        recent_commits: commits,
        hook_history,
    }
}

pub fn execute_json(cwd: &Path, db: &Database, branch: Option<&str>) -> Result<String> {
    match branch {
        Some(id) => {
            let (repo_path, entry) = resolve_worktree(cwd, db, id)?;
            let status = compute_git_status(&repo_path, &entry);
            let json_obj = build_deep_json(&entry, status, db);
            format_json_value(&json_obj)
        }
        None => {
            let (repo_path, entries) = fetch_all_worktrees(cwd, db)?;
            let items: Vec<SummaryJson> = entries
                .iter()
                .map(|e| {
                    let status = compute_git_status(&repo_path, e);
                    build_summary_json(e, status)
                })
                .collect();
            format_json(&items)
        }
    }
}

pub fn execute_porcelain(cwd: &Path, db: &Database, branch: Option<&str>) -> Result<String> {
    match branch {
        Some(id) => {
            let (repo_path, entry) = resolve_worktree(cwd, db, id)?;
            let status = compute_git_status(&repo_path, &entry);
            let item = build_summary_json(&entry, status);
            Ok(format_porcelain(&[item]))
        }
        None => {
            let (repo_path, entries) = fetch_all_worktrees(cwd, db)?;
            let items: Vec<SummaryJson> = entries
                .iter()
                .map(|e| {
                    let status = compute_git_status(&repo_path, e);
                    build_summary_json(e, status)
                })
                .collect();
            Ok(format_porcelain(&items))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
        db: &Database,
        branch: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let wt_root = tempfile::tempdir().unwrap();
        let result = crate::cli::commands::create::execute(
            branch,
            None,
            repo_dir,
            wt_root.path(),
            crate::paths::DEFAULT_WORKTREE_TEMPLATE,
            db,
        )
        .expect("create should succeed");
        (wt_root, result.path)
    }

    #[test]
    fn summary_shows_all_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let (_feature_auth_root, _) = create_live_worktree(repo_dir.path(), &db, "feature/auth");
        let (_fix_bug_root, _) = create_live_worktree(repo_dir.path(), &db, "fix/bug");

        let output = render_summary_table(repo_dir.path(), &db, None, false)
            .expect("summary should succeed");

        assert!(output.contains("Name"), "should have Name header");
        assert!(output.contains("Branch"), "should have Branch header");
        assert!(
            output.contains("feature-auth"),
            "should show first worktree"
        );
        assert!(output.contains("fix-bug"), "should show second worktree");
    }

    #[test]
    fn summary_table_no_ansi_when_color_disabled() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let output =
            render_summary_table(repo_dir.path(), &db, None, false).expect("should succeed");
        assert!(
            !output.contains("\x1b"),
            "should not contain ANSI escape codes when color is disabled, got:\n{output}"
        );
    }

    #[test]
    fn summary_json_returns_array_of_worktrees() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let (_wt_root, _) = create_live_worktree(repo_dir.path(), &db, "feature/auth");

        let output = execute_json(repo_dir.path(), &db, None).expect("summary json should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().expect("should be array");

        assert!(
            arr.len() >= 2,
            "should have at least 2 entries, got {}",
            arr.len()
        );

        let wt = arr
            .iter()
            .find(|v| v["name"] == "feature-auth")
            .expect("should contain feature-auth");
        assert_eq!(wt["branch"], "feature/auth");
        assert!(wt.get("managed").is_none());
        assert!(wt["path"].is_string());
    }

    #[test]
    fn deep_view_includes_changed_files() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());

        // Create a worktree with a modified file
        let wt_parent = tempfile::tempdir().unwrap();
        let wt_path = wt_parent.path().join("test-changes");
        let head = repo.head().unwrap().shorthand().unwrap().to_string();
        let head_commit = repo
            .find_branch(&head, git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap();
        repo.branch("test-changes", &head_commit, false).unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        let branch_ref = repo
            .find_branch("test-changes", git2::BranchType::Local)
            .unwrap();
        opts.reference(Some(branch_ref.get()));
        repo.worktree("test-changes", &wt_path, Some(&opts))
            .unwrap();

        // Create a new file in the worktree
        std::fs::write(wt_path.join("new-file.txt"), "hello").unwrap();

        let db = Database::open_in_memory().unwrap();
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some(&head))
            .unwrap();
        let wt_canonical = wt_path.canonicalize().unwrap();
        db.insert_worktree(
            db_repo.id,
            "test-changes",
            "test-changes",
            wt_canonical.to_str().unwrap(),
            Some(&head),
        )
        .unwrap();

        let output =
            render_deep(repo_dir.path(), &db, "test-changes").expect("deep should succeed");

        assert!(
            output.contains("Changed files"),
            "should have Changed files section, got:\n{output}"
        );
        assert!(
            output.contains("new-file.txt"),
            "should list the changed file, got:\n{output}"
        );
    }

    #[test]
    fn deep_view_includes_recent_commits() {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = init_repo_with_commit(repo_dir.path());

        // Create a branch with an extra commit
        let wt_parent = tempfile::tempdir().unwrap();
        let wt_path = wt_parent.path().join("test-commits");
        let head = repo.head().unwrap().shorthand().unwrap().to_string();
        let head_commit = repo
            .find_branch(&head, git2::BranchType::Local)
            .unwrap()
            .get()
            .peel_to_commit()
            .unwrap();
        repo.branch("test-commits", &head_commit, false).unwrap();
        let mut opts = git2::WorktreeAddOptions::new();
        let branch_ref = repo
            .find_branch("test-commits", git2::BranchType::Local)
            .unwrap();
        opts.reference(Some(branch_ref.get()));
        repo.worktree("test-commits", &wt_path, Some(&opts))
            .unwrap();

        // Make a commit in the worktree
        let wt_repo = git2::Repository::open(&wt_path).unwrap();
        std::fs::write(wt_path.join("file.txt"), "content").unwrap();
        let mut index = wt_repo.index().unwrap();
        index.add_path(std::path::Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = wt_repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let parent = wt_repo.head().unwrap().peel_to_commit().unwrap();
        wt_repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "add file.txt for testing",
                &tree,
                &[&parent],
            )
            .unwrap();

        let db = Database::open_in_memory().unwrap();
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        let db_repo = db
            .insert_repo(repo_name, repo_path.to_str().unwrap(), Some(&head))
            .unwrap();
        let wt_canonical = wt_path.canonicalize().unwrap();
        db.insert_worktree(
            db_repo.id,
            "test-commits",
            "test-commits",
            wt_canonical.to_str().unwrap(),
            Some(&head),
        )
        .unwrap();

        let output =
            render_deep(repo_dir.path(), &db, "test-commits").expect("deep should succeed");

        assert!(
            output.contains("Recent commits"),
            "should have Recent commits section, got:\n{output}"
        );
        assert!(
            output.contains("add file.txt for testing"),
            "should show commit message, got:\n{output}"
        );
    }

    #[test]
    fn deep_view_includes_hook_history() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let (_wt_root, _) = create_live_worktree(repo_dir.path(), &db, "feature/auth");
        let repo_path = repo_dir.path().canonicalize().unwrap();
        let db_repo = db
            .get_repo_by_path(repo_path.to_str().unwrap())
            .unwrap()
            .unwrap();
        let wt = db
            .find_worktree_by_identifier(db_repo.id, "feature-auth")
            .unwrap()
            .unwrap();

        // Insert some events
        let payload = serde_json::json!({"status": "success"});
        db.insert_event(db_repo.id, Some(wt.id), "post_create", Some(&payload))
            .unwrap();
        db.insert_event(db_repo.id, Some(wt.id), "post_sync", None)
            .unwrap();

        let output =
            render_deep(repo_dir.path(), &db, "feature-auth").expect("deep should succeed");

        assert!(
            output.contains("Hook history"),
            "should have Hook history section, got:\n{output}"
        );
        assert!(
            output.contains("post_create"),
            "should show post_create event, got:\n{output}"
        );
        assert!(
            output.contains("post_sync"),
            "should show post_sync event, got:\n{output}"
        );
    }

    #[test]
    fn deep_json_returns_single_object() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let (_wt_root, wt_path) = create_live_worktree(repo_dir.path(), &db, "feature/auth");

        let output = execute_json(repo_dir.path(), &db, Some("feature-auth"))
            .expect("deep json should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert!(parsed.is_object(), "should be a single JSON object");
        assert_eq!(parsed["name"], "feature-auth");
        assert_eq!(parsed["branch"], "feature/auth");
        assert_eq!(parsed["base_branch"], "main");
        assert!(parsed.get("managed").is_none());
        assert_eq!(parsed["path"], serde_json::json!(wt_path.to_string_lossy()));
        assert!(parsed["changed_files"].is_array());
        assert!(parsed["recent_commits"].is_array());
        assert!(parsed["hook_history"].is_array());
    }

    #[test]
    fn deep_mode_errors_for_nonexistent_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();

        let repo_path = repo_dir.path().canonicalize().unwrap();
        let repo_name = repo_path.file_name().unwrap().to_str().unwrap();
        db.insert_repo(repo_name, repo_path.to_str().unwrap(), Some("main"))
            .unwrap();

        let result = render_deep(repo_dir.path(), &db, "nonexistent");
        assert!(result.is_err(), "should error for nonexistent worktree");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not found"),
            "error should mention 'not found', got: {msg}"
        );
    }

    #[test]
    fn deep_mode_shows_detail_for_linked_worktree() {
        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let db = Database::open_in_memory().unwrap();
        let (_wt_root, wt_path) = create_live_worktree(repo_dir.path(), &db, "feature/auth");

        let output =
            render_deep(repo_dir.path(), &db, "feature-auth").expect("deep should succeed");

        assert!(output.contains("Branch:"), "should show Branch label");
        assert!(output.contains("feature/auth"), "should show branch name");
        assert!(output.contains("Path:"), "should show Path label");
        assert!(
            output.contains(wt_path.to_string_lossy().as_ref()),
            "should show path"
        );
        assert!(output.contains("Base:"), "should show Base label");
        assert!(output.contains("main"), "should show base branch");
    }

    #[test]
    fn external_git_delete_hides_worktree_from_status_summary() {
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

        let output = execute(repo_dir.path(), &db, None, false).expect("status should succeed");

        assert!(
            !output.contains("ephemeral"),
            "externally deleted worktree should not appear, got: {output}"
        );
    }
}
