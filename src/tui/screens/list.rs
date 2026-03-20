use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;
use crate::state::Database;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};

/// View model for a single worktree row in the TUI list.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeRow {
    pub name: String,
    pub branch: String,
    pub path: String,
    pub status: String,
    pub ahead_behind: String,
    pub managed: bool,
    /// Comma-separated process names running in this worktree.
    pub processes: String,
}

/// State for the worktree list screen.
pub struct ListState {
    pub rows: Vec<WorktreeRow>,
    pub selected: usize,
}

impl ListState {
    pub fn new(rows: Vec<WorktreeRow>) -> Self {
        Self { rows, selected: 0 }
    }

    pub fn select_next(&mut self) {
        if !self.rows.is_empty() && self.selected < self.rows.len() - 1 {
            self.selected += 1;
        }
    }

    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Restore selection from session state. Tries to find the worktree by name;
    /// if not found, falls back to `scroll_position` (clamped to bounds).
    /// Returns `true` if the worktree was found by name.
    pub fn restore_selection(&mut self, worktree_name: &str, scroll_position: usize) -> bool {
        if self.rows.is_empty() {
            self.selected = 0;
            return false;
        }
        if let Some(idx) = self.rows.iter().position(|r| r.name == worktree_name) {
            self.selected = idx;
            true
        } else if scroll_position < self.rows.len() {
            self.selected = scroll_position;
            false
        } else {
            self.selected = 0;
            false
        }
    }
}

/// Load worktree data from the database and git, returning rows for the list view.
///
/// Additional directories in `scan_paths` are scanned for worktrees that
/// may live outside the default location (FR-30).
pub fn load_worktrees(cwd: &Path, db: &Database, scan_paths: &[String]) -> Result<Vec<WorktreeRow>> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path = &repo_info.path;
    let repo_path_str = repo_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("repo path is not valid UTF-8"))?;

    let repo = db.get_repo_by_path(repo_path_str)?;
    let db_worktrees = match repo {
        Some(ref r) => db.list_worktrees(r.id)?,
        None => Vec::new(),
    };

    let managed_paths: HashSet<PathBuf> = db_worktrees
        .iter()
        .filter_map(|wt| Path::new(&wt.path).canonicalize().ok())
        .collect();

    let mut rows = Vec::new();

    for wt in &db_worktrees {
        let status = compute_status(repo_path, &wt.branch, wt.base_branch.as_deref(), &wt.path);
        let procs = crate::process::detect_processes(&wt.path);
        let processes = procs.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ");
        rows.push(WorktreeRow {
            name: wt.name.clone(),
            branch: wt.branch.clone(),
            path: wt.path.clone(),
            status: status.0,
            ahead_behind: status.1,
            managed: true,
            processes,
        });
    }

    let git_worktrees = git::list_worktrees(repo_path)?;
    for gw in &git_worktrees {
        if !managed_paths.contains(&gw.path) {
            let branch = gw.branch.clone().unwrap_or_else(|| "(detached)".to_string());
            let status = compute_status(
                repo_path,
                &branch,
                None,
                &gw.path.to_string_lossy(),
            );
            let wt_path_str = gw.path.to_string_lossy().to_string();
            let procs = crate::process::detect_processes(&wt_path_str);
            let processes = procs.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ");
            rows.push(WorktreeRow {
                name: gw.name.clone(),
                branch,
                path: wt_path_str,
                status: status.0,
                ahead_behind: status.1,
                managed: false,
                processes,
            });
        }
    }

    // Scan additional directories for worktrees (FR-30)
    if !scan_paths.is_empty() {
        let mut seen_paths: HashSet<PathBuf> = managed_paths;
        for gw in &git_worktrees {
            seen_paths.insert(gw.path.clone());
        }

        let scanned = git::scan_directories(scan_paths);
        for sw in scanned {
            if !seen_paths.contains(&sw.path) {
                seen_paths.insert(sw.path.clone());
                let branch = sw.branch.clone().unwrap_or_else(|| "(detached)".to_string());
                let wt_path_str = sw.path.to_string_lossy().to_string();
                let status = compute_status(
                    repo_path,
                    &branch,
                    None,
                    &wt_path_str,
                );
                let procs = crate::process::detect_processes(&wt_path_str);
                let processes = procs.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ");
                rows.push(WorktreeRow {
                    name: sw.name.clone(),
                    branch,
                    path: wt_path_str,
                    status: status.0,
                    ahead_behind: status.1,
                    managed: false,
                    processes,
                });
            }
        }
    }

    Ok(rows)
}

fn compute_status(
    repo_path: &Path,
    branch: &str,
    base_branch: Option<&str>,
    wt_path: &str,
) -> (String, String) {
    let dirty = git::dirty_count(Path::new(wt_path)).unwrap_or(0);
    let status = if dirty == 0 {
        "clean".to_string()
    } else {
        format!("~{dirty}")
    };

    let ab = match git::ahead_behind(repo_path, branch, base_branch) {
        Ok(Some((a, b))) => format!("+{a}/-{b}"),
        _ => "-".to_string(),
    };

    (status, ab)
}

const FOOTER_KEYS: &str = " Enter switch  d detail  n create  s sync  D delete  l log  q quit ";

pub fn render(state: &ListState, frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    let base_style = Style::default().fg(theme.foreground).bg(theme.background);
    let footer_style = Style::default()
        .fg(theme.background)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);

    if state.rows.is_empty() {
        let msg = Paragraph::new("No worktrees. Press n to create one.")
            .style(base_style)
            .alignment(ratatui::layout::Alignment::Center);
        let footer = Paragraph::new(Line::from(FOOTER_KEYS)).style(footer_style);
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
        frame.render_widget(msg, chunks[0]);
        frame.render_widget(footer, chunks[1]);
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    let header_cells = ["Name", "Branch", "Status", "Ahead/Behind", "Procs", ""]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = state
        .rows
        .iter()
        .map(|r| {
            let badge = if r.managed { "" } else { "[unmanaged]" };
            let style = if r.managed {
                base_style
            } else {
                Style::default().fg(theme.dimmed).bg(theme.background)
            };
            Row::new(vec![
                Cell::from(r.name.clone()),
                Cell::from(r.branch.clone()),
                Cell::from(r.status.clone()),
                Cell::from(r.ahead_behind.clone()),
                Cell::from(r.processes.clone()),
                Cell::from(badge),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(20),
        Constraint::Percentage(20),
        Constraint::Percentage(10),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .style(base_style)
        .block(Block::default().borders(Borders::NONE))
        .row_highlight_style(Style::default().bg(theme.accent).fg(theme.background));

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected));

    frame.render_stateful_widget(table, chunks[0], &mut table_state);

    let footer = Paragraph::new(Line::from(FOOTER_KEYS)).style(footer_style);
    frame.render_widget(footer, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn render_to_buffer(state: &ListState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name("catppuccin");
        terminal
            .draw(|frame| render(state, frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    fn sample_rows() -> Vec<WorktreeRow> {
        vec![
            WorktreeRow {
                name: "feature-auth".into(),
                branch: "feature/auth".into(),
                path: "/tmp/wt/feature-auth".into(),
                status: "clean".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
                processes: String::new(),
            },
            WorktreeRow {
                name: "fix-bug".into(),
                branch: "fix/bug".into(),
                path: "/tmp/wt/fix-bug".into(),
                status: "~3".into(),
                ahead_behind: "+0/-2".into(),
                managed: true,
                processes: String::new(),
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                path: "/tmp/wt/main".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: false,
                processes: String::new(),
            },
        ]
    }

    #[test]
    fn restore_selection_finds_worktree_by_name() {
        let mut state = ListState::new(sample_rows());
        // "fix-bug" is at index 1
        let found = state.restore_selection("fix-bug", 0);
        assert!(found, "should find worktree by name");
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn restore_selection_falls_back_to_scroll_position() {
        let mut state = ListState::new(sample_rows());
        // Name not found, but scroll position 2 is valid
        let found = state.restore_selection("nonexistent", 2);
        assert!(!found, "should not find nonexistent worktree");
        assert_eq!(state.selected, 2, "should fall back to scroll position");
    }

    #[test]
    fn restore_selection_clamps_scroll_position() {
        let mut state = ListState::new(sample_rows());
        // Name not found, scroll position 99 is out of bounds
        let found = state.restore_selection("nonexistent", 99);
        assert!(!found);
        assert_eq!(state.selected, 0, "should clamp to 0 when out of bounds");
    }

    #[test]
    fn restore_selection_on_empty_list() {
        let mut state = ListState::new(vec![]);
        let found = state.restore_selection("anything", 5);
        assert!(!found);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn list_state_starts_with_selection_at_zero() {
        let state = ListState::new(sample_rows());
        assert_eq!(state.selected, 0);
        assert_eq!(state.rows.len(), 3);
    }

    #[test]
    fn list_state_empty_rows() {
        let state = ListState::new(vec![]);
        assert_eq!(state.selected, 0);
        assert!(state.rows.is_empty());
    }

    #[test]
    fn select_next_advances_selection() {
        let mut state = ListState::new(sample_rows());
        assert_eq!(state.selected, 0);
        state.select_next();
        assert_eq!(state.selected, 1);
        state.select_next();
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn select_next_clamps_at_last_row() {
        let mut state = ListState::new(sample_rows());
        state.selected = 2;
        state.select_next();
        assert_eq!(state.selected, 2, "should not go past last row");
    }

    #[test]
    fn select_previous_moves_up() {
        let mut state = ListState::new(sample_rows());
        state.selected = 2;
        state.select_previous();
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn select_previous_clamps_at_zero() {
        let mut state = ListState::new(sample_rows());
        state.select_previous();
        assert_eq!(state.selected, 0, "should not go below 0");
    }

    #[test]
    fn select_next_on_empty_list_stays_at_zero() {
        let mut state = ListState::new(vec![]);
        state.select_next();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn renders_table_header_with_expected_columns() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(text.contains("Name"), "header should contain Name");
        assert!(text.contains("Branch"), "header should contain Branch");
        assert!(text.contains("Status"), "header should contain Status");
        assert!(
            text.contains("Ahead/Behind"),
            "header should contain Ahead/Behind"
        );
    }

    #[test]
    fn renders_worktree_data_in_rows() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(
            text.contains("feature-auth"),
            "should show worktree name, got: {text}"
        );
        assert!(
            text.contains("feature/auth"),
            "should show branch, got: {text}"
        );
        assert!(text.contains("clean"), "should show clean status");
        assert!(text.contains("+1/-0"), "should show ahead/behind");
    }

    #[test]
    fn selected_row_has_highlight_style() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        // Row 0 is selected by default — its first cell should have theme accent bg
        // The header is row 0 in the buffer, data starts at row 1
        let cell = buf.cell((0, 1)).unwrap();
        assert_eq!(
            cell.bg, theme.accent,
            "selected row should have theme.accent background, got: {:?}",
            cell.bg
        );
    }

    #[test]
    fn unmanaged_worktree_shows_badge() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(
            text.contains("[unmanaged]"),
            "unmanaged row should show badge"
        );
    }

    #[test]
    fn unmanaged_row_has_dimmed_style() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        // The unmanaged row is index 2 (third row), rendered at buffer row 3 (header=0, row0=1, row1=2, row2=3)
        let cell = buf.cell((0, 3)).unwrap();
        assert_eq!(
            cell.fg, theme.dimmed,
            "unmanaged row should use theme.dimmed color, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn footer_shows_keybindings() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(text.contains("Enter switch"), "footer should show Enter switch");
        assert!(text.contains("d detail"), "footer should show d detail");
        assert!(text.contains("n create"), "footer should show n create");
        assert!(text.contains("s sync"), "footer should show s sync");
        assert!(text.contains("D delete"), "footer should show D delete");
        assert!(text.contains("l log"), "footer should show l log");
        assert!(text.contains("q quit"), "footer should show q quit");
    }

    #[test]
    fn empty_state_shows_message() {
        let state = ListState::new(vec![]);
        let buf = render_to_buffer(&state, 80, 5);
        let text = buffer_text(&buf);
        assert!(
            text.contains("No worktrees"),
            "empty state should show message, got: {text}"
        );
    }

    #[test]
    fn empty_state_still_shows_footer() {
        let state = ListState::new(vec![]);
        let buf = render_to_buffer(&state, 80, 5);
        let text = buffer_text(&buf);
        assert!(
            text.contains("n create"),
            "empty state should still show footer keybindings"
        );
    }

    #[test]
    fn empty_state_uses_theme_foreground() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let state = ListState::new(vec![]);
        let buf = render_to_buffer(&state, 80, 5);
        // Find a cell in the "No worktrees" message area (row 0, skip leading spaces)
        let text = buffer_text(&buf);
        let offset = text.find('N').expect("should find 'N' from 'No worktrees'");
        let width = 80usize;
        let x = (offset % width) as u16;
        let y = (offset / width) as u16;
        let cell = buf.cell((x, y)).unwrap();
        assert_eq!(
            cell.fg, theme.foreground,
            "empty state text should use theme.foreground, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn renders_process_info_in_table() {
        let rows = vec![
            WorktreeRow {
                name: "feature-auth".into(),
                branch: "feature/auth".into(),
                path: "/tmp/wt/feature-auth".into(),
                status: "clean".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
                processes: "node, vite".into(),
            },
            WorktreeRow {
                name: "fix-bug".into(),
                branch: "fix/bug".into(),
                path: "/tmp/wt/fix-bug".into(),
                status: "~3".into(),
                ahead_behind: "+0/-2".into(),
                managed: true,
                processes: String::new(),
            },
        ];
        let state = ListState::new(rows);
        let buf = render_to_buffer(&state, 120, 10);
        let text = buffer_text(&buf);

        assert!(
            text.contains("node, vite"),
            "should show process names, got: {text}"
        );
    }

    #[test]
    fn managed_row_uses_theme_foreground() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let mut state = ListState::new(sample_rows());
        // Select row 1 so row 0 (managed) is NOT highlighted
        state.selected = 1;
        let buf = render_to_buffer(&state, 100, 10);
        // Row 0 is at buffer row 1 (header at row 0)
        let cell = buf.cell((0, 1)).unwrap();
        assert_eq!(
            cell.fg, theme.foreground,
            "managed row should use theme.foreground, got: {:?}",
            cell.fg
        );
    }
}
