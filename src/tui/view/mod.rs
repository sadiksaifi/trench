//! Rendering for the TUI.
//!
//! The screen is the worktree list plus at most one dialog. Every screen
//! shares one frame: content, one status line, and one hint bar on the last
//! row that lists every key that works.

mod create;
mod help;
mod list;
mod operation;
mod remove;
mod sync;
mod widgets;

use ratatui::{
    layout::{Alignment, Rect},
    style::Modifier,
    text::Line,
    widgets::{Block, Paragraph},
    Frame,
};

use crate::tui::{
    app::{AppState, Viewport},
    theme::Theme,
};

pub fn render(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(Block::default().style(theme.on_surface(theme.text)), area);
    if area.width < Viewport::MIN_WIDTH || area.height < Viewport::MIN_HEIGHT {
        render_too_small(frame, area, theme);
        return;
    }
    list::render(state, frame, area, theme);
    if !state.dialog_open() && !state.help_open {
        return;
    }
    dim(frame, area, theme);
    if let Some(modal) = state.operation_modal.as_ref() {
        operation::render(modal, frame, area, theme);
    } else if let Some(dialog) = state.create_dialog.as_ref() {
        create::render(dialog, state, frame, area, theme);
    } else if let Some(dialog) = state.sync_dialog.as_ref() {
        sync::render(dialog, state, frame, area, theme);
    } else if let Some(dialog) = state.remove_dialog.as_ref() {
        remove::render(dialog, frame, area, theme);
    } else {
        help::render(state, frame, area, theme);
    }
}

/// Push the list behind a dialog back without painting over the terminal's
/// own background.
fn dim(frame: &mut Frame, area: Rect, theme: &Theme) {
    let style = theme.on_surface(theme.disabled);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
                cell.modifier = Modifier::empty();
                cell.set_style(style);
            }
        }
    }
}

fn render_too_small(frame: &mut Frame, area: Rect, theme: &Theme) {
    let lines = [
        Line::styled("Terminal too small", theme.accent),
        Line::styled(
            format!(
                "Need {}×{}, have {}×{}",
                Viewport::MIN_WIDTH,
                Viewport::MIN_HEIGHT,
                area.width,
                area.height
            ),
            theme.text,
        ),
        Line::styled("q quit", theme.muted),
    ];
    let height = (lines.len() as u16).min(area.height);
    let top = area.y + (area.height - height) / 2;
    frame.render_widget(
        Paragraph::new(lines.to_vec())
            .alignment(Alignment::Center)
            .style(theme.on_surface(theme.text)),
        Rect::new(area.x, top, area.width, height),
    );
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::PathBuf;

    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    use crate::tui::{
        app::{AppState, Event, WorktreeId, WorktreeIdentity},
        theme,
    };

    /// Render `state` at `width`x`height` and return the buffer.
    pub fn draw(state: &mut AppState, width: u16, height: u16, theme_name: &str) -> Buffer {
        let _ = crate::tui::app::reduce(state, Event::ViewportChanged { width, height });
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = theme::from_name(theme_name);
        terminal
            .draw(|frame| super::render(state, frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The screen as text, one string per row, without trailing spaces.
    ///
    /// On rows crossed by a dialog, the dimmed list beside the dialog is
    /// blanked, so a test of a dialog does not also pin the list behind it.
    pub fn screen(state: &mut AppState, width: u16, height: u16) -> Vec<String> {
        lines(&draw(state, width, height, "terminal"))
            .into_iter()
            .map(|row| {
                let border = |character: char| "╭│╰╮╯".contains(character);
                let characters = row.chars().collect::<Vec<_>>();
                let (Some(first), Some(last)) = (
                    characters.iter().position(|character| border(*character)),
                    characters.iter().rposition(|character| border(*character)),
                ) else {
                    return row;
                };
                characters
                    .iter()
                    .enumerate()
                    .map(|(index, character)| {
                        if (first..=last).contains(&index) {
                            *character
                        } else {
                            ' '
                        }
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    pub fn lines(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    pub fn identity(branch: &str, path: &str) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new(path),
            worktree: branch.replace('/', "-"),
            branch: Some(branch.to_string()),
            path: PathBuf::from(path),
            head: Some("1234567890abcdef".to_string()),
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        }
    }

    /// A repository named `acme` with a current main worktree and `others`.
    pub fn state(others: &[(&str, &str)]) -> AppState {
        let mut identities = vec![WorktreeIdentity {
            is_main: true,
            is_current: true,
            ..identity("main", "/code/acme")
        }];
        identities.extend(others.iter().map(|(branch, path)| identity(branch, path)));
        let mut state = AppState::new(identities);
        state.repository = "acme".to_string();
        state
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::testing::{draw, screen, state};
    use crate::tui::app::Viewport;

    #[test]
    fn a_terminal_below_the_minimum_says_what_it_needs_and_how_to_leave() {
        let mut state = state(&[]);
        let screen = screen(&mut state, 39, 10);

        assert_eq!(screen[3].trim(), "Terminal too small");
        assert_eq!(screen[4].trim(), "Need 40×10, have 39×10");
        assert_eq!(screen[5].trim(), "q quit");
        assert!(screen.iter().all(|row| !row.contains("BRANCH")));
    }

    #[test]
    fn the_minimum_size_shows_the_list() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let screen = screen(&mut state, Viewport::MIN_WIDTH, Viewport::MIN_HEIGHT);

        assert!(screen[1].contains("BRANCH"), "{screen:#?}");
        assert!(screen.iter().any(|row| row.contains("feature/auth")));
    }

    #[test]
    fn the_default_theme_never_paints_a_background_even_behind_a_dialog() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        state.help_open = true;
        let buffer = draw(&mut state, 80, 24, "terminal");

        for cell in buffer.content() {
            assert_eq!(cell.bg, Color::Reset);
        }
    }
}
