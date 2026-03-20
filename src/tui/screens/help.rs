use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// A single keybinding entry: key label + description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingEntry {
    pub key: &'static str,
    pub description: &'static str,
}

/// A group of keybindings sharing a context label (e.g. "Global", "List").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingGroup {
    pub context: &'static str,
    pub bindings: &'static [KeybindingEntry],
}

/// Returns all keybinding groups for the help overlay.
pub fn keybinding_groups() -> &'static [KeybindingGroup] {
    static GROUPS: &[KeybindingGroup] = &[
        KeybindingGroup {
            context: "Global",
            bindings: &[
                KeybindingEntry { key: "?", description: "Toggle help overlay" },
                KeybindingEntry { key: "q / Esc", description: "Back / quit" },
                KeybindingEntry { key: "Ctrl+c", description: "Force quit" },
            ],
        },
        KeybindingGroup {
            context: "List",
            bindings: &[
                KeybindingEntry { key: "j / ↓", description: "Move down" },
                KeybindingEntry { key: "k / ↑", description: "Move up" },
                KeybindingEntry { key: "Enter", description: "Switch to worktree" },
                KeybindingEntry { key: "d", description: "Open detail view" },
                KeybindingEntry { key: "n", description: "Create worktree" },
                KeybindingEntry { key: "s", description: "Sync worktree" },
                KeybindingEntry { key: "D", description: "Delete worktree" },
                KeybindingEntry { key: "l", description: "View hook log" },
            ],
        },
        KeybindingGroup {
            context: "Detail",
            bindings: &[
                KeybindingEntry { key: "s", description: "Sync worktree" },
                KeybindingEntry { key: "o", description: "Open in $EDITOR" },
            ],
        },
    ];
    GROUPS
}

/// Render the help overlay centered within `area`.
pub fn render(frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    let groups = keybinding_groups();

    // Build lines from keybinding data
    let bold = Style::default().fg(theme.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme.dimmed);
    let mut lines: Vec<Line<'_>> = Vec::new();

    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(group.context, bold)));
        for entry in group.bindings {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:12}", entry.key), bold),
                Span::raw(entry.description),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press ? or Esc to close",
        dim,
    )));

    // Size the dialog to fit content + border
    let content_height = lines.len() as u16;
    let dialog_width = 44;
    let dialog_height = content_height + 2; // +2 for top/bottom border

    let dialog_area = centered_rect(dialog_width, dialog_height, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Help ")
        .title_alignment(Alignment::Center);

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, dialog_area);
}

/// Compute a centered rectangle within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn render_to_buffer(width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name("catppuccin");
        terminal
            .draw(|frame| render(frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn render_shows_help_title_in_border() {
        let buf = render_to_buffer(60, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("Help"), "should contain 'Help' title in border");
    }

    #[test]
    fn render_shows_all_group_headers() {
        let buf = render_to_buffer(60, 30);
        let text = buffer_text(&buf);
        for group in keybinding_groups() {
            assert!(
                text.contains(group.context),
                "should contain group header '{}'",
                group.context
            );
        }
    }

    #[test]
    fn render_shows_keybinding_entries() {
        let buf = render_to_buffer(60, 30);
        let text = buffer_text(&buf);
        // Check a sample of keybindings appear
        assert!(text.contains("Toggle help overlay"), "should show '?' description");
        assert!(text.contains("Move down"), "should show j/↓ description");
        assert!(text.contains("Sync worktree"), "should show sync description");
    }

    #[test]
    fn render_shows_dismiss_hint() {
        let buf = render_to_buffer(60, 30);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Press ? or Esc to close"),
            "should contain dismiss hint"
        );
    }

    #[test]
    fn keybinding_groups_returns_global_list_and_detail_contexts() {
        let groups = keybinding_groups();

        // Must have at least 3 groups: Global, List, Detail
        assert!(groups.len() >= 3, "expected at least 3 groups, got {}", groups.len());

        let contexts: Vec<&str> = groups.iter().map(|g| g.context).collect();
        assert!(contexts.contains(&"Global"), "missing Global group");
        assert!(contexts.contains(&"List"), "missing List group");
        assert!(contexts.contains(&"Detail"), "missing Detail group");
    }

    #[test]
    fn each_group_has_at_least_one_binding() {
        let groups = keybinding_groups();
        for group in groups {
            assert!(
                !group.bindings.is_empty(),
                "group '{}' has no bindings",
                group.context
            );
        }
    }

    #[test]
    fn global_group_contains_help_and_quit_bindings() {
        let groups = keybinding_groups();
        let global = groups.iter().find(|g| g.context == "Global").unwrap();
        let keys: Vec<&str> = global.bindings.iter().map(|b| b.key).collect();
        assert!(keys.contains(&"?"), "Global group missing '?' keybinding");
        assert!(keys.contains(&"q / Esc"), "Global group missing quit keybinding");
    }

    #[test]
    fn list_group_contains_switch_detail_and_log_bindings() {
        let groups = keybinding_groups();
        let list = groups.iter().find(|g| g.context == "List").unwrap();
        let descs: Vec<&str> = list.bindings.iter().map(|b| b.description).collect();
        assert!(descs.contains(&"Switch to worktree"), "List group missing switch binding");
        assert!(descs.contains(&"Open detail view"), "List group missing detail binding");
        assert!(descs.contains(&"View hook log"), "List group missing log binding");
    }
}
