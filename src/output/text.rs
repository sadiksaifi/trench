//! Display-width aware text helpers shared by the CLI and the TUI.
//!
//! Widths are terminal columns, not bytes or `char`s, so wide and combining
//! characters in branch names keep columns aligned.

use std::path::Path;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const ELLIPSIS: &str = "…";

/// Terminal columns occupied by `value`.
pub fn width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

/// Pad `value` with spaces on the right up to `columns`.
pub fn pad_end(value: &str, columns: usize) -> String {
    let padding = columns.saturating_sub(width(value));
    format!("{value}{}", " ".repeat(padding))
}

/// Keep the start of `value`: `feature/a-very-lo…`.
pub fn truncate_end(value: &str, columns: usize) -> String {
    if width(value) <= columns {
        return value.to_string();
    }
    match columns {
        0 => String::new(),
        1 => ELLIPSIS.to_string(),
        _ => format!("{}{ELLIPSIS}", take_start(value, columns - 1)),
    }
}

/// Keep the end of `value`: `…/acme/feature-login`. Paths differ at the end.
pub fn truncate_start(value: &str, columns: usize) -> String {
    if width(value) <= columns {
        return value.to_string();
    }
    match columns {
        0 => String::new(),
        1 => ELLIPSIS.to_string(),
        _ => format!("{ELLIPSIS}{}", take_end(value, columns - 1)),
    }
}

/// Keep both ends of `value`: `feature/JIRA-1…asked-for`. Branch names often
/// share a prefix and differ at the end, so neither end alone identifies one.
pub fn truncate_middle(value: &str, columns: usize) -> String {
    if width(value) <= columns {
        return value.to_string();
    }
    match columns {
        0 => String::new(),
        1 => ELLIPSIS.to_string(),
        _ => {
            let budget = columns - 1;
            let head = budget.div_ceil(2);
            let tail = budget - head;
            format!(
                "{}{ELLIPSIS}{}",
                take_start(value, head),
                take_end(value, tail)
            )
        }
    }
}

/// Split `value` so the first part is at most `columns` wide.
///
/// The first part is never empty for a non-empty value, so repeated splitting
/// always terminates, even when one character is wider than `columns`.
pub fn split_at(value: &str, columns: usize) -> (&str, &str) {
    let mut used = 0;
    let mut end = 0;
    for (index, grapheme) in value.grapheme_indices(true) {
        let grapheme_width = width(grapheme);
        if used + grapheme_width > columns && index > 0 {
            break;
        }
        used += grapheme_width;
        end = index + grapheme.len();
    }
    value.split_at(end)
}

/// Render `path` with the home directory written as `~`.
pub fn home_relative(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn take_start(value: &str, columns: usize) -> String {
    let mut used = 0;
    let mut taken = String::new();
    for grapheme in value.graphemes(true) {
        let grapheme_width = width(grapheme);
        if used + grapheme_width > columns {
            break;
        }
        taken.push_str(grapheme);
        used += grapheme_width;
    }
    taken
}

fn take_end(value: &str, columns: usize) -> String {
    let mut used = 0;
    let mut taken = Vec::new();
    for grapheme in value.graphemes(true).rev() {
        let grapheme_width = width(grapheme);
        if used + grapheme_width > columns {
            break;
        }
        taken.push(grapheme);
        used += grapheme_width;
    }
    taken.reverse();
    taken.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_values_are_returned_unchanged() {
        for truncate in [truncate_end, truncate_start, truncate_middle] {
            assert_eq!(truncate("main", 4), "main");
            assert_eq!(truncate("main", 40), "main");
        }
    }

    #[test]
    fn each_truncation_keeps_the_end_it_names_and_fits_the_width() {
        let branch = "feature/JIRA-12345-nobody-asked-for";
        assert_eq!(truncate_end(branch, 12), "feature/JIR…");
        assert_eq!(truncate_start(branch, 12), "…y-asked-for");
        assert_eq!(truncate_middle(branch, 12), "featur…d-for");
        for truncate in [truncate_end, truncate_start, truncate_middle] {
            assert!(width(&truncate(branch, 12)) <= 12);
        }
    }

    #[test]
    fn middle_truncation_keeps_two_long_names_distinct() {
        let first = "feature/JIRA-12345-implement-the-thing-nobody-asked-for";
        let second = "feature/JIRA-12345-implement-the-thing-nobody-asked-for-v2";
        assert_ne!(truncate_middle(first, 24), truncate_middle(second, 24));
    }

    #[test]
    fn tiny_widths_never_overflow() {
        for truncate in [truncate_end, truncate_start, truncate_middle] {
            assert_eq!(truncate("feature", 0), "");
            assert_eq!(truncate("feature", 1), "…");
            assert_eq!(width(&truncate("feature", 2)), 2);
        }
    }

    #[test]
    fn widths_count_terminal_columns_for_wide_characters() {
        assert_eq!(width("日本語"), 6);
        assert_eq!(truncate_end("日本語-branch", 5), "日本…");
        assert_eq!(truncate_start("branch-日本語", 5), "…本語");
        assert_eq!(pad_end("日本", 6), "日本  ");
        assert_eq!(pad_end("toolong", 3), "toolong");
    }

    #[test]
    fn split_at_never_returns_an_empty_head_for_text() {
        assert_eq!(split_at("abcdef", 4), ("abcd", "ef"));
        assert_eq!(split_at("ab", 4), ("ab", ""));
        assert_eq!(split_at("日本語", 3), ("日", "本語"));
        assert_eq!(split_at("日本語", 1), ("日", "本語"));
        assert_eq!(split_at("", 3), ("", ""));
    }

    #[test]
    fn home_is_abbreviated_only_as_a_whole_path_prefix() {
        let home = Path::new("/Users/me");
        assert_eq!(home_relative(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(
            home_relative(Path::new("/Users/me/code/acme"), Some(home)),
            "~/code/acme"
        );
        assert_eq!(
            home_relative(Path::new("/Users/merlin/code"), Some(home)),
            "/Users/merlin/code"
        );
        assert_eq!(home_relative(Path::new("/tmp/x"), None), "/tmp/x");
    }
}
