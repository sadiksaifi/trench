use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
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
                KeybindingEntry {
                    key: "?",
                    description: "Toggle help overlay",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Back / quit",
                },
                KeybindingEntry {
                    key: "q",
                    description: "Back / quit outside text entry",
                },
                KeybindingEntry {
                    key: "Ctrl+c",
                    description: "Force quit",
                },
            ],
        },
        KeybindingGroup {
            context: "List",
            bindings: &[
                KeybindingEntry {
                    key: "j / ↓",
                    description: "Move down",
                },
                KeybindingEntry {
                    key: "k / ↑",
                    description: "Move up",
                },
                KeybindingEntry {
                    key: "Enter",
                    description: "Switch to worktree",
                },
                KeybindingEntry {
                    key: "d",
                    description: "Open detail view",
                },
                KeybindingEntry {
                    key: "o",
                    description: "Open in $EDITOR",
                },
                KeybindingEntry {
                    key: "n",
                    description: "Create worktree",
                },
                KeybindingEntry {
                    key: "s",
                    description: "Sync worktree",
                },
                KeybindingEntry {
                    key: "D",
                    description: "Delete worktree",
                },
                KeybindingEntry {
                    key: "l",
                    description: "View hook log",
                },
            ],
        },
        KeybindingGroup {
            context: "Detail",
            bindings: &[
                KeybindingEntry {
                    key: "s",
                    description: "Sync worktree",
                },
                KeybindingEntry {
                    key: "o",
                    description: "Open in $EDITOR",
                },
                KeybindingEntry {
                    key: "l",
                    description: "View hook log",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Back",
                },
            ],
        },
        KeybindingGroup {
            context: "Create",
            bindings: &[
                KeybindingEntry {
                    key: "Enter",
                    description: "Advance / create",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Cancel",
                },
                KeybindingEntry {
                    key: "Space",
                    description: "Toggle hooks",
                },
            ],
        },
        KeybindingGroup {
            context: "Sync",
            bindings: &[
                KeybindingEntry {
                    key: "Enter",
                    description: "Confirm / dismiss",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Cancel / back",
                },
            ],
        },
        KeybindingGroup {
            context: "Delete",
            bindings: &[
                KeybindingEntry {
                    key: "Enter",
                    description: "Confirm / dismiss",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Cancel / back",
                },
            ],
        },
        KeybindingGroup {
            context: "Hook Log",
            bindings: &[
                KeybindingEntry {
                    key: "j / ↓",
                    description: "Scroll down",
                },
                KeybindingEntry {
                    key: "k / ↑",
                    description: "Scroll up",
                },
                KeybindingEntry {
                    key: "PgUp / PgDn",
                    description: "Page scroll",
                },
                KeybindingEntry {
                    key: "Enter",
                    description: "Dismiss completed live log",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Back",
                },
            ],
        },
        KeybindingGroup {
            context: "Failure",
            bindings: &[
                KeybindingEntry {
                    key: "Enter",
                    description: "Dismiss result",
                },
                KeybindingEntry {
                    key: "Esc",
                    description: "Back",
                },
            ],
        },
    ];
    GROUPS
}

/// Render the help overlay centered within `area`.
pub fn render(frame: &mut Frame, area: Rect, theme: &crate::tui::theme::Theme) {
    let groups = keybinding_groups();

    let bold = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme.fg_muted);
    let mut lines: Vec<Line<'_>> = Vec::new();

    for group in groups {
        let mut spans = vec![Span::styled(format!("{:8}", group.context), bold)];
        for (index, entry) in group.bindings.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(entry.key.to_string(), bold));
            spans.push(Span::styled(
                format!(" {}", entry.description),
                Style::default().fg(theme.fg_muted),
            ));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Press ? or Esc to close", dim)));

    let dialog_width = area.width.saturating_sub(4).clamp(44, 96);
    let inner_width = dialog_width.saturating_sub(2).max(1) as usize;
    let content_height: u16 = lines
        .iter()
        .map(|line| {
            let width = line.width().max(1);
            width.div_ceil(inner_width) as u16
        })
        .sum();
    let dialog_height = content_height + 3;

    let inner =
        crate::tui::chrome::render_modal(frame, area, theme, dialog_width, dialog_height, " Help ");
    let paragraph = Paragraph::new(lines)
        .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, inner);
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
        assert!(
            text.contains("Help"),
            "should contain 'Help' title in border"
        );
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
        assert!(
            text.contains("Toggle help overlay"),
            "should show '?' description"
        );
        assert!(text.contains("Move down"), "should show j/↓ description");
        assert!(
            text.contains("Sync worktree"),
            "should show sync description"
        );
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
    fn keybinding_groups_returns_all_action_contexts() {
        let groups = keybinding_groups();

        // Must include action contexts surfaced by the TUI
        assert!(
            groups.len() >= 7,
            "expected at least 7 groups, got {}",
            groups.len()
        );

        let contexts: Vec<&str> = groups.iter().map(|g| g.context).collect();
        assert!(contexts.contains(&"Global"), "missing Global group");
        assert!(contexts.contains(&"List"), "missing List group");
        assert!(contexts.contains(&"Detail"), "missing Detail group");
        assert!(contexts.contains(&"Create"), "missing Create group");
        assert!(contexts.contains(&"Sync"), "missing Sync group");
        assert!(contexts.contains(&"Delete"), "missing Delete group");
        assert!(contexts.contains(&"Hook Log"), "missing Hook Log group");
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
        assert!(keys.contains(&"Esc"), "Global group missing Esc keybinding");
        assert!(keys.contains(&"q"), "Global group missing q keybinding");
    }

    #[test]
    fn list_group_contains_switch_detail_open_and_log_bindings() {
        let groups = keybinding_groups();
        let list = groups.iter().find(|g| g.context == "List").unwrap();
        let descs: Vec<&str> = list.bindings.iter().map(|b| b.description).collect();
        assert!(
            descs.contains(&"Switch to worktree"),
            "List group missing switch binding"
        );
        assert!(
            descs.contains(&"Open detail view"),
            "List group missing detail binding"
        );
        assert!(
            descs.contains(&"Open in $EDITOR"),
            "List group missing open binding"
        );
        assert!(
            descs.contains(&"View hook log"),
            "List group missing log binding"
        );
    }

    #[test]
    fn detail_group_contains_sync_open_and_log_bindings() {
        let groups = keybinding_groups();
        let detail = groups.iter().find(|g| g.context == "Detail").unwrap();
        let descs: Vec<&str> = detail.bindings.iter().map(|b| b.description).collect();
        assert!(
            descs.contains(&"Sync worktree"),
            "Detail missing sync binding"
        );
        assert!(
            descs.contains(&"Open in $EDITOR"),
            "Detail missing open binding"
        );
        assert!(
            descs.contains(&"View hook log"),
            "Detail missing log binding"
        );
    }
}
