use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::tui::{
    app::{unavailable_reason, AppState, Viewport, WorktreeIdentity, WorktreeStatus},
    keymap::{self, Binding, Context},
    theme::Theme,
};

pub struct ViewModel<'a> {
    state: &'a AppState,
    area: Rect,
}

impl<'a> ViewModel<'a> {
    pub fn new(state: &'a AppState, area: Rect) -> Self {
        Self { state, area }
    }

    pub fn is_tiny(&self) -> bool {
        self.area.width < Viewport::MIN_WIDTH || self.area.height < Viewport::MIN_HEIGHT
    }

    pub fn is_wide(&self) -> bool {
        self.area.width >= Viewport::WIDE_WIDTH
    }

    pub fn inspector_visible(&self) -> bool {
        self.state
            .inspector_override
            .unwrap_or_else(|| self.is_wide())
    }

    pub fn selected(&self) -> Option<&'a WorktreeIdentity> {
        self.state.selected_identity()
    }

    pub fn status_for(&self, identity: &WorktreeIdentity) -> Option<&'a WorktreeStatus> {
        self.state.statuses.get(&identity.id)
    }
}

pub fn render(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(
        Block::default().style(theme.with_bg(Style::default().fg(theme.fg), theme.bg)),
        area,
    );
    let model = ViewModel::new(state, area);
    if model.is_tiny() {
        render_resize(&model, frame, theme);
    } else {
        render_cockpit(&model, frame, theme);
    }
    if state.help_open {
        render_help(&model, frame, theme);
    }
}

fn render_resize(model: &ViewModel<'_>, frame: &mut Frame, theme: &Theme) {
    let [body, keybar] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(model.area);
    let message = vec![
        Line::from(Span::styled(
            "Resize terminal",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metric_line(
            "Current",
            &format!("{}×{}", model.area.width, model.area.height),
            theme,
        ),
        metric_line(
            "Minimum",
            &format!("{}×{}", Viewport::MIN_WIDTH, Viewport::MIN_HEIGHT),
            theme,
        ),
    ];
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg)),
        body,
    );
    render_keybar(model.state, frame, keybar, theme, Context::Resize, true);
}

fn render_cockpit(model: &ViewModel<'_>, frame: &mut Frame, theme: &Theme) {
    let [body, keybar] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(model.area);
    if model.inspector_visible() && model.is_wide() {
        let [list, inspector] =
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                .areas(body);
        render_list(model, frame, list, theme);
        render_inspector(model, frame, inspector, theme);
    } else if model.inspector_visible() {
        render_inspector(model, frame, body, theme);
    } else {
        render_list(model, frame, body, theme);
    }
    render_keybar(
        model.state,
        frame,
        keybar,
        theme,
        Context::Cockpit,
        !model.is_wide(),
    );
}

fn render_list(model: &ViewModel<'_>, frame: &mut Frame, area: Rect, theme: &Theme) {
    let title = format!(" Worktrees · {} ", model.state.identities.len());
    let header = Row::new(["Worktree", "Branch", "Git"])
        .style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .height(1);
    let rows = model.state.identities.iter().map(|identity| {
        let current = if identity.is_current { "* " } else { "" };
        Row::new([
            Cell::from(format!("{current}{}", identity.worktree)),
            Cell::from(
                identity
                    .branch
                    .clone()
                    .unwrap_or_else(|| "detached".to_string()),
            ),
            Cell::from(
                model
                    .status_for(identity)
                    .map(compact_git)
                    .unwrap_or_else(|| "…".to_string()),
            ),
        ])
        .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(31),
            Constraint::Percentage(34),
            Constraint::Percentage(35),
        ],
    )
    .header(header)
    .block(panel(Some(title), theme))
    .row_highlight_style(
        theme.with_bg(
            Style::default()
                .fg(theme.selection_fg)
                .add_modifier(Modifier::BOLD),
            theme.selection_bg,
        ),
    )
    .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel));
    let mut table_state = TableState::default();
    let selected = model
        .state
        .selected
        .as_ref()
        .and_then(|id| model.state.identities.iter().position(|row| &row.id == id));
    table_state.select(selected);
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_inspector(model: &ViewModel<'_>, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = panel(None, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(identity) = model.selected() else {
        frame.render_widget(
            Paragraph::new("No worktree selected")
                .alignment(Alignment::Center)
                .style(theme.with_bg(Style::default().fg(theme.fg_muted), theme.bg_panel)),
            inner,
        );
        return;
    };
    let status = model.status_for(identity);
    let mut lines = vec![Line::from(Span::styled(
        identity.worktree.clone(),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    if identity.is_current {
        lines.push(Line::from(Span::styled(
            " current ",
            theme.with_bg(
                Style::default()
                    .fg(theme.selection_fg)
                    .add_modifier(Modifier::BOLD),
                theme.accent_soft,
            ),
        )));
    }
    lines.extend([
        Line::from(""),
        metric_line(
            "Branch",
            identity.branch.as_deref().unwrap_or("detached"),
            theme,
        ),
        metric_line("Path", &identity.path.to_string_lossy(), theme),
        metric_line(
            "HEAD",
            &identity
                .head
                .as_deref()
                .map(short_head)
                .unwrap_or_else(|| "—".to_string()),
            theme,
        ),
        metric_line("Staged", &count(status.map(|value| value.staged)), theme),
        metric_line(
            "Modified",
            &count(status.map(|value| value.modified)),
            theme,
        ),
        metric_line(
            "Untracked",
            &count(status.map(|value| value.untracked)),
            theme,
        ),
        compare_line(status, theme),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel)),
        inner,
    );
}

fn render_keybar(
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    context: Context,
    narrow: bool,
) {
    let bindings: Vec<_> = keymap::keybar_bindings(context, narrow)
        .into_iter()
        .filter(|binding| unavailable_reason(state, binding.action).is_none())
        .collect();
    let mut spans = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            binding.label,
            Style::default()
                .fg(theme.selection_fg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", binding.description),
            Style::default().fg(theme.fg_muted),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
        area,
    );
}

fn render_help(model: &ViewModel<'_>, frame: &mut Frame, theme: &Theme) {
    let context = if model.is_tiny() {
        Context::Resize
    } else {
        Context::Cockpit
    };
    let bindings = keymap::bindings(context);
    let two_columns = model.is_wide();
    let width = if two_columns {
        model.area.width.saturating_sub(8).min(96)
    } else {
        model.area.width.saturating_sub(4).min(54)
    };
    let rows = if two_columns {
        bindings.len().div_ceil(2)
    } else {
        bindings.len()
    };
    let height = (rows as u16 + 4).min(model.area.height.saturating_sub(2));
    let dialog = centered_rect(width, height, model.area);
    frame.render_widget(Clear, dialog);
    let block = panel(Some(" Help · Worktrees ".to_string()), theme);
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    if two_columns {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(inner);
        let split = bindings.len().div_ceil(2);
        render_help_column(model.state, frame, left, theme, &bindings[..split]);
        render_help_column(model.state, frame, right, theme, &bindings[split..]);
    } else {
        render_help_column(model.state, frame, inner, theme, bindings);
    }
}

fn render_help_column(
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    bindings: &[Binding],
) {
    let lines = bindings.iter().map(|binding| {
        let reason = unavailable_reason(state, binding.action);
        let description = reason.unwrap_or(binding.description);
        let style = if reason.is_some() {
            Style::default().fg(theme.fg_muted)
        } else {
            Style::default().fg(theme.fg)
        };
        Line::from(vec![
            Span::styled(
                format!("{:<8}", binding.label),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(description.to_string(), style),
        ])
    });
    frame.render_widget(
        Paragraph::new(lines.collect::<Vec<_>>())
            .wrap(Wrap { trim: true })
            .style(theme.with_bg(Style::default(), theme.bg_panel)),
        area,
    );
}

fn panel(title: Option<String>, theme: &Theme) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(theme.with_bg(Style::default(), theme.bg_panel));
    match title {
        Some(title) => block.title(title),
        None => block,
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn compact_git(status: &WorktreeStatus) -> String {
    let changed = status.staged + status.modified + status.untracked;
    format!(
        "{changed} changed · ↑{} ↓{}",
        optional_number(status.ahead),
        optional_number(status.behind)
    )
}

fn compare_line(status: Option<&WorktreeStatus>, theme: &Theme) -> Line<'static> {
    let base = status
        .and_then(|value| value.base.as_deref())
        .unwrap_or("—");
    let value = format!(
        "↑{} ↓{}",
        optional_number(status.and_then(|value| value.ahead)),
        optional_number(status.and_then(|value| value.behind))
    );
    metric_line(&format!("Compare {base}"), &value, theme)
}

fn optional_number(value: Option<usize>) -> String {
    value.map_or_else(|| "—".to_string(), |value| value.to_string())
}

fn count(value: Option<u32>) -> String {
    value.map_or_else(|| "—".to_string(), |value| value.to_string())
}

fn short_head(head: &str) -> String {
    head.chars().take(7).collect()
}

fn metric_line(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<14}"),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(theme.fg)),
    ])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{backend::TestBackend, buffer::Buffer, style::Color, Terminal};

    use super::*;
    use crate::tui::app::{reduce, Event, WorktreeId};

    fn identity(
        path: &str,
        worktree: &str,
        branch: Option<&str>,
        is_main: bool,
        is_current: bool,
    ) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new(path),
            worktree: worktree.to_string(),
            branch: branch.map(str::to_string),
            path: PathBuf::from(path),
            head: Some("1234567890abcdef".to_string()),
            is_main,
            is_current,
            detached: branch.is_none(),
        }
    }

    fn sample_state() -> AppState {
        let current = identity(
            "/worktrees/feature-auth",
            "feature-auth",
            Some("feature/auth"),
            false,
            true,
        );
        let main = identity("/repos/trench", "trench", Some("main"), true, false);
        let mut state = AppState::new(vec![current.clone(), main]);
        state.statuses.insert(
            current.id,
            WorktreeStatus {
                base: Some("main".to_string()),
                staged: 1,
                modified: 1,
                untracked: 1,
                ahead: Some(2),
                behind: Some(1),
            },
        );
        state
    }

    fn render_buffer(state: &mut AppState, width: u16, height: u16, theme_name: &str) -> Buffer {
        let _ = reduce(state, Event::ViewportChanged { width, height });
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name(theme_name);
        terminal
            .draw(|frame| render(state, frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn text(buffer: &Buffer) -> String {
        lines(buffer).join("\n")
    }

    #[test]
    fn resize_view_uses_strict_60_by_16_boundary() {
        let mut state = sample_state();
        let width_tiny = text(&render_buffer(&mut state, 59, 16, "ops"));
        let height_tiny = text(&render_buffer(&mut state, 60, 15, "ops"));
        let exact_minimum = text(&render_buffer(&mut state, 60, 16, "ops"));

        assert!(width_tiny.contains("Resize terminal"), "{width_tiny}");
        assert!(width_tiny.contains("Current       59×16"), "{width_tiny}");
        assert!(width_tiny.contains("Minimum       60×16"), "{width_tiny}");
        assert!(height_tiny.contains("Resize terminal"), "{height_tiny}");
        assert!(exact_minimum.contains("Worktrees · 2"), "{exact_minimum}");
        assert!(
            !exact_minimum.contains("Resize terminal"),
            "{exact_minimum}"
        );
    }

    #[test]
    fn wide_cockpit_contains_only_the_frozen_columns_and_inspector_fields() {
        let mut state = sample_state();
        let output = text(&render_buffer(&mut state, 120, 24, "catppuccin"));

        for expected in [
            "Worktrees · 2",
            "Worktree",
            "Branch",
            "Git",
            "feature-auth",
            "feature/auth",
            "3 changed · ↑2 ↓1",
            "Path",
            "/worktrees/feature-auth",
            "HEAD          1234567",
            "Staged        1",
            "Modified      1",
            "Untracked     1",
            "Compare main  ↑2 ↓1",
            "current",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        for removed in [
            "Worktree Cockpit",
            "watch on",
            "Processes",
            "Procs",
            "Detail",
        ] {
            assert!(!output.contains(removed), "found {removed:?}\n{output}");
        }
    }

    #[test]
    fn narrow_layout_hides_inspector_until_i_toggles_it_for_the_run() {
        let mut state = sample_state();
        let hidden = text(&render_buffer(&mut state, 80, 20, "ops"));
        assert!(hidden.contains("Worktrees · 2"), "{hidden}");
        assert!(!hidden.contains("/worktrees/feature-auth"), "{hidden}");

        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('i')));
        let visible = text(&render_buffer(&mut state, 80, 20, "ops"));
        assert!(visible.contains("/worktrees/feature-auth"), "{visible}");
        assert!(!visible.contains("Worktrees · 2"), "{visible}");
    }

    #[test]
    fn keybars_end_in_help_and_only_expose_eligible_row_actions() {
        let main = identity("/repos/trench", "trench", Some("main"), true, true);
        let id = main.id.clone();
        let mut state = AppState::new(vec![main]);
        state.statuses.insert(id, WorktreeStatus::default());
        let buffer = render_buffer(&mut state, 120, 20, "ops");
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();

        assert!(footer.ends_with("? help"), "{footer}");
        assert!(!footer.contains("d remove"), "{footer}");
        assert!(footer.contains("s sync"), "{footer}");

        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('?')));
        let help = text(&render_buffer(&mut state, 120, 20, "ops"));
        assert!(
            help.contains("The main worktree cannot be removed"),
            "{help}"
        );
    }

    #[test]
    fn narrow_keybar_omits_but_keeps_hidden_shortcuts_routable() {
        let mut state = sample_state();
        let buffer = render_buffer(&mut state, 80, 20, "ops");
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();

        assert!(footer.contains("Enter switch"), "{footer}");
        assert!(footer.contains("c create"), "{footer}");
        assert!(footer.contains("/ search"), "{footer}");
        assert!(footer.ends_with("? help"), "{footer}");
        assert!(!footer.contains("o open"), "{footer}");

        let effects = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('o')));
        assert!(matches!(
            effects.as_slice(),
            [crate::tui::app::Effect::Open(_)]
        ));
    }

    #[test]
    fn contextual_help_uses_two_columns_wide_and_one_column_narrow() {
        let mut state = sample_state();
        state.help_open = true;
        let wide = lines(&render_buffer(&mut state, 120, 24, "ops"));
        let narrow = lines(&render_buffer(&mut state, 80, 24, "ops"));
        let wide_enter = wide.iter().position(|line| line.contains("Enter")).unwrap();
        let wide_refresh = wide
            .iter()
            .position(|line| line.contains("r       refresh"))
            .unwrap();
        let narrow_enter = narrow
            .iter()
            .position(|line| line.contains("Enter"))
            .unwrap();
        let narrow_refresh = narrow
            .iter()
            .position(|line| line.contains("r       refresh"))
            .unwrap();

        assert_eq!(wide_enter, wide_refresh, "{}", wide.join("\n"));
        assert_ne!(narrow_enter, narrow_refresh, "{}", narrow.join("\n"));
    }

    #[test]
    fn selected_row_uses_the_configured_theme() {
        let mut state = sample_state();
        let buffer = render_buffer(&mut state, 80, 20, "catppuccin");
        let theme = crate::tui::theme::from_name("catppuccin");
        let selected_cells = buffer
            .content()
            .iter()
            .filter(|cell| cell.bg == theme.selection_bg)
            .count();

        assert!(selected_cells > 0);
        assert_ne!(theme.selection_bg, Color::Reset);
    }
}
