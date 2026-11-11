//! Pieces every screen shares: the hint bar, the dialog frame, and inputs.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};

use crate::{
    output::text,
    tui::{line_input::LineInput, theme::Theme},
};

pub const CURSOR: &str = "▌";
pub const SELECTED: &str = "›";
pub const OK: &str = "✓";
pub const FAILED: &str = "×";
pub const WARNING: &str = "!";
pub const PENDING: &str = "·";

/// Row tags share one width so they form a left-aligned column.
pub const LOCAL_TAG: &str = "local branch  ";
pub const REMOTE_TAG: &str = "remote branch ";

pub fn ref_tag(kind: crate::ref_catalog::RefKind) -> &'static str {
    match kind {
        crate::ref_catalog::RefKind::Local => LOCAL_TAG,
        crate::ref_catalog::RefKind::Remote => REMOTE_TAG,
    }
}

/// Columns between the screen edge and text on the list screen.
pub const MARGIN: u16 = 1;

/// How a hint is drawn, which says whether its key does anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// The key that removes something.
    Danger,
    /// The key does nothing right now.
    Disabled,
}

/// One `Key label` pair in the hint bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub key: &'static str,
    pub label: String,
    pub tone: Tone,
    /// When the bar is too narrow, hints with higher ranks are dropped first.
    pub rank: u8,
}

impl Hint {
    pub fn new(key: &'static str, label: impl Into<String>, rank: u8) -> Self {
        Self {
            key,
            label: label.into(),
            tone: Tone::Normal,
            rank,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn enabled(self, enabled: bool) -> Self {
        if enabled {
            self
        } else {
            self.tone(Tone::Disabled)
        }
    }

    fn width(&self) -> usize {
        text::width(self.key) + 1 + text::width(&self.label)
    }
}

const HINT_GAP: usize = 2;

/// The hints that fit in `width` columns, in their original order.
///
/// Hints keep their positions: a hint is either shown where it always is or
/// dropped, lowest priority first. A disabled hint dims instead of vanishing,
/// so the bar never reshuffles as the selection moves.
pub fn fit_hints(hints: &[Hint], width: usize) -> Vec<&Hint> {
    let mut shown: Vec<&Hint> = hints.iter().collect();
    let total = |shown: &[&Hint]| {
        shown.iter().map(|hint| hint.width()).sum::<usize>()
            + shown.len().saturating_sub(1) * HINT_GAP
    };
    while total(&shown) > width && shown.len() > 1 {
        let Some(drop) = shown
            .iter()
            .enumerate()
            .max_by_key(|(index, hint)| (hint.rank, *index))
            .map(|(index, _)| index)
        else {
            break;
        };
        shown.remove(drop);
    }
    shown
}

/// Draw the hint bar on the one-row `area`.
pub fn hint_bar(frame: &mut Frame, area: Rect, hints: &[Hint], theme: &Theme) {
    let width = usize::from(area.width.saturating_sub(MARGIN * 2));
    let mut spans = vec![Span::raw(" ".repeat(usize::from(MARGIN)))];
    for (index, hint) in fit_hints(hints, width).into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" ".repeat(HINT_GAP)));
        }
        let (key, label) = match hint.tone {
            Tone::Normal => (theme.text.add_modifier(Modifier::BOLD), theme.muted),
            Tone::Danger => (theme.danger, theme.muted),
            Tone::Disabled => (theme.disabled, theme.disabled),
        };
        spans.push(Span::styled(hint.key, key));
        spans.push(Span::styled(format!(" {}", hint.label), label));
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme.on_surface(theme.text)),
        area,
    );
}

/// Where a dialog of `height` rows sits, and the text area inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogLayout {
    pub outer: Rect,
    /// Inside the border and one column of padding on each side.
    pub content: Rect,
    /// The screen's last row, which holds the dialog's hints.
    pub hints: Rect,
}

/// Dialogs share one width rule so they never jump between screens.
pub fn dialog_layout(area: Rect, height: u16) -> DialogLayout {
    let hints = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    let available = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    let width = if area.width >= 52 {
        (area.width - 4).min(76)
    } else {
        area.width.saturating_sub(2)
    };
    let height = height.min(available.height);
    let outer = Rect::new(
        available.x + (available.width.saturating_sub(width)) / 2,
        available.y + (available.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    let content = Rect::new(
        outer.x + 2,
        outer.y + 1,
        outer.width.saturating_sub(4),
        outer.height.saturating_sub(2),
    );
    DialogLayout {
        outer,
        content,
        hints,
    }
}

/// Draw the dialog frame: a rounded border with the title in its top edge.
pub fn dialog_frame(
    frame: &mut Frame,
    layout: DialogLayout,
    title: &str,
    status: Option<Line<'static>>,
    theme: &Theme,
) {
    // One blank column on each side keeps the dimmed list from touching the
    // border, which would read as text running into the dialog.
    let cleared = Rect::new(
        layout.outer.x.saturating_sub(1),
        layout.outer.y,
        layout.outer.width + 2,
        layout.outer.height,
    )
    .intersection(frame.area());
    frame.render_widget(Clear, cleared);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.on_dialog(theme.border))
        .style(theme.on_dialog(theme.text))
        .title(Line::from(Span::styled(
            format!(" {title} "),
            theme.on_dialog(theme.accent),
        )));
    if let Some(status) = status {
        block = block.title(status.alignment(Alignment::Right));
    }
    frame.render_widget(block, layout.outer);
}

/// A labeled value: `Branch  feature/auth`.
pub fn field<'a>(label: &str, label_width: usize, value: Vec<Span<'a>>, theme: &Theme) -> Line<'a> {
    let mut spans = vec![Span::styled(
        format!("{}  ", text::pad_end(label, label_width)),
        theme.muted,
    )];
    spans.extend(value);
    Line::from(spans)
}

/// A single-line text input with a block cursor, or its placeholder.
///
/// Without focus the value is plain text and no cursor is drawn, so exactly
/// one field on screen looks editable.
pub fn input<'a>(
    input: &LineInput,
    placeholder: &'a str,
    width: usize,
    focused: bool,
    theme: &Theme,
) -> Vec<Span<'a>> {
    if !focused {
        return vec![Span::styled(
            text::truncate_end(input.value(), width),
            theme.text,
        )];
    }
    let width = width.saturating_sub(1);
    if input.value().is_empty() {
        return vec![
            Span::styled(CURSOR, theme.accent),
            Span::styled(text::truncate_end(placeholder, width), theme.muted),
        ];
    }
    let window = input.window(width);
    vec![
        Span::styled(window.before_cursor, theme.text),
        Span::styled(CURSOR, theme.accent),
        Span::styled(window.after_cursor, theme.text),
    ]
}

/// The rows of a list that fit, keeping the selection visible.
///
/// When rows are cut off below, the last slot says how many (`more`), so a
/// list never ends without saying it continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub rows: std::ops::Range<usize>,
    pub more: usize,
}

pub fn window(total: usize, selected: Option<usize>, capacity: usize) -> Window {
    if total <= capacity {
        return Window {
            rows: 0..total,
            more: 0,
        };
    }
    if capacity == 0 {
        return Window {
            rows: 0..0,
            more: total,
        };
    }
    // One slot is reserved for the "N more" line unless the window reaches
    // the end of the list.
    let visible = capacity.saturating_sub(1).max(1);
    let selected = selected.unwrap_or(0).min(total - 1);
    let mut start = selected.saturating_sub(visible - 1);
    if start + capacity >= total {
        start = total - capacity;
        return Window {
            rows: start..total,
            more: 0,
        };
    }
    Window {
        rows: start..start + visible,
        more: total - (start + visible),
    }
}

/// A selectable row padded to `width`, with an optional right-hand tag.
pub fn row<'a>(
    label: String,
    tag: Option<(&'a str, Style)>,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'a> {
    let marker = if selected { SELECTED } else { " " };
    let tag_width = tag.map_or(0, |(tag, _)| text::width(tag) + 2);
    let label_width = width.saturating_sub(2 + tag_width);
    let label = text::pad_end(&text::truncate_middle(&label, label_width), label_width);
    let base = if selected {
        theme.selection
    } else {
        theme.text
    };
    let mut spans = vec![Span::styled(format!("{marker} {label}"), base)];
    if let Some((tag, style)) = tag {
        spans.push(Span::styled(
            format!("  {tag}"),
            if selected { theme.selection } else { style },
        ));
    }
    Line::from(spans)
}

/// Wrap `value` to at most `max_lines` lines. Text that does not fit ends in
/// an ellipsis, so a cut is always visible.
pub fn wrap_clamped(value: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = wrap(value, width);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            *last = text::truncate_end(&format!("{last}…"), width);
        }
    }
    lines
}

/// Wrap `value` to `width` columns at spaces, breaking long words.
pub fn wrap(value: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    for word in value.split_whitespace() {
        // `rest` is a slice of the word, and widths are tracked as numbers,
        // so a word of any length is walked once.
        let mut rest = word;
        let mut rest_width = text::width(rest);
        loop {
            let separator = usize::from(!line.is_empty());
            if line_width + separator + rest_width <= width {
                if separator == 1 {
                    line.push(' ');
                }
                line.push_str(rest);
                line_width += separator + rest_width;
                break;
            }
            if line.is_empty() {
                // The word alone is wider than the line: split it.
                let (head, tail) = text::split_at(rest, width);
                rest_width -= text::width(head);
                lines.push(head.to_string());
                rest = tail;
                if rest.is_empty() {
                    break;
                }
            } else {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(hints: &[&Hint]) -> Vec<&'static str> {
        hints.iter().map(|hint| hint.key).collect()
    }

    #[test]
    fn hints_drop_by_rank_and_keep_their_order() {
        let hints = [
            Hint::new("Enter", "switch", 0),
            Hint::new("o", "open", 6),
            Hint::new("c", "create", 1),
            Hint::new("d", "remove", 4),
            Hint::new("?", "help", 2),
            Hint::new("q", "quit", 3),
        ];

        assert_eq!(
            labels(&fit_hints(&hints, 80)),
            ["Enter", "o", "c", "d", "?", "q"]
        );
        assert_eq!(
            labels(&fit_hints(&hints, 50)),
            ["Enter", "c", "d", "?", "q"]
        );
        assert_eq!(labels(&fit_hints(&hints, 38)), ["Enter", "c", "?", "q"]);
        assert_eq!(labels(&fit_hints(&hints, 30)), ["Enter", "c", "?"]);
        assert_eq!(labels(&fit_hints(&hints, 3)), ["Enter"]);
    }

    #[test]
    fn dialogs_use_one_width_rule_and_leave_the_last_row_for_hints() {
        let wide = dialog_layout(Rect::new(0, 0, 120, 40), 15);
        assert_eq!(wide.outer.width, 76);
        assert_eq!(wide.content.width, 72);
        assert_eq!(wide.hints, Rect::new(0, 39, 120, 1));

        let medium = dialog_layout(Rect::new(0, 0, 60, 20), 15);
        assert_eq!(medium.outer.width, 56);
        assert_eq!(medium.outer.x, 2);

        let small = dialog_layout(Rect::new(0, 0, 40, 10), 15);
        assert_eq!(small.outer, Rect::new(1, 0, 38, 9));
        assert_eq!(small.content.width, 34);
    }

    #[test]
    fn window_keeps_the_selection_visible_and_counts_hidden_rows() {
        assert_eq!(
            window(3, Some(2), 5),
            Window {
                rows: 0..3,
                more: 0
            }
        );
        assert_eq!(
            window(9, Some(0), 4),
            Window {
                rows: 0..3,
                more: 6
            }
        );
        assert_eq!(
            window(9, Some(5), 4),
            Window {
                rows: 3..6,
                more: 3
            }
        );
        assert_eq!(
            window(9, Some(8), 4),
            Window {
                rows: 5..9,
                more: 0
            }
        );
        for selected in 0..9 {
            let window = window(9, Some(selected), 4);
            assert!(window.rows.contains(&selected), "{selected}: {window:?}");
        }
    }

    #[test]
    fn clamped_wrapping_marks_the_cut_and_never_exceeds_the_width() {
        assert_eq!(
            wrap_clamped("one two three four five six", 9, 2),
            ["one two", "three…"]
        );
        assert_eq!(wrap_clamped("alpha bravo charlie", 11, 1), ["alpha brav…"]);
        assert_eq!(wrap_clamped("fits", 10, 2), ["fits"]);
        for line in wrap_clamped("alpha bravo charlie delta", 11, 2) {
            assert!(text::width(&line) <= 11, "{line}");
        }
    }

    #[test]
    fn wrapping_one_enormous_word_takes_time_in_proportion_to_its_length() {
        // A hook can print megabytes without a newline or a space. Wrapping
        // must not copy the remainder for every line it produces.
        let word = "x".repeat(1_000_000);
        let started = std::time::Instant::now();

        let lines = wrap(&word, 50);

        assert_eq!(lines.len(), 20_000);
        assert!(lines.iter().all(|line| line.len() == 50));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn wrap_breaks_at_spaces_and_splits_words_longer_than_a_line() {
        assert_eq!(
            wrap(
                "post_create hook failed: `bun install` exited with code 1",
                24
            ),
            [
                "post_create hook failed:",
                "`bun install` exited",
                "with code 1"
            ]
        );
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 10), Vec::<String>::new());
        for line in wrap("a/very/long/path/without/any/spaces and more", 9) {
            assert!(text::width(&line) <= 9, "{line}");
        }
    }
}
