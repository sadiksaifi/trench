//! The key reference for the worktree list. Dialogs list their own keys in
//! the hint bar, so only the list needs this.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    output::text,
    tui::{
        app::{unavailable_reason, AppState},
        keymap,
        theme::Theme,
        view::widgets::{self, Hint},
    },
};

pub fn render(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    let bindings = keymap::bindings(state.context());
    // A short terminal cannot fit one key per row, so the keys go in two
    // columns with one-word descriptions instead of being cut off.
    let rows_available = usize::from(area.height.saturating_sub(3));
    let compact = bindings.len() > rows_available;
    let rows = if compact {
        bindings.len().div_ceil(2)
    } else {
        bindings.len()
    };
    let layout = widgets::dialog_layout(area, rows as u16 + 2);
    widgets::dialog_frame(frame, layout, "Keys", None, theme);
    let key_width = bindings
        .iter()
        .map(|binding| text::width(binding.label))
        .max()
        .unwrap_or(0);
    let column_width = if compact {
        usize::from(layout.content.width) / 2
    } else {
        usize::from(layout.content.width)
    };
    let entry = |binding: &keymap::Binding| {
        // A key that does nothing for the selected row stays listed, so
        // help has the same rows wherever it is opened.
        let (key, description) = if unavailable_reason(state, binding.action).is_some() {
            (theme.disabled, theme.disabled)
        } else {
            (theme.accent, theme.text)
        };
        let description_text = text::pad_end(
            &text::truncate_end(
                if compact { binding.hint } else { binding.help },
                column_width.saturating_sub(key_width + 2),
            ),
            column_width.saturating_sub(key_width + 2),
        );
        [
            Span::styled(text::pad_end(binding.label, key_width), key),
            Span::styled(format!("  {description_text}"), description),
        ]
    };
    let lines = (0..rows)
        .map(|row| {
            let mut spans = entry(&bindings[row]).to_vec();
            if let Some(second) = compact.then(|| bindings.get(row + rows)).flatten() {
                spans.extend(entry(second));
            }
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(
        frame,
        layout.hints,
        &[Hint::new("? or Esc", "close", 0)],
        theme,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use crate::tui::{
        app::{reduce, Event},
        keymap::Key,
        view::testing::{draw, lines, screen, state},
    };

    #[test]
    fn help_lists_every_list_key_in_one_column_and_says_how_to_close() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let _ = reduce(&mut state, Event::Input(Key::Char('?')));
        let screen = screen(&mut state, 60, 16);

        assert_eq!(
            screen[1..14],
            [
                "  ╭ Keys ────────────────────────────────────────────────╮",
                "  │ Enter  switch to the selected worktree               │",
                "  │ o      open it in your editor                        │",
                "  │ c      create a worktree                             │",
                "  │ s      sync it with its base branch                  │",
                "  │ d      remove it                                     │",
                "  │ /      search                                        │",
                "  │ r      refresh the list                              │",
                "  │ ↓ j    move down                                     │",
                "  │ ↑ k    move up                                       │",
                "  │ ?      show these keys                               │",
                "  │ q      quit                                          │",
                "  ╰──────────────────────────────────────────────────────╯",
            ]
        );
        assert_eq!(screen[15], " ? or Esc close");
    }

    #[test]
    fn help_dims_the_keys_that_do_nothing_for_the_selected_worktree() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let _ = reduce(&mut state, Event::Input(Key::Char('?')));
        let buffer = draw(&mut state, 60, 16, "terminal");
        let rows = lines(&buffer);
        let remove = rows
            .iter()
            .position(|row| row.contains("remove it"))
            .unwrap() as u16;
        let create = rows
            .iter()
            .position(|row| row.contains("create a worktree"))
            .unwrap() as u16;

        assert!(buffer[(4, remove)].modifier.contains(Modifier::DIM));
        assert!(!buffer[(4, create)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn help_fits_the_minimum_terminal() {
        let mut state = state(&[]);
        let _ = reduce(&mut state, Event::Input(Key::Char('?')));
        let screen = screen(&mut state, 40, 10);

        assert_eq!(
            screen[..8],
            [
                " ╭ Keys ──────────────────────────────╮",
                " │ Enter  switch    r      refresh    │",
                " │ o      open      ↓ j    next       │",
                " │ c      create    ↑ k    previous   │",
                " │ s      sync      ?      help       │",
                " │ d      remove    q      quit       │",
                " │ /      search                      │",
                " ╰────────────────────────────────────╯",
            ]
        );
        assert_eq!(screen[9], " ? or Esc close");
        assert!(screen.iter().all(|row| row.chars().count() <= 40));
    }
}
