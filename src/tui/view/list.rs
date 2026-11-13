//! The main screen: the worktree list and an info panel in bordered panes,
//! a status line while there is a message, and one hint bar.

use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::{
    output::text,
    paths,
    tui::{
        app::{unavailable_reason, AppState, NoticeKind, Viewport, WorktreeIdentity},
        keymap::{self, Action, Context},
        theme::Theme,
        view::widgets::{self, Hint, Tone, MARGIN},
    },
};

const MISSING_TAG: &str = "  missing";
/// The list's share of a wide terminal, in percent. The info panel takes the
/// rest.
const LIST_SHARE: u16 = 62;
const SEARCH_HEIGHT: u16 = 3;
/// The widest label in the info panel: `Branch`, `Status`.
const INFO_LABEL_WIDTH: usize = 6;
/// Selection cursor, a space, the current-worktree marker, a space.
const GUTTER: usize = 4;
const COLUMN_GAP: usize = 2;

pub fn render(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    let last = area.height - 1;
    // A message takes rows above the hint bar only while it is shown. One
    // too long for a row wraps onto a second instead of being cut.
    let status = status(state, area.width, theme);
    let status_rows = status.len() as u16;
    let mut body = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(1 + status_rows),
    );
    let visible = state.visible_identities();

    if let Some(query) = state.search.as_ref() {
        let search = Rect::new(body.x, body.y, body.width, SEARCH_HEIGHT.min(body.height));
        render_search(
            query,
            visible.len(),
            state.identities.len(),
            frame,
            search,
            theme,
        );
        body = Rect::new(
            body.x,
            body.y + search.height,
            body.width,
            body.height.saturating_sub(search.height),
        );
    }
    match (state.info_visible(), area.width >= Viewport::WIDE_WIDTH) {
        (true, true) => {
            // In u32: a terminal over 1057 columns overflows u16 here.
            let list_width = (u32::from(body.width) * u32::from(LIST_SHARE) / 100) as u16;
            let list = Rect::new(body.x, body.y, list_width, body.height);
            let info = Rect::new(
                body.x + list_width,
                body.y,
                body.width - list_width,
                body.height,
            );
            render_list(state, &visible, frame, list, theme);
            render_info(state.selected_visible(), frame, info, theme);
        }
        (true, false) => render_info(state.selected_visible(), frame, body, theme),
        (false, _) => render_list(state, &visible, frame, body, theme),
    }

    if !status.is_empty() {
        frame.render_widget(
            Paragraph::new(status),
            Rect::new(area.x, last - status_rows, area.width, status_rows),
        );
    }
    widgets::hint_bar(
        frame,
        Rect::new(area.x, area.y + last, area.width, 1),
        &hints(state, visible.is_empty()),
        theme,
    );
}

/// A bordered pane with `title` centered in its top edge.
fn pane<'a>(title: Line<'a>, theme: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme.on_surface(theme.border))
        .title(title.alignment(Alignment::Center))
}

fn render_list(
    state: &AppState,
    visible: &[&WorktreeIdentity],
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
) {
    let total = state.identities.len();
    let room = usize::from(area.width).saturating_sub(4);
    let mut title = vec![Span::styled(
        format!(
            " {} ",
            text::truncate_end(&state.repository, room.saturating_sub(16))
        ),
        theme.accent,
    )];
    title.push(Span::styled(
        if total == 1 {
            "· 1 worktree ".to_string()
        } else {
            format!("· {total} worktrees ")
        },
        theme.muted,
    ));
    if state.refresh.updating_refs {
        title.push(Span::styled(
            format!("· fetching origin {} ", spinner(state.refresh.spinner_tick)),
            theme.muted,
        ));
    }
    let block = pane(Line::from(title), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let home = paths::home_dir();
    let columns = Columns::new(visible, home.as_deref(), inner.width);
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
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let rows = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    frame.render_widget(
        Paragraph::new(body_lines(state, visible, &columns, rows, theme)),
        rows,
    );
}

/// The search field, in its own pane above the list.
fn render_search(
    query: &crate::tui::search::QueryBuffer,
    matches: usize,
    total: usize,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
) {
    let block = pane(Line::styled(" Search ", theme.accent), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let count = format!("{matches} of {total}");
    let room = usize::from(inner.width).saturating_sub(text::width(&count) + 6);
    let mut spans = vec![Span::styled(" / ", theme.accent)];
    spans.extend(widgets::input(
        query.input(),
        "type to filter worktrees",
        room,
        true,
        theme,
    ));
    let used: usize = spans.iter().map(|span| text::width(&span.content)).sum();
    let gap = usize::from(inner.width)
        .saturating_sub(used + text::width(&count) + usize::from(MARGIN))
        .max(1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(count, theme.muted));
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// What is known about the selected worktree, with nothing cut: the path
/// wraps.
fn render_info(identity: Option<&WorktreeIdentity>, frame: &mut Frame, area: Rect, theme: &Theme) {
    let room = usize::from(area.width).saturating_sub(4);
    let title = match identity {
        Some(identity) => {
            let suffix = if identity.is_current {
                " · current"
            } else {
                ""
            };
            let name =
                text::truncate_middle(&identity.worktree, room.saturating_sub(text::width(suffix)));
            Line::from(vec![
                Span::styled(format!(" {name}"), theme.accent),
                Span::styled(format!("{suffix} "), theme.muted),
            ])
        }
        None => Line::styled(" Info ", theme.accent),
    };
    let block = pane(title, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(identity) = identity else {
        frame.render_widget(
            Paragraph::new(Line::styled(" No worktree selected.", theme.muted)),
            inner,
        );
        return;
    };

    let pad = " ".repeat(usize::from(MARGIN));
    let value_room =
        usize::from(inner.width).saturating_sub(usize::from(MARGIN) * 2 + INFO_LABEL_WIDTH + 2);
    let field = |label: &str, value: String, style: Style| {
        let mut line = widgets::field(
            label,
            INFO_LABEL_WIDTH,
            vec![Span::styled(value, style)],
            theme,
        );
        line.spans.insert(0, Span::raw(pad.clone()));
        line
    };
    // A value on several rows: the label on the first, the rest indented
    // under the value.
    let multi = |label: &str, parts: Vec<String>, style: Style| {
        parts
            .into_iter()
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    field(label, part, style)
                } else {
                    Line::styled(
                        format!("{pad}{}{part}", " ".repeat(INFO_LABEL_WIDTH + 2)),
                        style,
                    )
                }
            })
            .collect::<Vec<_>>()
    };

    let (branch, branch_style) = match identity.branch.as_deref() {
        Some(branch) => (branch.to_string(), theme.text),
        None => ("detached".to_string(), theme.muted),
    };
    let path = identity.path.to_string_lossy();
    // HEAD and the status always keep their rows. The branch and the path
    // share the rest; when they do not fit, each is cut with an ellipsis
    // where the cut is, never silently.
    let fixed = 1 + usize::from(identity.missing);
    let shared = usize::from(inner.height).saturating_sub(fixed);
    let branch_needs = rows_of(&branch, value_room).len();
    let path_needs = rows_of(&path, value_room).len();
    let blank = shared > branch_needs + path_needs;
    let shared = shared - usize::from(blank);
    // The branch gets a row before the path, and leaves the path one when
    // there are two. With no rows left, neither is shown.
    let branch_rows = match shared {
        0 => 0,
        1 => 1,
        _ => branch_needs.min(shared - 1),
    };
    let path_rows = path_needs.min(shared - branch_rows);

    let mut lines = Vec::new();
    if blank {
        lines.push(Line::default());
    }
    lines.extend(multi(
        "Branch",
        fit(&branch, value_room, branch_rows, text::truncate_middle),
        branch_style,
    ));
    lines.extend(multi(
        "Path",
        fit(&path, value_room, path_rows, text::truncate_start),
        theme.text,
    ));
    lines.push(match identity.head.as_deref() {
        Some(head) => field("HEAD", head.chars().take(7).collect(), theme.text),
        None => field("HEAD", "none".to_string(), theme.muted),
    });
    if identity.missing {
        lines.push(field(
            "Status",
            "directory missing".to_string(),
            theme.warning,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `value` split into rows `width` columns wide. Every character is kept,
/// spaces included, because a path or a branch must read exactly.
fn rows_of(value: &str, width: usize) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;

    let width = width.max(1);
    let mut rows = vec![String::new()];
    // By grapheme, so a character built from several code points, such as
    // a joined emoji or a letter with a combining accent, stays whole.
    for grapheme in value.graphemes(true) {
        let row = rows.last_mut().expect("starts with one row");
        if !row.is_empty() && text::width(row) + text::width(grapheme) > width {
            rows.push(grapheme.to_string());
        } else {
            row.push_str(grapheme);
        }
    }
    rows
}

/// `value` on at most `rows` rows. A value that needs more is cut by `cut`
/// to what the rows hold, so the ellipsis shows where the cut is.
fn fit(value: &str, width: usize, rows: usize, cut: fn(&str, usize) -> String) -> Vec<String> {
    let all = rows_of(value, width);
    if all.len() <= rows {
        return all;
    }
    if rows == 0 {
        return Vec::new();
    }
    // A wide character that does not fit at the end of a row moves to the
    // next one and leaves a column unused, so the cut can need more rows
    // than `width * rows` suggests. Cut shorter until it fits.
    let mut columns = width.max(1) * rows;
    loop {
        let parts = rows_of(&cut(value, columns), width);
        if parts.len() <= rows || columns <= 1 {
            return parts;
        }
        columns -= 1;
    }
}

/// Column widths for the rows on screen.
struct Columns {
    branch: usize,
    /// Each row's branch and path, already cut to their columns.
    branches: Vec<String>,
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
        let path = content.saturating_sub(branch + COLUMN_GAP);
        let labels = rows
            .iter()
            .map(|row| row.label().to_string())
            .collect::<Vec<_>>();
        let paths = rows
            .iter()
            .map(|row| text::home_relative(&row.path, home))
            .collect::<Vec<_>>();
        let path_widths = rows
            .iter()
            .map(|row| {
                let tag = if row.missing { MISSING_TAG } else { "" };
                path.saturating_sub(text::width(tag))
            })
            .collect::<Vec<_>>();
        Self {
            branch,
            branches: distinct(&labels, &vec![branch; rows.len()], text::truncate_middle),
            paths: distinct(&paths, &path_widths, text::truncate_start),
        }
    }
}

/// `values` cut to `widths` by `cut`. Two different values that come out
/// the same, because they differ only where the cut fell, are cut again
/// to show the part where they differ: `…ABC-123-add…`.
fn distinct(values: &[String], widths: &[usize], cut: fn(&str, usize) -> String) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;

    let shown = values
        .iter()
        .zip(widths)
        .map(|(value, width)| cut(value, *width))
        .collect::<Vec<_>>();
    let common_prefix = |left: &str, right: &str| {
        left.graphemes(true)
            .zip(right.graphemes(true))
            .take_while(|(left, right)| left == right)
            .count()
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let width = widths[index];
            let differs_at = (0..values.len())
                .filter(|&other| {
                    other != index && shown[other] == shown[index] && values[other] != *value
                })
                .map(|other| common_prefix(value, &values[other]))
                .min();
            match differs_at {
                Some(at) if width >= 2 => {
                    // Two characters of context before the difference.
                    let start = at.saturating_sub(2);
                    if start == 0 {
                        text::truncate_end(value, width)
                    } else {
                        let rest = value.graphemes(true).skip(start).collect::<String>();
                        format!("{}{}", text::ELLIPSIS, text::truncate_end(&rest, width - 1))
                    }
                }
                _ => shown[index].clone(),
            }
        })
        .collect()
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
                &columns.branches[index],
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
    branch: &str,
    path: &str,
    columns: &Columns,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let cursor = if selected { widgets::SELECTED } else { " " };
    let current = if identity.is_current { "*" } else { " " };
    let branch = text::pad_end(branch, columns.branch);
    let tag = if identity.missing { MISSING_TAG } else { "" };
    let path = path.to_string();
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

    /// The rows inside the left pane, without its border.
    fn inner(screen: &[String]) -> Vec<String> {
        screen
            .iter()
            .map(|row| {
                let row = row.strip_prefix('│').unwrap_or(row);
                row.split('│').next().unwrap_or("").trim_end().to_string()
            })
            .collect()
    }

    const LONG: &str = "feature/JIRA-12345-implement-the-thing-nobody-asked-for";
    const LONGER: &str = "feature/JIRA-12345-implement-the-thing-nobody-asked-for-v2";

    #[test]
    fn a_wide_terminal_shows_the_list_and_an_info_panel_in_bordered_panes() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let screen = screen(&mut state, 100, 12);

        assert!(screen[0].starts_with("┌"), "{screen:#?}");
        assert!(screen[0].contains(" acme · 2 worktrees "), "{screen:#?}");
        assert!(screen[0].contains(" main · current "), "{screen:#?}");
        assert!(screen[10].starts_with("└") && screen[10].ends_with("┘"));
        let info = |row: usize| {
            screen[row]
                .split('│')
                .nth(3)
                .unwrap_or("")
                .trim_end()
                .to_string()
        };
        assert_eq!(info(2), " Branch  main");
        assert_eq!(info(3), " Path    /code/acme");
        assert_eq!(info(4), " HEAD    1234567");
        assert!(screen[11].contains("i info"), "{}", screen[11]);
    }

    #[test]
    fn the_info_panel_follows_the_selection_and_says_when_the_directory_is_missing() {
        let mut state = state(&[("stale/gone", "/wt/acme/stale-gone")]);
        state.identities[1].missing = true;
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let screen = screen(&mut state, 100, 12).join("\n");

        assert!(screen.contains(" stale-gone "), "{screen}");
        assert!(!screen.contains("current"), "{screen}");
        assert!(screen.contains("Branch  stale/gone"), "{screen}");
        assert!(screen.contains("Status  directory missing"), "{screen}");
    }

    #[test]
    fn a_long_path_wraps_in_the_info_panel_instead_of_being_cut() {
        let path = "/wt/acme/feature-JIRA-12345-implement-the-thing-nobody-asked-for";
        let mut state = state(&[(LONG, path)]);
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let screen = screen(&mut state, 100, 14);
        let panel = screen
            .iter()
            .map(|row| row.split('│').nth(3).unwrap_or("").trim().to_string())
            .collect::<Vec<_>>()
            .join("");

        assert!(
            panel.replace(' ', "").contains(&format!("Path{path}")),
            "the whole path is in the panel: {screen:#?}"
        );
    }

    /// The info panel's rows, without its border.
    fn panel(screen: &[String]) -> Vec<String> {
        screen
            .iter()
            .map(|row| row.split('│').nth(3).unwrap_or("").trim_end().to_string())
            .collect()
    }

    const ENDLESS: &str = "feature/abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcdefghij-v2";

    #[test]
    fn a_long_branch_wraps_in_the_info_panel_instead_of_being_cut() {
        let mut state = state(&[(ENDLESS, "/wt/acme/endless")]);
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let rows = panel(&screen(&mut state, 100, 24));
        let branch = rows
            .iter()
            .skip_while(|row| !row.starts_with(" Branch"))
            .take_while(|row| !row.starts_with(" Path"))
            .map(|row| row.trim_start_matches(" Branch").trim())
            .collect::<String>();

        assert_eq!(branch, ENDLESS, "{rows:#?}");
    }

    #[test]
    fn a_short_panel_keeps_head_and_status_and_shows_where_the_path_was_cut() {
        let path = format!("/wt/acme/feature-{}-v2", "abcdefghij".repeat(10));
        let mut state = state(&[("feature/long", path.as_str())]);
        state.identities[1].missing = true;
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let rows = panel(&screen(&mut state, 100, 10));
        let text = rows.join("\n");

        assert!(text.contains(" HEAD    1234567"), "{text}");
        assert!(text.contains(" Status  directory missing"), "{text}");
        let path_rows = rows
            .iter()
            .skip_while(|row| !row.starts_with(" Path"))
            .take_while(|row| !row.starts_with(" HEAD"))
            .map(|row| row.trim_start_matches(" Path").trim())
            .collect::<String>();
        assert!(path_rows.starts_with('…'), "{text}");
        assert!(
            path_rows.ends_with("abcdefghij-v2"),
            "the tail names the directory: {text}"
        );
    }

    #[test]
    fn the_info_panel_shows_a_path_exactly_even_with_repeated_spaces() {
        let mut state = state(&[("spaces", "/tmp/a  b/spaces")]);
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let text = panel(&screen(&mut state, 100, 24)).join("\n");

        assert!(text.contains(" Path    /tmp/a  b/spaces"), "{text}");
    }

    #[test]
    fn wide_characters_never_push_head_out_of_the_info_panel() {
        let wide = "日".repeat(70);
        let mut state = state(&[(wide.as_str(), "/wt/acme/wide")]);
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let rows = panel(&screen(&mut state, 102, 10));

        assert!(
            rows.iter().any(|row| row.starts_with(" HEAD    1234567")),
            "{rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.contains('…')),
            "the cut is shown: {rows:#?}"
        );
    }

    #[test]
    fn a_panel_with_room_for_two_rows_keeps_head_and_status() {
        let mut state = state(&[("stale/gone", "/wt/acme/stale-gone")]);
        state.identities[1].missing = true;
        state.refresh.warning = Some(
            "Could not fetch origin; showing local refs. Check your network or the remote URL."
                .to_string(),
        );
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let _ = reduce(
            &mut state,
            Event::ViewportChanged {
                width: 40,
                height: 10,
            },
        );
        let _ = reduce(&mut state, Event::Input(Key::Char('i')));
        let _ = reduce(&mut state, Event::Input(Key::Char('/')));
        let screen = screen(&mut state, 40, 10);
        let text = screen.join("\n");

        assert!(text.contains("HEAD    1234567"), "{text}");
        assert!(text.contains("Status  directory missing"), "{text}");
    }

    #[test]
    fn the_info_panel_never_splits_a_joined_emoji_across_rows() {
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        let branch = format!("{}{family}-topic", "a".repeat(24));

        assert!(super::rows_of(&branch, 26)
            .iter()
            .any(|row| row.contains(family)));
        let rows = super::rows_of(&branch, 25);
        assert!(rows.iter().any(|row| row.contains(family)), "{rows:?}");
        assert_eq!(rows.concat(), branch, "every character is kept");
    }

    #[test]
    fn a_very_wide_terminal_splits_the_panes_without_overflowing() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let screen = screen(&mut state, 1100, 10);

        // 62% of 1100 columns: the list's right border sits at column 681.
        let top = screen[0].chars().collect::<Vec<_>>();
        assert_eq!(top[681], '┐', "{}", screen[0]);
        assert_eq!(top[682], '┌', "{}", screen[0]);
    }

    #[test]
    fn i_toggles_the_info_panel_and_a_narrow_terminal_starts_without_it() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let narrow = screen(&mut state, 80, 12);
        assert!(!narrow.join("\n").contains("HEAD"), "{narrow:#?}");
        assert!(narrow[1].contains("BRANCH"));

        // On a narrow terminal the panel takes the list's place.
        let _ = reduce(&mut state, Event::Input(Key::Char('i')));
        let shown = screen(&mut state, 80, 12).join("\n");
        assert!(shown.contains("HEAD    1234567"), "{shown}");
        assert!(!shown.contains("BRANCH"), "{shown}");

        let _ = reduce(&mut state, Event::Input(Key::Char('i')));
        assert!(!screen(&mut state, 80, 12).join("\n").contains("HEAD"));

        // The choice holds when the terminal grows.
        let wide = screen(&mut state, 120, 12).join("\n");
        assert!(!wide.contains("HEAD"), "{wide}");
    }

    #[test]
    fn list_shows_branch_and_path_behind_a_fixed_gutter() {
        let mut state = state(&[
            ("feature/auth", "/wt/acme/feature-auth"),
            ("fix/typo", "/wt/acme/fix-typo"),
        ]);
        let _ = reduce(&mut state, Event::Input(Key::Down));
        let screen = screen(&mut state, 80, 12);

        assert!(screen[0].contains(" acme · 3 worktrees "), "{}", screen[0]);
        assert_eq!(
            inner(&screen)[1..5],
            [
                "     BRANCH        PATH",
                "   * main          /code/acme",
                " ›   feature/auth  /wt/acme/feature-auth",
                "     fix/typo      /wt/acme/fix-typo",
            ]
        );
        assert_eq!(
            screen[11],
            " Enter switch  o open  c create  s sync  d remove  i info  ? help  q quit"
        );
    }

    #[test]
    fn selected_row_is_a_full_width_bar_and_the_current_marker_is_not_color_alone() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        let buffer = draw(&mut state, 60, 10, "terminal");

        // The bar fills the pane between its borders.
        for x in 1..59 {
            assert!(
                buffer[(x, 2)].modifier.contains(Modifier::REVERSED),
                "column {x}"
            );
            assert!(!buffer[(x, 3)].modifier.contains(Modifier::REVERSED));
        }
        assert!(!buffer[(0, 2)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buffer[(2, 2)].symbol(), "›");
        assert_eq!(buffer[(4, 2)].symbol(), "*");
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
        let screen = inner(&screen(&mut state, 60, 12));

        assert_eq!(
            screen[3],
            "     feature/JIR…y-asked-for  …the-thing-nobody-asked-for"
        );
        assert_eq!(
            screen[4],
            "     feature/JIR…sked-for-v2  …-thing-nobody-asked-for-v2"
        );
    }

    #[test]
    fn names_that_differ_only_in_the_middle_still_look_different() {
        let mut state = state(&[
            (
                "feature/ABC-123-add-login-page",
                "/wt/acme/feature-ABC-123-add-login-page",
            ),
            (
                "feature/DEF-123-add-login-page",
                "/wt/acme/feature-DEF-123-add-login-page",
            ),
        ]);
        let rows = inner(&screen(&mut state, 40, 10));

        assert_ne!(rows[3], rows[4], "{rows:#?}");
        for (row, ticket) in [(&rows[3], "ABC"), (&rows[4], "DEF")] {
            let (branch, path) = row.trim().split_once("  ").unwrap();
            assert!(branch.contains(ticket), "{rows:#?}");
            assert!(path.trim().contains(ticket), "{rows:#?}");
        }
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
        let screen = inner(&screen(&mut state, 80, 12));

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

        assert!(screen[0].contains(" acme · 1 worktree "), "{}", screen[0]);
        let rows = inner(&screen);
        assert_eq!(rows[2], " › * main    /code/acme");
        assert_eq!(
            rows[4],
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
        let rows = inner(&screen);

        assert!(screen[0].contains(" Search "), "{}", screen[0]);
        assert_eq!(
            rows[1],
            " / fi▌                                             1 of 3"
        );
        assert!(screen[3].contains(" acme · 3 worktrees "), "{}", screen[3]);
        assert_eq!(rows[5], " ›   fix/typo  /wt/acme/fix-typo");
        assert_eq!(screen[9], " Enter switch  ↑/↓ move  Esc close search");
    }

    #[test]
    fn search_without_matches_says_so_and_dims_enter() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        for key in [Key::Char('/'), Key::Char('z'), Key::Char('z')] {
            let _ = reduce(&mut state, Event::Input(key));
        }
        let buffer = draw(&mut state, 60, 10, "terminal");
        let screen = inner(&crate::tui::view::testing::lines(&buffer));

        assert_eq!(screen[4], "     BRANCH  PATH");
        assert_eq!(screen[5], "     No worktree matches \"zz\".");
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
        assert_eq!(inner(&screen)[4], "     fix/typo      /wt/acme/fix-typo");
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
        assert_eq!(inner(&screen(&mut state, 60, 10))[7], "     … 8 more");

        for _ in 0..12 {
            let _ = reduce(&mut state, Event::Input(Key::Down));
        }
        let screen = screen(&mut state, 60, 10);
        assert_eq!(inner(&screen)[7], " ›   feature/11  /wt/acme/feature-11");
        assert!(screen.iter().all(|row| !row.contains("more")));
    }

    #[test]
    fn fetching_origin_is_shown_beside_the_count() {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        state.refresh.updating_refs = true;
        let title = &screen(&mut state, 80, 10)[0];

        assert!(
            title.contains(" acme · 2 worktrees · fetching origin "),
            "{title}"
        );
    }
}
