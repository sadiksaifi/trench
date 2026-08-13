use std::path::Path;

use anyhow::Result;

use crate::output::json::format_json;
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::table::Table;
use crate::worktree_catalog::{WorktreeCatalog, WorktreeRecord};

impl PorcelainRecord for WorktreeRecord {
    fn porcelain_fields(&self) -> Vec<String> {
        let dirty = self.staged + self.modified + self.untracked;
        vec![
            self.worktree.clone(),
            self.branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string()),
            self.path.clone(),
            if dirty == 0 {
                "clean".to_string()
            } else {
                format!("~{dirty}")
            },
            self.ahead
                .map_or_else(|| "-".to_string(), |n| n.to_string()),
            self.behind
                .map_or_else(|| "-".to_string(), |n| n.to_string()),
            dirty.to_string(),
        ]
    }
}

fn records(cwd: &Path, default_base: Option<&str>) -> Result<Vec<WorktreeRecord>> {
    Ok(WorktreeCatalog::discover(cwd)?
        .with_base(default_base)
        .records()?)
}

pub fn execute(cwd: &Path, default_base: Option<&str>) -> Result<String> {
    let records = records(cwd, default_base)?;
    let max_width = crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| columns as usize);
    let mut table = Table::new(vec!["Worktree", "Branch", "Path", "Git"]);
    for record in records {
        let branch = record.branch.as_deref().unwrap_or("(detached)");
        let dirty = record.staged + record.modified + record.untracked;
        let git = match (dirty, record.ahead, record.behind) {
            (0, None, None) => "clean".to_string(),
            (0, Some(ahead), Some(behind)) => format!("clean · ↑{ahead} ↓{behind}"),
            (changed, None, None) => format!("{changed} changed"),
            (changed, Some(ahead), Some(behind)) => {
                format!("{changed} changed · ↑{ahead} ↓{behind}")
            }
            (changed, _, _) => format!("{changed} changed"),
        };
        let worktree = if record.is_current {
            format!("* {}", record.worktree)
        } else {
            record.worktree
        };
        table = table.row(vec![&worktree, branch, &record.path, &git]);
    }
    if let Some(width) = max_width {
        table = table.max_width(width);
    }
    Ok(format!("{}\n", table.render()))
}

pub fn execute_json(cwd: &Path, default_base: Option<&str>) -> Result<String> {
    format_json(&records(cwd, default_base)?)
}

pub fn execute_porcelain(cwd: &Path, default_base: Option<&str>) -> Result<String> {
    Ok(format_porcelain(&records(cwd, default_base)?))
}
