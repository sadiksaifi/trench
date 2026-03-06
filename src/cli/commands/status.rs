use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;
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

pub fn execute(cwd: &Path, db: &Database, branch: Option<&str>) -> Result<String> {
    if branch.is_some() {
        todo!("deep status not yet implemented")
    }
    render_summary_table(
        cwd,
        db,
        crossterm::terminal::size().ok().map(|(c, _)| c as usize),
    )
}

pub fn execute_json(_cwd: &Path, _db: &Database, _branch: Option<&str>) -> Result<String> {
    todo!()
}

pub fn execute_porcelain(_cwd: &Path, _db: &Database, _branch: Option<&str>) -> Result<String> {
    todo!()
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
}
