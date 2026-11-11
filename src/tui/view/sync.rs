//! The sync dialog: what is synced, onto what, and how.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    cli::commands::sync::stateless::SyncStrategy,
    output::text,
    tui::{
        app::AppState,
        sync_flow::{SyncDialog, SyncMode},
        theme::Theme,
        view::{
            list::spinner,
            widgets::{self, Hint},
        },
    },
};

/// `Strategy`, the widest label.
const LABEL_WIDTH: usize = 8;
const HEIGHT: u16 = 11;

pub fn render(dialog: &SyncDialog, state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    let layout = widgets::dialog_layout(area, HEIGHT);
    let width = usize::from(layout.content.width);
    let rows = usize::from(layout.content.height);
    let room = width.saturating_sub(LABEL_WIDTH + 2);
    let status = if dialog.origin_spinner_visible() {
        Some(Line::styled(
            format!(" fetching origin {} ", spinner(state.refresh.spinner_tick)),
            theme.on_dialog(theme.muted),
        ))
    } else {
        dialog
            .warning()
            .map(|_| Line::styled(" could not fetch origin ", theme.on_dialog(theme.warning)))
    };
    widgets::dialog_frame(frame, layout, "Sync worktree", status, theme);

    let branch = dialog.branch().unwrap_or(dialog.worktree());
    let mut lines = vec![widgets::field(
        "Branch",
        LABEL_WIDTH,
        vec![Span::styled(
            text::truncate_middle(branch, room),
            theme.text,
        )],
        theme,
    )];
    match dialog.mode() {
        SyncMode::Form => {
            lines.push(with_hint(
                "Base",
                text::truncate_middle(dialog.base().unwrap_or("none found"), room),
                "Tab to change",
                room,
                theme,
            ));
            lines.push(strategy(dialog.strategy(), room, theme));
            lines.push(Line::default());
            if let Some(error) = dialog.validation_error() {
                lines.extend(
                    widgets::wrap_clamped(&format!("{} {error}", widgets::FAILED), width, 3)
                        .into_iter()
                        .map(|line| Line::styled(line, theme.error)),
                );
            } else {
                lines.extend(sentence(dialog, branch, state, width, theme));
            }
        }
        SyncMode::BasePicker => {
            lines.push(widgets::field(
                "Base",
                LABEL_WIDTH,
                widgets::input(
                    dialog.base_input(),
                    "type to filter bases",
                    room,
                    true,
                    theme,
                ),
                theme,
            ));
            lines.push(Line::default());
            let candidates = dialog.base_candidates();
            if candidates.is_empty() {
                lines.push(Line::styled(
                    format!("  No branch matches \"{}\".", dialog.base_input().value()),
                    theme.muted,
                ));
            }
            let window = widgets::window(
                candidates.len(),
                Some(dialog.base_selection()),
                rows.saturating_sub(lines.len()),
            );
            lines.extend(window.rows.clone().map(|index| {
                widgets::row(
                    candidates[index].name.clone(),
                    (width >= 44)
                        .then_some((widgets::ref_tag(candidates[index].kind), theme.muted)),
                    index == dialog.base_selection(),
                    width,
                    theme,
                )
            }));
            if window.more > 0 {
                lines.push(Line::styled(
                    format!("  … {} more", window.more),
                    theme.muted,
                ));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(frame, layout.hints, &hints(dialog), theme);
}

/// A field whose right edge names the key that changes it.
///
/// The value always wins: the reminder is dropped when both do not fit,
/// because the hint bar names the same key.
fn with_hint(
    label: &str,
    value: String,
    hint: &'static str,
    room: usize,
    theme: &Theme,
) -> Line<'static> {
    let gap = room.saturating_sub(text::width(&value) + text::width(hint));
    let mut value = vec![Span::styled(value, theme.text)];
    if gap >= 2 {
        value.push(Span::raw(" ".repeat(gap)));
        value.push(Span::styled(hint, theme.muted));
    }
    widgets::field(label, LABEL_WIDTH, value, theme)
}

fn strategy(strategy: SyncStrategy, room: usize, theme: &Theme) -> Line<'static> {
    let option = |name: &str, chosen: bool| {
        // The filled dot marks the choice without color; the accent adds to it.
        Span::styled(
            format!("{} {name}", if chosen { "●" } else { "○" }),
            if chosen { theme.accent } else { theme.muted },
        )
    };
    let hint = "←/→ to switch";
    let used = "● rebase   ○ merge".chars().count();
    let gap = room.saturating_sub(used + text::width(hint));
    let mut value = vec![
        option("rebase", strategy == SyncStrategy::Rebase),
        Span::raw("   "),
        option("merge", strategy == SyncStrategy::Merge),
    ];
    if gap >= 2 {
        value.push(Span::raw(" ".repeat(gap)));
        value.push(Span::styled(hint, theme.muted));
    }
    widgets::field("Strategy", LABEL_WIDTH, value, theme)
}

/// What Enter will do, as a sentence.
fn sentence(
    dialog: &SyncDialog,
    branch: &str,
    state: &AppState,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let Some(base) = dialog.base() else {
        return vec![
            Line::styled(
                format!("{} No base branch found.", widgets::FAILED),
                theme.error,
            ),
            Line::styled("Press Tab and choose one.", theme.muted),
        ];
    };
    let action = match dialog.strategy() {
        SyncStrategy::Rebase => format!("Rebases {branch} onto {base}."),
        SyncStrategy::Merge => format!("Merges {base} into {branch}."),
    };
    let mut lines = widgets::wrap_clamped(&action, width, 2)
        .into_iter()
        .map(|line| Line::styled(line, theme.text))
        .collect::<Vec<_>>();
    if !state.hooks.sync.is_empty() {
        lines.push(Line::styled(
            text::truncate_end(
                &format!("Runs hooks: {}", state.hooks.sync.join(", ")),
                width,
            ),
            theme.muted,
        ));
    }
    lines
}

fn hints(dialog: &SyncDialog) -> Vec<Hint> {
    match dialog.mode() {
        SyncMode::Form => vec![
            Hint::new("Enter", "sync", 0).enabled(dialog.submission().is_some()),
            Hint::new("Tab", "base", 2),
            Hint::new("←/→", "strategy", 3),
            Hint::new("Esc", "cancel", 1),
        ],
        SyncMode::BasePicker => vec![
            Hint::new("Enter", "use base", 0).enabled(!dialog.base_candidates().is_empty()),
            Hint::new("↑/↓", "pick", 2),
            Hint::new("Esc", "back", 1),
        ],
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ref_catalog::RefSnapshot,
        tui::{
            app::AppState,
            sync_flow::{SyncDialog, SyncKey},
            view::testing::{identity, screen, state},
        },
    };

    fn open(keys: &[SyncKey]) -> AppState {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        state.hooks.sync = vec!["post_sync"];
        let mut dialog = SyncDialog::new(
            &identity("feature/auth", "/wt/acme/feature-auth"),
            RefSnapshot::from_parts(
                ["main", "feature/auth", "release"],
                ["origin/main"],
                Some("origin/main"),
                Some("main"),
                true,
            ),
            None,
        );
        for key in keys {
            dialog.handle_key(*key);
        }
        state.sync_dialog = Some(dialog);
        state
    }

    #[test]
    fn form_states_the_branch_base_strategy_and_what_enter_will_do() {
        let screen = screen(&mut open(&[]), 60, 16);

        assert_eq!(
            screen[2..13],
            [
                "  ╭ Sync worktree ───────────────────────────────────────╮",
                "  │ Branch    feature/auth                               │",
                "  │ Base      origin/main                  Tab to change │",
                "  │ Strategy  ● rebase   ○ merge           ←/→ to switch │",
                "  │                                                      │",
                "  │ Rebases feature/auth onto origin/main.               │",
                "  │ Runs hooks: post_sync                                │",
                "  │                                                      │",
                "  │                                                      │",
                "  │                                                      │",
                "  ╰──────────────────────────────────────────────────────╯",
            ]
        );
        assert_eq!(
            screen[15],
            " Enter sync  Tab base  ←/→ strategy  Esc cancel"
        );
    }

    #[test]
    fn switching_strategy_rewrites_the_sentence() {
        let screen = screen(&mut open(&[SyncKey::Right]), 60, 16);

        assert_eq!(
            screen[5],
            "  │ Strategy  ○ rebase   ● merge           ←/→ to switch │"
        );
        assert_eq!(
            screen[7],
            "  │ Merges origin/main into feature/auth.                │"
        );
    }

    #[test]
    fn the_base_picker_filters_inside_the_same_dialog() {
        let form = screen(&mut open(&[]), 60, 16);
        let picker = screen(
            &mut open(&[
                SyncKey::Tab,
                SyncKey::Character('r'),
                SyncKey::Character('e'),
            ]),
            60,
            16,
        );

        assert_eq!(picker[2], form[2]);
        assert_eq!(picker[12], form[12]);
        assert_eq!(
            picker[3..8],
            [
                "  │ Branch    feature/auth                               │",
                "  │ Base      re▌                                        │",
                "  │                                                      │",
                "  │ › release                             local branch   │",
                "  │   feature/auth                        local branch   │",
            ]
        );
        assert_eq!(picker[15], " Enter use base  ↑/↓ pick  Esc back");
    }

    #[test]
    fn a_planning_error_replaces_the_sentence() {
        let mut state = open(&[]);
        state
            .sync_dialog
            .as_mut()
            .unwrap()
            .set_validation_error(Some("worktree has uncommitted changes".to_string()));
        let screen = screen(&mut state, 60, 16);

        assert_eq!(
            screen[7],
            "  │ × worktree has uncommitted changes                   │"
        );
    }

    #[test]
    fn a_sentence_too_long_for_the_dialog_ends_in_an_ellipsis() {
        let mut state = state(&[]);
        let branch = "feature/JIRA-12345-implement-the-thing-nobody-asked-for";
        state.sync_dialog = Some(SyncDialog::new(
            &identity(branch, "/wt/acme/long"),
            RefSnapshot::from_parts(
                ["main", branch],
                ["origin/main"],
                Some("origin/main"),
                Some("main"),
                true,
            ),
            None,
        ));
        let screen = screen(&mut state, 40, 10);

        assert_eq!(
            screen[5..7],
            [
                " │ Rebases                            │",
                " │ feature/JIRA-12345-implement-the-… │",
            ]
        );
    }

    #[test]
    fn the_form_fits_the_minimum_terminal() {
        let screen = screen(&mut open(&[]), 40, 10);

        assert_eq!(
            screen[..10],
            [
                " ╭ Sync worktree ─────────────────────╮",
                " │ Branch    feature/auth             │",
                " │ Base      origin/main              │",
                " │ Strategy  ● rebase   ○ merge       │",
                " │                                    │",
                " │ Rebases feature/auth onto          │",
                " │ origin/main.                       │",
                " │ Runs hooks: post_sync              │",
                " ╰────────────────────────────────────╯",
                " Enter sync  Tab base  Esc cancel",
            ]
        );
    }
}
