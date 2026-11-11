//! Human-readable CLI messages: one format for errors, warnings and hints.
//!
//! Structured output (`--json`, `--porcelain`) never passes through here.

use std::ffi::OsStr;
use std::io::IsTerminal;

use crate::output::text;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED_BOLD: &str = "\x1b[1;31m";
const YELLOW_BOLD: &str = "\x1b[1;33m";
const GREEN_BOLD: &str = "\x1b[1;32m";

/// Whether escape sequences may be written to one output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    color: bool,
}

impl Palette {
    pub const PLAIN: Self = Self { color: false };

    pub fn stdout() -> Self {
        Self::detect(std::io::stdout().is_terminal())
    }

    pub fn stderr() -> Self {
        Self::detect(std::io::stderr().is_terminal())
    }

    fn detect(is_terminal: bool) -> Self {
        Self {
            color: color_enabled(
                is_terminal,
                std::env::var_os("NO_COLOR").as_deref(),
                std::env::var_os("TERM").as_deref(),
            ),
        }
    }

    pub fn dim(self, value: &str) -> String {
        self.paint(DIM, value)
    }

    pub fn success(self, value: &str) -> String {
        self.paint(GREEN_BOLD, value)
    }

    pub fn warning(self, value: &str) -> String {
        self.paint(YELLOW_BOLD, value)
    }

    fn paint(self, style: &str, value: &str) -> String {
        if self.color {
            format!("{style}{value}{RESET}")
        } else {
            value.to_string()
        }
    }
}

/// Color is written only to a terminal that has not opted out.
///
/// `NO_COLOR` counts when set to any non-empty value (https://no-color.org).
pub fn color_enabled(is_terminal: bool, no_color: Option<&OsStr>, term: Option<&OsStr>) -> bool {
    is_terminal
        && !no_color.is_some_and(|value| !value.is_empty())
        && term.is_none_or(|value| value != "dumb")
}

/// An error with the next steps the user can take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    message: String,
    hints: Vec<String>,
}

impl Report {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hints: Vec::new(),
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    pub fn render(&self, palette: Palette) -> String {
        let mut rendered = format!("{} {}", palette.paint(RED_BOLD, "error:"), self.message);
        for hint in &self.hints {
            rendered.push_str(&format!("\n{} {hint}", palette.paint(BOLD, "hint:")));
        }
        rendered
    }

    pub fn print(&self) {
        eprintln!("{}", self.render(Palette::stderr()));
    }
}

/// Render an `anyhow` error chain as one `error:` line plus its causes.
pub fn render_error_chain(error: &anyhow::Error, palette: Palette) -> String {
    let mut rendered = Report::new(error.to_string()).render(palette);
    for cause in error.chain().skip(1) {
        rendered.push_str(&format!("\n  caused by: {cause}"));
    }
    rendered
}

pub fn render_warning(message: &str, palette: Palette) -> String {
    format!("{} {message}", palette.warning("warning:"))
}

pub fn print_warning(message: &str) {
    eprintln!("{}", render_warning(message, Palette::stderr()));
}

pub fn print_hint(message: &str) {
    eprintln!("{} {message}", Palette::stderr().paint(BOLD, "hint:"));
}

/// The header every `--dry-run` preview starts with.
pub const DRY_RUN_HEADER: &str = "Dry run: nothing was changed.";

/// Render a `--dry-run` preview: header, one action line, aligned fields.
pub fn render_preview(action: &str, fields: &[(&str, String)]) -> String {
    let label_width = fields
        .iter()
        .map(|(label, _)| text::width(label))
        .max()
        .unwrap_or(0);
    let mut rendered = format!("{DRY_RUN_HEADER}\n\n{action}\n");
    for (label, value) in fields {
        rendered.push_str(&format!(
            "  {}  {value}\n",
            text::pad_end(label, label_width)
        ));
    }
    rendered
}

/// Describe which lifecycle hooks a command runs.
pub fn describe_hooks(skipped: bool, configured: &[&str]) -> String {
    if skipped {
        "skipped (--no-hooks)".to_string()
    } else if configured.is_empty() {
        "none configured".to_string()
    } else {
        configured.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_needs_a_terminal_that_has_not_opted_out() {
        let set = |value: &'static str| Some(OsStr::new(value));
        assert!(color_enabled(true, None, set("xterm-256color")));
        assert!(color_enabled(true, set(""), None));
        assert!(!color_enabled(false, None, set("xterm-256color")));
        assert!(!color_enabled(true, set("1"), set("xterm-256color")));
        assert!(!color_enabled(true, None, set("dumb")));
    }

    #[test]
    fn reports_render_one_error_line_and_one_line_per_hint() {
        let report = Report::new("worktree not found: nope")
            .hint("run `trench list` to see worktrees")
            .hint("detached worktrees are named detached@<commit>");

        assert_eq!(
            report.render(Palette::PLAIN),
            "error: worktree not found: nope\n\
             hint: run `trench list` to see worktrees\n\
             hint: detached worktrees are named detached@<commit>"
        );
    }

    #[test]
    fn plain_rendering_has_no_escape_bytes_and_color_wraps_only_the_label() {
        let report = Report::new("boom").hint("try again");
        assert!(!report.render(Palette::PLAIN).contains('\x1b'));
        assert_eq!(
            report.render(Palette { color: true }),
            "\x1b[1;31merror:\x1b[0m boom\n\x1b[1mhint:\x1b[0m try again"
        );
        assert_eq!(
            render_warning("careful", Palette { color: true }),
            "\x1b[1;33mwarning:\x1b[0m careful"
        );
    }

    #[test]
    fn error_chains_list_each_cause_once() {
        let error = anyhow::anyhow!("permission denied")
            .context("failed to write receipt")
            .context("upgrade failed");

        assert_eq!(
            render_error_chain(&error, Palette::PLAIN),
            "error: upgrade failed\n  caused by: failed to write receipt\n  caused by: permission denied"
        );
    }

    #[test]
    fn previews_align_field_values() {
        assert_eq!(
            render_preview(
                "Would create worktree feature-auth",
                &[
                    ("Branch", "feature/auth (new, from main)".to_string()),
                    ("Path", "/tmp/feature-auth".to_string()),
                ],
            ),
            "Dry run: nothing was changed.\n\n\
             Would create worktree feature-auth\n\
             \x20 Branch  feature/auth (new, from main)\n\
             \x20 Path    /tmp/feature-auth\n"
        );
    }

    #[test]
    fn hooks_are_described_by_policy_then_by_configured_names() {
        assert_eq!(
            describe_hooks(true, &["pre_create"]),
            "skipped (--no-hooks)"
        );
        assert_eq!(describe_hooks(false, &[]), "none configured");
        assert_eq!(
            describe_hooks(false, &["pre_create", "post_create"]),
            "pre_create, post_create"
        );
    }
}
