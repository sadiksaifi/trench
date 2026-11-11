//! The worktree list: header, rows, one status line, one hint bar.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    output::text,
    paths,
    tui::{
        app::{unavailable_reason, AppState, NoticeKind, WorktreeIdentity},
        keymap::{self, Action, Context},
        theme::Theme,
        view::widgets::{self, Hint, Tone, MARGIN},
    },
};

const MISSING_TAG: &str = "  missing";
/// Selection cursor, a space, the current-worktree marker, a space.
const GUTTER: usize = 4;
const COLUMN_GAP: usize = 2;

pub fn render(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    let row = |y: u16| Rect::new(area.x, area.y + y, area.width, 1);
    let last = area.height - 1;
    // One row is always kept for the status line. A message too long for it
    // takes a second row from the list instead of being cut.
    let status = status(state, area.width, theme);
    let status_rows = (status.len() as u16).max(1);
    let body = Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(3 + status_rows),
    );

    let visible = state.visible_identities();
    let home = paths::home_dir();
    let columns = Columns::new(&visible, home.as_deref(), area.width);

    frame.render_widget(
        Paragraph::new(header(state, visible.len(), area.width, theme)),
        row(0),
    );
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(
                "{}{}{}PATH",
                " ".repeat(usize::from(MARGIN) + GUTTER),
                text::pad_end("BRANCH", columns.branch),
                " ".repeat(COLUMN_GAP)
            ),
            theme.muted,
        )),
        row(1),
    );
    frame.render_widget(
        Paragraph::new(body_lines(state, &visible, &columns, body, theme)),
        body,
    );
    if !status.is_empty() {
        frame.render_widget(
            Paragraph::new(status),
            Rect::new(area.x, last - status_rows, area.width, status_rows),
        );
    }
    widgets::hint_bar(frame, row(last), &hints(state, visible.is_empty()), theme);
}

/// Column widths for the rows on screen.
struct Columns {
    branch: usize,
    path: usize,
    paths: Vec<String>,
}

impl Columns {
    fn new(rows: &[&WorktreeIdentity], home: Option<&std::path::Path>, width: u16) -> Self {
        let content = usize::from(width).saturating_sub(usize::from(MARGIN) * 2 + GUTTER);
        let widest = rows
            .iter()
            .map(|row| text::width(row.label()))
            .max()
            .unwrap_or(0);
        // The branch column takes what it needs up to 45% of the row, so a
        // long branch name cannot push every path off screen.
        let branch = widest
            .max(text::width("BRANCH"))
            .min((content * 45 / 100).max(text::width("BRANCH")));
        Self {
            branch,
            path: content.saturating_sub(branch + COLUMN_GAP),
            paths: rows
                .iter()
                .map(|row| text::home_relative(&row.path, home))
                .collect(),
        }
    }
}

fn header(state: &AppState, visible: usize, width: u16, theme: &Theme) -> Line<'static> {
    let total = state.identities.len();
    let (left, left_width): (Vec<Span<'static>>, usize) = match state.search.as_ref() {
        Some(query) => {
            let room = usize::from(width).saturating_sub(20);
            let mut spans = vec![Span::styled(" / ", theme.accent)];
            spans.extend(widgets::input(
                query.input(),
                "type to filter worktrees",
                room,
                true,
                theme,
            ));
            let used = spans.iter().map(|span| text::width(&span.content)).sum();
            (spans, used)
        }
        None => {
            let repository =
                text::truncate_end(&state.repository, usize::from(width).saturating_sub(24));
            let used = 1 + "trench".len() + 2 + text::width(&repository);
            (
                vec![
                    Span::styled(" trench", theme.accent),
                    Span::styled(format!("  {repository}"), theme.text),
                ],
                used,
            )
        }
    };
    let count = match state.search {
        Some(_) => format!("{visible} of {total}"),
        None if total == 1 => "1 worktree".to_string(),
        None => format!("{total} worktrees"),
    };
    let right = if state.refresh.updating_refs {
        format!(
            "fetching origin {}  {count}",
            spinner(state.refresh.spinner_tick)
        )
    } else {
        count
    };
    let gap = usize::from(width)
        .saturating_sub(left_width + text::width(&right) + usize::from(MARGIN))
        .max(1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, theme.muted));
    Line::from(spans)
}

fn body_lines(
    state: &AppState,
    visible: &[&WorktreeIdentity],
    columns: &Columns,
    body: Rect,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(usize::from(MARGIN) + GUTTER);
    let note = |message: String| Line::styled(format!("{indent}{message}"), theme.muted);
    if visible.is_empty() {
        return vec![match state.search.as_ref() {
            Some(query) => note(format!("No worktree matches \"{}\".", query.as_str())),
            None => note("Loading worktrees…".to_string()),
        }];
    }
    let selected = state
        .selected
        .as_ref()
        .and_then(|id| visible.iter().position(|row| &row.id == id));
    let window = widgets::window(visible.len(), selected, usize::from(body.height));
    let mut lines = window
        .rows
        .clone()
        .map(|index| {
            worktree_row(
                visible[index],
                &columns.paths[index],
                columns,
                Some(index) == selected,
                usize::from(body.width),
                theme,
            )
        })
        .collect::<Vec<_>>();
    if window.more > 0 {
        lines.push(note(format!("… {} more", window.more)));
    }
    if state.search.is_none() && state.identities.len() == 1 && body.height >= 3 {
        lines.push(Line::default());
        lines.push(note(
            "No other worktrees yet. Press c to create one.".to_string(),
        ));
    }
    lines
}

fn worktree_row(
    identity: &WorktreeIdentity,
    path: &str,
    columns: &Columns,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let cursor = if selected { widgets::SELECTED } else { " " };
    let current = if identity.is_current { "*" } else { " " };
    let branch = text::pad_end(
        &text::truncate_middle(identity.label(), columns.branch),
        columns.branch,
    );
    let tag = if identity.missing { MISSING_TAG } else { "" };
    let path = text::truncate_start(path, columns.path.saturating_sub(text::width(tag)));
    let base = if selected {
        theme.selection
    } else {
        theme.text
    };
    let styled = |style: Style| if selected { theme.selection } else { style };

    let mut spans = vec![
        Span::styled(
            format!("{}{cursor} ", " ".repeat(usize::from(MARGIN))),
            base,
        ),
        Span::styled(format!("{current} "), styled(theme.success)),
        Span::styled(format!("{branch}{}", " ".repeat(COLUMN_GAP)), base),
        Span::styled(path, styled(theme.muted)),
        Span::styled(tag, styled(theme.warning)),
    ];
    if selected {
        // The selection is a full-width bar, so it reads as one row.
        let used: usize = spans.iter().map(|span| text::width(&span.content)).sum();
        spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
    }
    Line::from(spans)
}

/// The status line: the latest message, or a standing refresh warning.
///
/// It wraps onto a second row before it truncates, because these messages
/// name the command or key that fixes the problem.
fn status(state: &AppState, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let (kind, message) = match (&state.notification, &state.refresh.warning) {
        (Some(notice), _) => (notice.kind, notice.text.as_str()),
        (None, Some(warning)) => (NoticeKind::Warning, warning.as_str()),
        (None, None) => return Vec::new(),
    };
    let (glyph, style) = match kind {
        NoticeKind::Success => (widgets::OK, theme.success),
        NoticeKind::Warning => (widgets::WARNING, theme.warning),
        NoticeKind::Error => (widgets::FAILED, theme.error),
    };
    let margin = " ".repeat(usize::from(MARGIN));
    let room = usize::from(width).saturating_sub(usize::from(MARGIN) * 2 + 2);
    widgets::wrap_clamped(message, room, 2)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            Line::from(vec![
                Span::styled(
                    if index == 0 {
                        format!("{margin}{glyph} ")
                    } else {
                        format!("{margin}  ")
                    },
                    style,
                ),
                Span::styled(line, theme.text),
            ])
        })
        .collect()
}

fn hints(state: &AppState, nothing_visible: bool) -> Vec<Hint> {
    let context = state.context();
    keymap::hint_bindings(context)
        .into_iter()
        .map(|binding| {
            let available = unavailable_reason(state, binding.action).is_none()
                && !(nothing_visible && binding.action == Action::Switch);
            Hint::new(binding.label, binding.hint, binding.rank.unwrap_or(0)).tone(
                if available || context == Context::Resize {
                    Tone::Normal
                } else {
                    Tone::Disabled
                },
            )
        })
        .collect()
}

pub fn spinner(tick: u64) -> char {
    let frames = tui_spinner::FluxFrames::BRAILLE;
    frames[(tick % frames.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::style::Modifier;

    use crate::tui::{
        app::{reduce, Event, NoticeKind, WorktreeId, WorktreeIdentity},
        keymap::Key,
        view::testing::{draw, identity, screen, state},
    };

    const LONG: &str = "feature/JIRA-12345-implement-the-thing-nobody-asked-for";
    const LONGER: &str = "feature/JIRA-12345-implement-the-thing-nobody-asked-for-v2";

    #[test]
    fn list_shows_branch_and_path_behind_a_fixed_gutter() {
        let mut state = state(&[
            ("feature/auth", "/wt/acme/feature-auth"),
            ("fix/typo", "/wt/acme/fix-typo"),
        ]);
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let screen = screen(&mut state, 80, 12);

        assert_eq!(
            screen[..5],
            [
                " trench  acme                                                       3 worktrees",
                "     BRANCH        PATH",
                "   * main          /code/acme",
                " ›   feature/auth  /wt/acme/feature-auth",
                "     fix/typo      /wt/acme/fix-typo",
            ]
        );
        assert_eq!(
            screen[11],
            " Enter switch  o open  c create  s sync  d remove  / search  ? help  q quit"
        );
    }

    #[test]
    fn selected_row_is_a_full_width_bar_and_the_current_marker_is_not_color_alone() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let buffer = draw(&mut state, 60, 10, "terminal");

        for x in 0..60 {
            assert!(
                buffer[(x, 2)].modifier.contains(Modifier::REVERSED),
                "column {x}"
            );
            assert!(!buffer[(x, 3)].modifier.contains(Modifier::REVERSED));
        }
        assert_eq!(buffer[(1, 2)].symbol(), "›");
        assert_eq!(buffer[(3, 2)].symbol(), "*");
    }

    #[test]
    fn long_names_stay_distinct_and_paths_keep_their_tail() {
        let mut state = state(&[
            (
                LONG,
                "/wt/acme/feature-JIRA-12345-implement-the-thing-nobody-asked-for",
            ),
            (
                LONGER,
                "/wt/acme/feature-JIRA-12345-implement-the-thing-nobody-asked-for-v2",
            ),
        ]);
        let screen = screen(&mut state, 60, 12);

        assert_eq!(
            screen[3],
            "     feature/JIRA…y-asked-for  …-the-thing-nobody-asked-for"
        );
        assert_eq!(
            screen[4],
            "     feature/JIRA…sked-for-v2  …e-thing-nobody-asked-for-v2"
        );
    }

    #[test]
    fn detached_and_missing_worktrees_are_named_and_tagged() {
        let mut state = state(&[("stale/gone", "/wt/acme/stale-gone")]);
        state.identities[1].missing = true;
        state.identities.push(WorktreeIdentity {
            id: WorktreeId::new("/wt/acme/spike"),
            worktree: "detached@73c5694".to_string(),
            branch: None,
            detached: true,
            ..identity("spike", "/wt/acme/spike")
        });
        let screen = screen(&mut state, 80, 12);

        assert_eq!(
            screen[3],
            "     stale/gone        /wt/acme/stale-gone  missing"
        );
        assert_eq!(screen[4], "     detached@73c5694  /wt/acme/spike");
    }

    #[test]
    fn a_repository_with_only_its_main_worktree_says_how_to_create_one() {
        let mut state = state(&[]);
        let screen = screen(&mut state, 60, 12);

        assert_eq!(
            screen[0].trim_end(),
            " trench  acme                                    1 worktree"
        );
        assert_eq!(screen[2], " › * main    /code/acme");
        assert_eq!(
            screen[4],
            "     No other worktrees yet. Press c to create one."
        );
    }

    #[test]
    fn keys_that_do_nothing_for_the_selection_dim_in_place() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let on_main = draw(&mut state, 80, 10, "terminal");
        let remove = (41..49).collect::<Vec<u16>>();
        for x in &remove {
            assert!(on_main[(*x, 9)].modifier.contains(Modifier::DIM), "{x}");
        }
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let on_feature = draw(&mut state, 80, 10, "terminal");

        assert_eq!(
            crate::tui::view::testing::lines(&on_main)[9],
            crate::tui::view::testing::lines(&on_feature)[9],
            "hints keep their positions"
        );
        assert!(!on_feature[(41, 9)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn narrow_terminals_drop_the_least_needed_hints_first() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);

        assert_eq!(
            screen(&mut state, 60, 10)[9],
            " Enter switch  c create  s sync  d remove  ? help  q quit"
        );
        assert_eq!(
            screen(&mut state, 40, 10)[9],
            " Enter switch  c create  ? help  q quit"
        );
    }

    #[test]
    fn search_replaces_the_header_and_counts_matches() {
        let mut state = state(&[
            ("feature/auth", "/wt/acme/feature-auth"),
            ("fix/typo", "/wt/acme/fix-typo"),
        ]);
        for key in [Key::Char('/'), Key::Char('f'), Key::Char('i')] {
            let _ = reduce(&mut state, Event::Input(key));
        }
        let screen = screen(&mut state, 60, 10);

        assert_eq!(
            screen[0],
            " / fi▌                                               1 of 3"
        );
        assert_eq!(screen[2], " ›   fix/typo  /wt/acme/fix-typo");
        assert_eq!(screen[9], " Enter switch  ↑/↓ move  Esc close search");
    }

    #[test]
    fn search_without_matches_says_so_and_dims_enter() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        for key in [Key::Char('/'), Key::Char('z'), Key::Char('z')] {
            let _ = reduce(&mut state, Event::Input(key));
        }
        let buffer = draw(&mut state, 60, 10, "terminal");
        let screen = crate::tui::view::testing::lines(&buffer);

        assert_eq!(screen[1], "     BRANCH  PATH");
        assert_eq!(screen[2], "     No worktree matches \"zz\".");
        assert!(buffer[(1, 9)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn one_status_line_shows_the_latest_message_with_a_glyph_per_kind() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        state.refresh.warning = Some("Could not fetch origin; showing local refs".to_string());
        assert_eq!(
            screen(&mut state, 60, 10)[8],
            " ! Could not fetch origin; showing local refs"
        );

        for (kind, expected) in [
            (NoticeKind::Success, " ✓ Created feature/login"),
            (NoticeKind::Error, " × Created feature/login"),
            (NoticeKind::Warning, " ! Created feature/login"),
        ] {
            let _ = reduce(
                &mut state,
                Event::NotificationShown {
                    kind,
                    message: "Created feature/login".to_string(),
                    shown_at: Instant::now(),
                },
            );
            assert_eq!(screen(&mut state, 60, 10)[8], expected);
        }
    }

    #[test]
    fn a_long_status_message_takes_a_second_row_before_it_is_cut() {
        let mut state = state(&[
            ("feature/auth", "/wt/acme/feature-auth"),
            ("fix/typo", "/wt/acme/fix-typo"),
        ]);
        let _ = reduce(
            &mut state,
            Event::NotificationShown {
                kind: NoticeKind::Warning,
                message: "Worktree is locked. Run `git worktree unlock ~/.worktrees/acme/fix-typo` first.".to_string(),
                shown_at: Instant::now(),
            },
        );
        let screen = screen(&mut state, 60, 10);

        assert_eq!(
            screen[7..10],
            [
                " ! Worktree is locked. Run `git worktree unlock",
                "   ~/.worktrees/acme/fix-typo` first.",
                " Enter switch  c create  s sync  d remove  ? help  q quit",
            ]
        );
        assert_eq!(screen[4], "     fix/typo      /wt/acme/fix-typo");
    }

    #[test]
    fn a_list_longer_than_the_screen_keeps_the_selection_visible_and_counts_the_rest() {
        let others = (0..12)
            .map(|index| {
                (
                    format!("feature/{index:02}"),
                    format!("/wt/acme/feature-{index:02}"),
                )
            })
            .collect::<Vec<_>>();
        let borrowed = others
            .iter()
            .map(|(branch, path)| (branch.as_str(), path.as_str()))
            .collect::<Vec<_>>();
        let mut state = state(&borrowed);
        assert_eq!(screen(&mut state, 60, 10)[7], "     … 8 more");

        for _ in 0..12 {
            let _ = reduce(&mut state, Event::Input(Key::Down));
        }
        let screen = screen(&mut state, 60, 10);
        assert_eq!(screen[7], " ›   feature/11  /wt/acme/feature-11");
        assert!(screen.iter().all(|row| !row.contains("more")));
    }

    #[test]
    fn fetching_origin_is_shown_beside_the_count() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        state.refresh.updating_refs = true;
        let header = &screen(&mut state, 80, 10)[0];

        assert!(header.starts_with(" trench  acme"), "{header}");
        assert!(header.contains("fetching origin"), "{header}");
        assert!(header.ends_with("2 worktrees"), "{header}");
    }
}
