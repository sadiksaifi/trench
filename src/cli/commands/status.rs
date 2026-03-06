use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::git;
use crate::output::json::{format_json, format_json_value};
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::table::Table;
use crate::state::{Database, Worktree};

/// A unified worktree entry for status output.
struct StatusEntry {
    name: String,
    branch: String,
    path: String,
    base_branch: Option<String>,
    managed: bool,
}

/// Fetch all worktrees (managed + unmanaged) for the repo at `cwd`.
fn fetch_all_worktrees(cwd: &Path, db: &Database) -> Result<(PathBuf, Vec<StatusEntry>)> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db.get_repo_by_path(repo_path_str)?;
    let db_worktrees: Vec<Worktree> = match repo {
        Some(ref r) => db.list_worktrees(r.id)?,
        None => Vec::new(),
    };

    let managed_paths: HashSet<PathBuf> = db_worktrees
        .iter()
        .filter_map(|wt| Path::new(&wt.path).canonicalize().ok())
        .collect();

    let mut entries: Vec<StatusEntry> = Vec::new();

    for wt in &db_worktrees {
        entries.push(StatusEntry {
            name: wt.name.clone(),
            branch: wt.branch.clone(),
            path: wt.path.clone(),
            base_branch: wt.base_branch.clone(),
            managed: true,
        });
    }

    let git_worktrees = git::list_worktrees(&repo_info.path)?;
    for gw in git_worktrees {
        if !managed_paths.contains(&gw.path) {
            entries.push(StatusEntry {
                name: gw.name.clone(),
                branch: gw.branch.unwrap_or_else(|| "(detached)".to_string()),
                path: gw.path.to_string_lossy().into_owned(),
                base_branch: None,
                managed: false,
            });
        }
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
) -> Result<String> {
    let (repo_path, entries) = fetch_all_worktrees(cwd, db)?;

    if entries.is_empty() {
        return Ok("No worktrees.\n".to_string());
    }

    let mut table = Table::new(vec![
        "Name", "Branch", "Status", "Ahead/Behind",
    ]);
    let mut unmanaged_rows: Vec<bool> = Vec::new();

    for entry in &entries {
        let status = compute_git_status(&repo_path, entry);
        let dirty_str = format_dirty(status.dirty);
        let ab_str = format_ahead_behind(status.ahead, status.behind);
        let display_name = if entry.managed {
            entry.name.clone()
        } else {
            format!("{} [unmanaged]", entry.name)
        };
        table = table.row(vec![&display_name, &entry.branch, &dirty_str, &ab_str]);
        unmanaged_rows.push(!entry.managed);
    }

    if let Some(width) = max_width {
        table = table.max_width(width);
    }

    let rendered = table.render();

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

/// Resolve a worktree by identifier (sanitized name or branch) from the DB.
/// Falls back to git-discovered worktrees for unmanaged entries.
fn resolve_worktree(
    cwd: &Path,
    db: &Database,
    identifier: &str,
) -> Result<(PathBuf, StatusEntry)> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path_str = repo_info
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    // Try DB first
    if let Some(repo) = db.get_repo_by_path(repo_path_str)? {
        if let Some(wt) = db.find_worktree_by_identifier(repo.id, identifier)? {
            return Ok((
                repo_info.path,
                StatusEntry {
                    name: wt.name,
                    branch: wt.branch,
                    path: wt.path,
                    base_branch: wt.base_branch,
                    managed: true,
                },
            ));
        }
    }

    // Fall back to git-discovered worktrees
    let git_worktrees = git::list_worktrees(&repo_info.path)?;
    for gw in git_worktrees {
        let branch_match = gw.branch.as_deref() == Some(identifier);
        let name_match = gw.name == identifier;
        if branch_match || name_match {
            return Ok((
                repo_info.path,
                StatusEntry {
                    name: gw.name,
                    branch: gw.branch.unwrap_or_else(|| "(detached)".to_string()),
                    path: gw.path.to_string_lossy().into_owned(),
                    base_branch: None,
                    managed: false,
                },
            ));
        }
    }

    anyhow::bail!("worktree not found: {identifier}")
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
    if !entry.managed {
        out.push_str("Managed:      no [unmanaged]\n");
    }

    Ok(out)
}

pub fn execute(cwd: &Path, db: &Database, branch: Option<&str>) -> Result<String> {
    match branch {
        Some(id) => render_deep(cwd, db, id),
        None => render_summary_table(
            cwd,
            db,
            crossterm::terminal::size().ok().map(|(c, _)| c as usize),
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
    managed: bool,
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
            self.managed.to_string(),
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
        managed: entry.managed,
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
    managed: bool,
    changed_files: Vec<String>,
    recent_commits: Vec<String>,
    hook_history: Vec<String>,
}

fn build_deep_json(entry: &StatusEntry, status: GitStatus) -> DeepJson {
    DeepJson {
        name: entry.name.clone(),
        branch: entry.branch.clone(),
        path: entry.path.clone(),
        base_branch: entry.base_branch.clone(),
        ahead: status.ahead,
        behind: status.behind,
        dirty: status.dirty,
        status: format_dirty(status.dirty),
        managed: entry.managed,
        changed_files: Vec::new(),
        recent_commits: Vec::new(),
        hook_history: Vec::new(),
    }
}

pub fn execute_json(cwd: &Path, db: &Database, branch: Option<&str>) -> Result<String> {
    match branch {
        Some(id) => {
            let (repo_path, entry) = resolve_worktree(cwd, db, id)?;
            let status = compute_git_status(&repo_path, &entry);
            let json_obj = build_deep_json(&entry, status);
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

    #[test]
    fn summary_shows_all_worktrees() {
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
            "/tmp/wt/feature-auth",
            Some("main"),
        )
        .unwrap();
        db.insert_worktree(
            db_repo.id,
            "fix-bug",
            "fix/bug",
            "/tmp/wt/fix-bug",
            Some("main"),
        )
        .unwrap();

        let output =
            render_summary_table(repo_dir.path(), &db, None).expect("summary should succeed");

        assert!(output.contains("Name"), "should have Name header");
        assert!(output.contains("Branch"), "should have Branch header");
        assert!(output.contains("feature-auth"), "should show first worktree");
        assert!(output.contains("fix-bug"), "should show second worktree");
    }

    #[test]
    fn summary_json_returns_array_of_worktrees() {
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
            "/tmp/wt/feature-auth",
            Some("main"),
        )
        .unwrap();

        let output =
            execute_json(repo_dir.path(), &db, None).expect("summary json should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let arr = parsed.as_array().expect("should be array");

        // At least the managed worktree + main worktree
        assert!(arr.len() >= 2, "should have at least 2 entries, got {}", arr.len());

        // Find the managed worktree
        let wt = arr
            .iter()
            .find(|v| v["name"] == "feature-auth")
            .expect("should contain feature-auth");
        assert_eq!(wt["branch"], "feature/auth");
        assert_eq!(wt["managed"], true);
        assert!(wt["path"].is_string());
    }

    #[test]
    fn deep_json_returns_single_object() {
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
            "/tmp/wt/feature-auth",
            Some("main"),
        )
        .unwrap();

        let output = execute_json(repo_dir.path(), &db, Some("feature-auth"))
            .expect("deep json should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert!(parsed.is_object(), "should be a single JSON object");
        assert_eq!(parsed["name"], "feature-auth");
        assert_eq!(parsed["branch"], "feature/auth");
        assert_eq!(parsed["base_branch"], "main");
        assert_eq!(parsed["managed"], true);
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
    fn deep_mode_shows_detail_for_managed_worktree() {
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
            "/tmp/wt/feature-auth",
            Some("main"),
        )
        .unwrap();

        let output =
            render_deep(repo_dir.path(), &db, "feature-auth").expect("deep should succeed");

        assert!(output.contains("Branch:"), "should show Branch label");
        assert!(output.contains("feature/auth"), "should show branch name");
        assert!(output.contains("Path:"), "should show Path label");
        assert!(output.contains("/tmp/wt/feature-auth"), "should show path");
        assert!(output.contains("Base:"), "should show Base label");
        assert!(output.contains("main"), "should show base branch");
    }
}
