use std::path::Path;

use anyhow::Result;

use crate::output::human::Palette;
use crate::output::json::format_json;
use crate::output::porcelain::{format_porcelain, PorcelainRecord};
use crate::output::text;
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

/// How the human table adapts to where it is printed.
#[derive(Debug, Clone, Copy)]
pub struct HumanLayout<'a> {
    /// Columns of the terminal, or `None` when stdout is not a terminal.
    pub terminal_width: Option<usize>,
    /// Home directory to write as `~`, or `None` to print absolute paths.
    pub home: Option<&'a Path>,
    pub palette: Palette,
}

impl HumanLayout<'_> {
    /// Piped output: absolute paths, no color, never reshaped.
    pub const PIPED: Self = Self {
        terminal_width: None,
        home: None,
        palette: Palette::PLAIN,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanList {
    pub text: String,
    pub worktrees: usize,
}

pub fn execute(cwd: &Path, layout: HumanLayout<'_>) -> Result<HumanList> {
    let catalog = WorktreeCatalog::discover(cwd)?;
    let rows: Vec<HumanRow> = catalog
        .identities()
        .iter()
        .map(|identity| HumanRow {
            current: identity.is_current,
            // A detached worktree has no branch; its catalog name
            // (`detached@<commit>`) is the selector other commands accept.
            branch: identity
                .branch
                .clone()
                .unwrap_or_else(|| identity.worktree.clone()),
            path: text::home_relative(&identity.path, layout.home),
            missing: identity.missing,
        })
        .collect();
    Ok(HumanList {
        text: render_rows(&rows, layout),
        worktrees: rows.len(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HumanRow {
    current: bool,
    branch: String,
    path: String,
    missing: bool,
}

const MISSING_TAG: &str = "  (missing)";

fn render_rows(rows: &[HumanRow], layout: HumanLayout<'_>) -> String {
    let branch_width = rows
        .iter()
        .map(|row| text::width(&row.branch))
        .chain([text::width("BRANCH")])
        .max()
        .unwrap_or(0);
    let widest = rows
        .iter()
        .map(|row| {
            2 + branch_width
                + 2
                + text::width(&row.path)
                + if row.missing {
                    text::width(MISSING_TAG)
                } else {
                    0
                }
        })
        .max()
        .unwrap_or(0);
    // Values are never truncated: a cut path cannot be copied. When the table
    // does not fit the terminal, each worktree gets two lines instead.
    let stacked = layout.terminal_width.is_some_and(|width| widest > width);
    let palette = layout.palette;

    let mut out = String::new();
    if !stacked {
        out.push_str(&palette.dim(&format!(
            "  {}  PATH",
            text::pad_end("BRANCH", branch_width)
        )));
        out.push('\n');
    }
    for row in rows {
        let marker = if row.current {
            palette.success("*")
        } else {
            " ".to_string()
        };
        let missing = if row.missing {
            palette.warning(MISSING_TAG)
        } else {
            String::new()
        };
        if stacked {
            out.push_str(&format!(
                "{marker} {}\n    {}{missing}\n",
                row.branch, row.path
            ));
        } else {
            out.push_str(&format!(
                "{marker} {}  {}{missing}\n",
                text::pad_end(&row.branch, branch_width),
                row.path
            ));
        }
    }
    out
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

    fn row(current: bool, branch: &str, path: &str) -> HumanRow {
        HumanRow {
            current,
            branch: branch.to_string(),
            path: path.to_string(),
            missing: false,
        }
    }

    #[test]
    fn table_aligns_branch_and_path_behind_a_fixed_current_marker() {
        let rows = [
            row(true, "main", "/code/acme"),
            row(false, "feature/auth", "/wt/acme/feature-auth"),
            HumanRow {
                missing: true,
                ..row(false, "stale/gone", "/wt/acme/stale-gone")
            },
        ];

        assert_eq!(
            render_rows(&rows, HumanLayout::PIPED),
            "  BRANCH        PATH\n\
             * main          /code/acme\n\
             \x20 feature/auth  /wt/acme/feature-auth\n\
             \x20 stale/gone    /wt/acme/stale-gone  (missing)\n"
        );
    }

    #[test]
    fn columns_align_by_display_width_for_wide_branch_names() {
        let rows = [row(true, "日本語", "/a"), row(false, "main", "/b")];

        assert_eq!(
            render_rows(&rows, HumanLayout::PIPED),
            "  BRANCH  PATH\n* 日本語  /a\n  main    /b\n"
        );
    }

    #[test]
    fn piped_output_is_never_reshaped_or_truncated() {
        let long = "feature/".to_string() + &"very-long-".repeat(20);
        let rows = [row(true, &long, "/a/long/path")];

        let rendered = render_rows(&rows, HumanLayout::PIPED);

        assert!(rendered.contains(&format!("* {long}  /a/long/path\n")));
        assert!(!rendered.contains('…'));
    }

    #[test]
    fn a_table_wider_than_the_terminal_stacks_each_worktree_on_two_lines() {
        let rows = [
            row(true, "main", "/code/acme"),
            HumanRow {
                missing: true,
                ..row(
                    false,
                    "feature/JIRA-12345-implement-the-thing",
                    "/wt/acme/feature-JIRA-12345-implement-the-thing",
                )
            },
        ];
        let layout = HumanLayout {
            terminal_width: Some(60),
            ..HumanLayout::PIPED
        };

        assert_eq!(
            render_rows(&rows, layout),
            "* main\n\
             \x20   /code/acme\n\
             \x20 feature/JIRA-12345-implement-the-thing\n\
             \x20   /wt/acme/feature-JIRA-12345-implement-the-thing  (missing)\n"
        );
        let fits = HumanLayout {
            terminal_width: Some(200),
            ..HumanLayout::PIPED
        };
        assert!(render_rows(&rows, fits).starts_with("  BRANCH"));
    }

    #[test]
    fn human_list_names_detached_worktrees_by_their_selector_and_abbreviates_home() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let commit = {
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .unwrap()
        };
        repo.set_head_detached(commit).unwrap();
        let home = root.path().canonicalize().unwrap();
        let home = home.parent().unwrap();

        let list = execute(
            root.path(),
            HumanLayout {
                home: Some(home),
                ..HumanLayout::PIPED
            },
        )
        .unwrap();

        assert_eq!(list.worktrees, 1);
        let short: String = commit.to_string().chars().take(7).collect();
        let directory = root.path().file_name().unwrap().to_string_lossy();
        assert_eq!(
            list.text.lines().nth(1).unwrap(),
            format!("* detached@{short}  ~/{directory}")
        );
        assert!(!list.text.ends_with("\n\n"));
    }
}
