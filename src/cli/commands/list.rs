use std::path::Path;

use anyhow::Result;

use crate::output::json::format_json;
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::table::Table;
use crate::worktree_catalog::{WorktreeCatalog, WorktreeRecord};

impl PorcelainRecord for WorktreeRecord {
    fn porcelain_fields(&self) -> Vec<String> {
        vec![
            self.worktree.clone(),
            self.branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string()),
            self.path.clone(),
        ]
    }
}

fn records(cwd: &Path) -> Result<Vec<WorktreeRecord>> {
    Ok(WorktreeCatalog::discover(cwd)?.records())
}

pub fn execute(cwd: &Path) -> Result<String> {
    let max_width = crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| columns as usize);
    render_human(cwd, max_width)
}

fn render_human(cwd: &Path, max_width: Option<usize>) -> Result<String> {
    let records = records(cwd)?;
    let mut table = Table::new(vec!["Worktree", "Branch", "Path"]);
    for record in records {
        let branch = record.branch.as_deref().unwrap_or("(detached)");
        let worktree = if record.is_current {
            format!("* {}", record.worktree)
        } else {
            record.worktree
        };
        table = table.row(vec![&worktree, branch, &record.path]);
    }
    if let Some(width) = max_width {
        table = table.max_width(width);
    }
    Ok(format!("{}\n", table.render()))
}

pub fn execute_json(cwd: &Path) -> Result<String> {
    format_json(&records(cwd)?)
}

pub fn execute_porcelain(cwd: &Path) -> Result<String> {
    Ok(format_porcelain(&records(cwd)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_output_shows_only_worktree_branch_and_path() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .unwrap();
        }

        let output = render_human(root.path(), None).unwrap();

        let header = output.lines().next().unwrap();
        assert!(header.contains("Worktree"), "{output}");
        assert!(header.contains("Branch"), "{output}");
        assert!(!header.contains("Git"), "{output}");
        assert!(!header.contains("Procs"), "{output}");
        assert!(!header.contains("Tags"), "{output}");
        assert!(header.contains("Path"), "{output}");
    }
}
