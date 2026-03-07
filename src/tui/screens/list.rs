use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};

/// View model for a single worktree row in the TUI list.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeRow {
    pub name: String,
    pub branch: String,
    pub status: String,
    pub ahead_behind: String,
    pub managed: bool,
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
}

const FOOTER_KEYS: &str = " n create  s sync  D delete  Enter detail  q quit ";

pub fn render(state: &ListState, frame: &mut Frame, area: Rect) {
    if state.rows.is_empty() {
        let msg = Paragraph::new("No worktrees. Press n to create one.")
            .alignment(ratatui::layout::Alignment::Center);
        let footer = Paragraph::new(Line::from(FOOTER_KEYS))
            .style(Style::default().add_modifier(Modifier::REVERSED));
        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
        frame.render_widget(msg, chunks[0]);
        frame.render_widget(footer, chunks[1]);
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    let header_cells = ["Name", "Branch", "Status", "Ahead/Behind", ""]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = state
        .rows
        .iter()
        .map(|r| {
            let badge = if r.managed { "" } else { "[unmanaged]" };
            let style = if r.managed {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::DIM)
            };
            Row::new(vec![
                Cell::from(r.name.clone()),
                Cell::from(r.branch.clone()),
                Cell::from(r.status.clone()),
                Cell::from(r.ahead_behind.clone()),
                Cell::from(badge),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::NONE))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected));

    frame.render_stateful_widget(table, chunks[0], &mut table_state);

    let footer = Paragraph::new(Line::from(FOOTER_KEYS))
        .style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_widget(footer, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn render_to_buffer(state: &ListState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(state, frame, frame.area()))
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
                status: "clean".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
            },
            WorktreeRow {
                name: "fix-bug".into(),
                branch: "fix/bug".into(),
                status: "~3".into(),
                ahead_behind: "+0/-2".into(),
                managed: true,
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: false,
            },
        ]
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
    fn selected_row_has_reversed_style() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        // Row 0 is selected by default — its first cell should have REVERSED modifier
        // The header is row 0 in the buffer, data starts at row 1
        let cell = buf.cell((0, 1)).unwrap();
        assert!(
            cell.modifier.contains(Modifier::REVERSED),
            "selected row should have REVERSED style, got: {:?}",
            cell.modifier
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
    fn unmanaged_row_has_dim_style() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        // The unmanaged row is index 2 (third row), rendered at buffer row 3 (header=0, row0=1, row1=2, row2=3)
        let cell = buf.cell((0, 3)).unwrap();
        assert!(
            cell.modifier.contains(Modifier::DIM),
            "unmanaged row should be DIM, got: {:?}",
            cell.modifier
        );
    }

    #[test]
    fn footer_shows_keybindings() {
        let state = ListState::new(sample_rows());
        let buf = render_to_buffer(&state, 100, 10);
        let text = buffer_text(&buf);
        assert!(text.contains("n create"), "footer should show n create");
        assert!(text.contains("s sync"), "footer should show s sync");
        assert!(text.contains("D delete"), "footer should show D delete");
        assert!(
            text.contains("Enter detail"),
            "footer should show Enter detail"
        );
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
}
