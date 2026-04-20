use std::path::Path;

use anyhow::Result;

use crate::git;
use crate::state::Database;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, TableState},
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
    pub is_current: bool,
    /// Comma-separated process names running in this worktree.
    pub processes: String,
}

/// A transient status message displayed in the list view footer area.
/// Auto-cleared on the next keypress.
pub struct StatusMessage {
    pub text: String,
    pub success: bool,
}

/// State for the worktree list screen.
pub struct ListState {
    pub rows: Vec<WorktreeRow>,
    pub selected: usize,
    pub status_message: Option<StatusMessage>,
}

impl ListState {
    pub fn new(rows: Vec<WorktreeRow>) -> Self {
        Self {
            rows,
            selected: 0,
            status_message: None,
        }
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
pub fn load_worktrees(
    cwd: &Path,
    db: &Database,
    scan_paths: &[String],
) -> Result<Vec<WorktreeRow>> {
    let repo_info = git::discover_repo(cwd)?;
    let repo_path = &repo_info.path;
    let current_path = git::current_worktree_root(cwd)
        .ok()
        .map(|path| path.to_string_lossy().to_string());
    let live_worktrees = crate::live_worktree::list(&repo_info, db, scan_paths)?;

    let mut rows = Vec::new();

    for worktree in live_worktrees {
        let branch = worktree
            .entry
            .branch
            .clone()
            .unwrap_or_else(|| "(detached)".to_string());
        let path = worktree.entry.path.to_string_lossy().to_string();
        let base_branch = Some(crate::live_worktree::base_branch(&repo_info, &worktree));
        let status = compute_status(repo_path, &branch, base_branch.as_deref(), &path);
        let procs = crate::process::detect_processes(&path);
        let processes = procs
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(WorktreeRow {
            name: worktree.entry.name.clone(),
            branch,
            path,
            status: status.0,
            ahead_behind: status.1,
            managed: true,
            is_current: current_path
                .as_deref()
                .is_some_and(|path| path == rowsafe_path(&worktree.entry.path)),
            processes,
        });
    }

    Ok(rows)
}

fn rowsafe_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
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

const KEYBAR_ITEMS: [(&str, &str); 7] = [
    ("Enter", "switch"),
    ("d", "detail"),
    ("n", "create"),
    ("s", "sync"),
    ("D", "delete"),
    ("l", "log"),
    ("q", "quit"),
];

pub fn render(state: &ListState, frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    let base_style = Style::default().fg(theme.fg).bg(theme.bg);
    let footer_style = Style::default()
        .fg(theme.selection_fg)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);

    if state.rows.is_empty() {
        let msg = Paragraph::new("No worktrees. Press n to create one.")
            .style(base_style)
            .alignment(ratatui::layout::Alignment::Center);
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
        frame.render_widget(msg, chunks[0]);
        render_legacy_footer(state, frame, chunks[1], theme, &footer_style);
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let header_cells = ["Name", "Now", "Branch", "Status", "Ahead/Behind", "Procs"]
        .iter()
        .map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
        });
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = state
        .rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(r.name.clone()),
                Cell::from(if r.is_current { "*" } else { "" }),
                Cell::from(r.branch.clone()),
                Cell::from(r.status.clone()),
                Cell::from(r.ahead_behind.clone()),
                Cell::from(r.processes.clone()),
            ])
            .style(base_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(18),
            Constraint::Length(3),
            Constraint::Percentage(24),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
            Constraint::Percentage(25),
        ],
    )
    .header(header)
    .style(base_style)
    .row_highlight_style(Style::default().bg(theme.accent).fg(theme.selection_fg));

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected));
    frame.render_stateful_widget(table, chunks[0], &mut table_state);
    render_legacy_footer(state, frame, chunks[1], theme, &footer_style);
}

pub fn render_with_options(
    state: &ListState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
    options: &crate::tui::chrome::UiOptions,
) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);

    render_summary_bar(state, frame, chunks[0], theme);

    if state.rows.is_empty() {
        crate::tui::chrome::render_empty_state(
            frame,
            chunks[1],
            theme,
            "Worktrees",
            "No worktrees. Press n to create one.",
        );
        render_footer(state, frame, chunks[2], theme);
        return;
    }

    let body_chunks = if chunks[1].width >= 120 {
        Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(chunks[1])
    } else {
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(chunks[1])
    };

    render_table(state, frame, body_chunks[0], theme, options);
    render_inspector(state, frame, body_chunks[1], theme, options);
    render_footer(state, frame, chunks[2], theme);
}

fn render_summary_bar(
    state: &ListState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let line = Line::from(vec![
        Span::styled(
            "Worktree Cockpit",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        crate::tui::chrome::pill(
            theme,
            &format!("{} total", state.rows.len()),
            crate::tui::chrome::Tone::Muted,
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(theme.fg).bg(theme.bg)),
        area,
    );
}

fn render_table(
    state: &ListState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
    options: &crate::tui::chrome::UiOptions,
) {
    let mut titles = vec![
        Cell::from("Name"),
        Cell::from("Now"),
        Cell::from("Branch"),
        Cell::from("Status"),
    ];
    if options.show_ahead_behind {
        titles.push(Cell::from("Ahead/Behind"));
    }
    titles.push(Cell::from("Procs"));

    let header = Row::new(titles.into_iter().map(|cell| {
        cell.style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    }));

    let rows: Vec<Row> = state
        .rows
        .iter()
        .map(|row| {
            let mut cells = vec![
                Cell::from(row.name.clone()),
                Cell::from(if row.is_current { "*" } else { "" }),
                Cell::from(row.branch.clone()),
                Cell::from(display_status(&row.status, options.show_dirty_count)),
            ];
            if options.show_ahead_behind {
                cells.push(Cell::from(row.ahead_behind.clone()));
            }
            cells.push(Cell::from(if row.processes.is_empty() {
                "idle".to_string()
            } else {
                row.processes.clone()
            }));

            Row::new(cells).style(Style::default().fg(theme.fg).bg(theme.bg_panel))
        })
        .collect();

    let widths = if options.show_ahead_behind {
        vec![
            Constraint::Percentage(18),
            Constraint::Length(3),
            Constraint::Percentage(24),
            Constraint::Percentage(13),
            Constraint::Percentage(16),
            Constraint::Percentage(17),
            Constraint::Percentage(12),
        ]
    } else {
        vec![
            Constraint::Percentage(20),
            Constraint::Length(3),
            Constraint::Percentage(28),
            Constraint::Percentage(15),
            Constraint::Percentage(25),
            Constraint::Percentage(12),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(crate::tui::chrome::panel(" Worktrees ", theme))
        .row_highlight_style(
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.selection_bg)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().fg(theme.fg).bg(theme.bg_panel));

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_inspector(
    state: &ListState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
    options: &crate::tui::chrome::UiOptions,
) {
    let Some(selected) = state.rows.get(state.selected) else {
        return;
    };
    let block = crate::tui::chrome::panel(" Focus ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let process_text = if selected.processes.is_empty() {
        "idle".to_string()
    } else {
        selected.processes.clone()
    };
    let mut lines = vec![
        Line::from(Span::styled(
            selected.name.clone(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metric_line("Branch", &selected.branch, theme),
        metric_line("Path", &selected.path, theme),
        metric_line(
            "Status",
            &display_status(&selected.status, options.show_dirty_count),
            theme,
        ),
    ];
    if options.show_ahead_behind {
        lines.push(metric_line("Ahead/Behind", &selected.ahead_behind, theme));
    }
    lines.push(metric_line("Processes", &process_text, theme));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![crate::tui::chrome::pill(
        theme,
        &display_status(&selected.status, options.show_dirty_count),
        status_tone(&selected.status),
    )]));
    if selected.is_current {
        lines.push(Line::from(vec![crate::tui::chrome::pill(
            theme,
            "current",
            crate::tui::chrome::Tone::Accent,
        )]));
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.fg).bg(theme.bg_panel)),
        inner,
    );
}

fn metric_line(label: &str, value: &str, theme: &crate::tui::theme::Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<12}"),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

fn display_status(raw: &str, show_dirty_count: bool) -> String {
    if show_dirty_count || raw == "clean" {
        raw.to_string()
    } else {
        "dirty".to_string()
    }
}

fn status_tone(status: &str) -> crate::tui::chrome::Tone {
    if status == "clean" {
        crate::tui::chrome::Tone::Success
    } else {
        crate::tui::chrome::Tone::Warning
    }
}

fn render_footer(
    state: &ListState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
) {
    if let Some(ref status) = state.status_message {
        let tone = if status.success {
            crate::tui::chrome::Tone::Success
        } else {
            crate::tui::chrome::Tone::Error
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                crate::tui::chrome::pill(theme, "status", tone),
                Span::raw(format!(" {}", status.text)),
            ]))
            .style(Style::default().fg(theme.fg).bg(theme.bg_elevated)),
            area,
        );
    } else {
        crate::tui::chrome::render_keybar(frame, area, theme, &KEYBAR_ITEMS);
    }
}

fn render_legacy_footer(
    state: &ListState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
    footer_style: &Style,
) {
    if let Some(ref status) = state.status_message {
        let color = if status.success {
            theme.success
        } else {
            theme.error
        };
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {}", status.text))).style(
                Style::default()
                    .fg(color)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD),
            ),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(
                " Enter switch  d detail  n create  s sync  D delete  l log  q quit ",
            ))
            .style(*footer_style),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::path::Path;

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
                is_current: true,
                processes: String::new(),
            },
            WorktreeRow {
                name: "fix-bug".into(),
                branch: "fix/bug".into(),
                path: "/tmp/wt/fix-bug".into(),
                status: "~3".into(),
                ahead_behind: "+0/-2".into(),
                managed: true,
                is_current: false,
                processes: String::new(),
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                path: "/tmp/wt/main".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: false,
                is_current: false,
                processes: String::new(),
            },
        ]
    }

    fn init_repo_with_commit(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).expect("failed to init repo");
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();
        drop(tree);
        repo
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
        assert!(text.contains("Now"), "header should contain Now");
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
            text.contains("*"),
            "should show current marker, got: {text}"
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
    fn worktree_rows_do_not_show_unmanaged_badge() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(!text.contains("[unmanaged]"));
    }

    #[test]
    fn non_selected_row_uses_theme_foreground() {
        let theme = crate::tui::theme::from_name("catppuccin");
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        // Third row is not highlighted and should use the normal foreground.
        let cell = buf.cell((0, 3)).unwrap();
        assert_eq!(
            cell.fg, theme.fg,
            "non-selected row should use theme.fg color, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn footer_shows_keybindings() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Enter switch"),
            "footer should show Enter switch"
        );
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
            cell.fg, theme.fg,
            "empty state text should use theme.fg, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn load_worktrees_hides_externally_deleted_worktree() {
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

        let rows = load_worktrees(repo_dir.path(), &db, &[]).expect("load should succeed");

        assert!(
            rows.iter().all(|row| row.name != "ephemeral"),
            "externally deleted worktree should not appear: {rows:?}"
        );
    }

    #[test]
    fn load_worktrees_marks_current_checkout() {
        use crate::cli::commands::create;
        use crate::paths;

        let repo_dir = tempfile::tempdir().unwrap();
        let _repo = init_repo_with_commit(repo_dir.path());
        let wt_root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();

        let created = create::execute(
            "focus-me",
            None,
            repo_dir.path(),
            wt_root.path(),
            paths::DEFAULT_WORKTREE_TEMPLATE,
            &db,
        )
        .expect("create should succeed");

        let rows = load_worktrees(&created.path, &db, &[]).expect("load should succeed");
        let current = rows
            .iter()
            .find(|row| row.name == "focus-me")
            .expect("linked worktree should be listed");
        assert!(current.is_current, "current checkout should be marked");
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
                is_current: true,
                processes: "node, vite".into(),
            },
            WorktreeRow {
                name: "fix-bug".into(),
                branch: "fix/bug".into(),
                path: "/tmp/wt/fix-bug".into(),
                status: "~3".into(),
                ahead_behind: "+0/-2".into(),
                managed: true,
                is_current: false,
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
            cell.fg, theme.fg,
            "managed row should use theme.fg, got: {:?}",
            cell.fg
        );
    }

    #[test]
    fn list_state_has_status_message_initially_none() {
        let state = ListState::new(vec![]);
        assert!(state.status_message.is_none());
    }

    #[test]
    fn status_message_replaces_footer_when_set() {
        let mut state = ListState::new(sample_rows());
        state.status_message = Some(StatusMessage {
            text: "Switch failed: worktree not found".into(),
            success: false,
        });
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Switch failed"),
            "should show status message text"
        );
        assert!(
            !text.contains("Enter switch"),
            "should not show footer keys when status is active"
        );
    }

    #[test]
    fn footer_shows_keys_when_no_status_message() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Enter switch"),
            "should show footer keys when no status"
        );
    }

    #[test]
    fn inspector_shows_current_badge_for_active_worktree() {
        let backend = TestBackend::new(140, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name("catppuccin");
        let state = ListState::new(sample_rows());
        let options = crate::tui::chrome::UiOptions::default();
        terminal
            .draw(|frame| render_with_options(&state, frame, frame.area(), &theme, &options))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("current"),
            "inspector should show current badge"
        );
    }
}
