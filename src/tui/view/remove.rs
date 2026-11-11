//! The remove confirmation: every fact and every risk on one screen.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    output::text,
    paths,
    tui::{
        remove_flow::{BranchChoice, RemoveDialog, Risk},
        theme::Theme,
        view::widgets::{self, Hint, Tone},
    },
};

const LABEL_WIDTH: usize = 6;
/// The dialog keeps this height when its content is shorter, so it does not
/// shrink and grow as the branch checkbox is toggled.
const MIN_HEIGHT: u16 = 11;
const HEADER_ROWS: usize = 2;

/// Whether the dialog, drawn in `area`, shows every risk the removal carries.
///
/// `y` accepts the risks on screen. When the terminal is too small to show
/// them all, `y` must do nothing: nobody can accept a line they cannot see.
pub fn shows_every_risk(dialog: &RemoveDialog, area: Rect) -> bool {
    let layout = widgets::dialog_layout(area, MIN_HEIGHT);
    arrange(
        &body(dialog, usize::from(layout.content.width)),
        rows_available(area),
    )
    .is_some()
}

pub fn render(dialog: &RemoveDialog, frame: &mut Frame, area: Rect, theme: &Theme) {
    let width = usize::from(widgets::dialog_layout(area, MIN_HEIGHT).content.width);
    let arranged = arrange(&body(dialog, width), rows_available(area));
    let body_rows = arranged.as_ref().map_or(0, Vec::len);
    let layout = widgets::dialog_layout(area, MIN_HEIGHT.max((HEADER_ROWS + body_rows + 2) as u16));
    widgets::dialog_frame(frame, layout, "Remove worktree", None, theme);

    let room = width.saturating_sub(LABEL_WIDTH + 2);
    let target = dialog.target();
    let home = paths::home_dir();
    let mut lines = vec![
        widgets::field(
            "Branch",
            LABEL_WIDTH,
            vec![Span::styled(
                text::truncate_middle(target.branch.as_deref().unwrap_or(&target.worktree), room),
                theme.text,
            )],
            theme,
        ),
        widgets::field(
            "Path",
            LABEL_WIDTH,
            vec![Span::styled(
                text::truncate_start(&text::home_relative(&target.path, home.as_deref()), room),
                theme.text,
            )],
            theme,
        ),
    ];
    match &arranged {
        Some(rows) => lines.extend(rows.iter().map(|row| match row {
            Some((role, value)) => Line::styled(
                value.clone(),
                match role {
                    Role::Choice => theme.text,
                    Role::Note => theme.muted,
                    Role::Risk => theme.warning,
                    Role::Error => theme.error,
                },
            ),
            None => Line::default(),
        })),
        None => lines.extend(
            prefixed(
                widgets::FAILED,
                "Too small to show what would be lost. Enlarge the terminal.",
                width,
            )
            .into_iter()
            .map(|line| Line::styled(line, theme.error)),
        ),
    }

    frame.render_widget(
        Paragraph::new(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(
        frame,
        layout.hints,
        &hints(
            dialog,
            arranged.is_some(),
            usize::from(layout.hints.width.saturating_sub(widgets::MARGIN * 2)),
        ),
        theme,
    );
}

/// Rows inside the tallest dialog `area` allows, below the two header rows.
fn rows_available(area: Rect) -> usize {
    // One row for the hint bar and two for the border.
    usize::from(area.height.saturating_sub(3)).saturating_sub(HEADER_ROWS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Choice,
    Note,
    Risk,
    Error,
}

/// The dialog text below the header, wrapped to `width`.
struct Body {
    /// The branch checkbox, or why there is none.
    choice: Vec<(Role, String)>,
    /// What the removal destroys, or a note that it destroys nothing extra.
    losses: Vec<(Role, String)>,
    /// Why the last `y` did nothing. The only part that may be left out.
    error: Vec<(Role, String)>,
}

/// Nothing here is ever shortened. A line either appears whole or the dialog
/// reports that it does not fit.
fn body(dialog: &RemoveDialog, width: usize) -> Body {
    let target = dialog.target();
    let branch = target.branch.as_deref().unwrap_or_default();
    let home = paths::home_dir();
    let wrapped = |role: Role, value: String| {
        widgets::wrap(&value, width)
            .into_iter()
            .map(move |line| (role, line))
            .collect::<Vec<_>>()
    };

    let choice = match dialog.branch_choice() {
        BranchChoice::Offered { merged } => {
            let mark = if dialog.delete_branch() { "[x]" } else { "[ ]" };
            let status = match merged {
                Some(true) => "merged",
                Some(false) => "not merged",
                None => "merge status unknown",
            };
            prefixed(
                mark,
                &format!("Also delete branch {branch} ({status})"),
                width,
            )
            .into_iter()
            .map(|line| (Role::Choice, line))
            .collect()
        }
        BranchChoice::Detached => wrapped(
            Role::Note,
            "Detached: there is no branch to delete.".to_string(),
        ),
        BranchChoice::AlreadyGone => {
            wrapped(Role::Note, format!("Branch {branch} no longer exists."))
        }
        BranchChoice::CheckedOutElsewhere(path) => wrapped(
            Role::Note,
            format!(
                "Branch {branch} is also checked out at {}, so it stays.",
                text::home_relative(&path, home.as_deref())
            ),
        ),
    };

    let mut losses = Vec::new();
    for risk in dialog.risks() {
        let message = match risk {
            Risk::UncommittedChanges => "Uncommitted changes will be lost.".to_string(),
            Risk::UnverifiableChanges => format!(
                "Branch {branch} no longer exists, so changes here cannot be checked and may be lost."
            ),
            Risk::UnmergedCommits => format!("Commits only on {branch} will be lost."),
            Risk::UnknownMergeStatus => {
                format!("The base was not found, so commits only on {branch} may be lost.")
            }
            Risk::UnreachableCommits => {
                "Commits here are on no branch and will be lost.".to_string()
            }
        };
        losses.extend(
            prefixed(widgets::WARNING, &message, width)
                .into_iter()
                .map(|line| (Role::Risk, line)),
        );
    }
    let error = dialog
        .validation_error()
        .map(|error| {
            prefixed(widgets::FAILED, error, width)
                .into_iter()
                .map(|line| (Role::Error, line))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if losses.is_empty() && error.is_empty() {
        let deletes_branch = if dialog.delete_branch() {
            format!(" Deletes branch {branch}.")
        } else {
            String::new()
        };
        losses = wrapped(
            Role::Note,
            if target.directory_missing {
                format!(
                    "The directory is already gone. Only Git's record is removed.{deletes_branch}"
                )
            } else {
                format!("Removes the directory.{deletes_branch} This cannot be undone.")
            },
        );
    }
    Body {
        choice,
        losses,
        error,
    }
}

/// Wrap `message` after a glyph, indenting continuation lines under the text.
fn prefixed(glyph: &str, message: &str, width: usize) -> Vec<String> {
    let indent = text::width(glyph) + 1;
    widgets::wrap(message, width.saturating_sub(indent))
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{glyph} {line}")
            } else {
                format!("{}{line}", " ".repeat(indent))
            }
        })
        .collect()
}

/// Lay the body out in `rows`: a blank row before each group when there is
/// room, and the error after the risks.
///
/// The risks are what `y` accepts, so they are never dropped. The error is
/// dropped first when rows run out, because the risk it points at says more
/// than it does. `None` means the choice and the risks alone do not fit.
fn arrange(body: &Body, rows: usize) -> Option<Vec<Option<(Role, String)>>> {
    let required = body.choice.len() + body.losses.len();
    if required > rows {
        return None;
    }
    let groups = 2;
    let with_error = required + body.error.len() <= rows;
    let text_rows = required + if with_error { body.error.len() } else { 0 };
    let spaced = text_rows + groups <= rows;
    let mut arranged = Vec::new();
    for group in [&body.choice, &body.losses] {
        if spaced && !group.is_empty() {
            arranged.push(None);
        }
        arranged.extend(group.iter().cloned().map(Some));
    }
    if with_error {
        arranged.extend(body.error.iter().cloned().map(Some));
    }
    Some(arranged)
}

/// The dialog's hints for a bar `width` columns wide. When the full
/// wording does not fit, every hint gets its short form, so the Space hint
/// is shortened instead of dropped.
fn hints(dialog: &RemoveDialog, every_risk_shown: bool, width: usize) -> Vec<Hint> {
    let full = hint_set(dialog, every_risk_shown, false);
    if widgets::fit_hints(&full, width).len() == full.len() {
        full
    } else {
        hint_set(dialog, every_risk_shown, true)
    }
}

fn hint_set(dialog: &RemoveDialog, every_risk_shown: bool, short: bool) -> Vec<Hint> {
    let risky = !dialog.risks().is_empty();
    let label = if risky && !short {
        "remove anyway"
    } else {
        "remove"
    };
    // `y` is dim whenever it would do nothing: the risks do not fit, or they
    // appeared a moment ago and the pause to read them is still running.
    let live = every_risk_shown && dialog.confirm_ready(std::time::Instant::now());
    let mut hints =
        vec![Hint::new("y", label, 0).tone(if live { Tone::Danger } else { Tone::Disabled })];
    if dialog.can_delete_branch() {
        hints.push(Hint::new(
            "Space",
            match (short, dialog.delete_branch()) {
                (true, _) => "branch",
                (false, true) => "keep branch",
                (false, false) => "delete branch too",
            },
            2,
        ));
    }
    hints.push(Hint::new("Esc", "cancel", 1));
    hints
}

#[cfg(test)]
mod tests {
    use std::{path::Path, process::Command};

    use crate::{
        cli::commands::remove::stateless::RemovalAssessment,
        tui::{
            app::{AppState, WorktreeId},
            remove_flow::{RemoveDialog, RemoveKey},
            view::testing::{screen, state},
        },
    };

    fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    struct Fixture {
        root: tempfile::TempDir,
    }

    impl Fixture {
        /// A repository with one linked worktree on branch `feature/auth`.
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            git(root.path(), &["init", "-b", "main"]);
            git(root.path(), &["config", "user.email", "test@example.com"]);
            git(root.path(), &["config", "user.name", "Test"]);
            std::fs::write(root.path().join("README.md"), "base\n").unwrap();
            git(root.path(), &["add", "README.md"]);
            git(root.path(), &["commit", "-m", "base"]);
            let fixture = Self { root };
            git(
                fixture.root.path(),
                &[
                    "worktree",
                    "add",
                    "-b",
                    "feature/auth",
                    fixture.worktree().to_str().unwrap(),
                    "main",
                ],
            );
            fixture
        }

        fn worktree(&self) -> std::path::PathBuf {
            self.root.path().join("wt")
        }

        fn commit_on_branch(&self) {
            std::fs::write(self.worktree().join("feature.txt"), "feature\n").unwrap();
            git(&self.worktree(), &["add", "feature.txt"]);
            git(&self.worktree(), &["commit", "-m", "feature"]);
        }

        fn open(&self, keys: &[RemoveKey]) -> AppState {
            let assessment = RemovalAssessment::discover(
                self.root.path(),
                self.worktree().to_str().unwrap(),
                Some("main"),
            )
            .unwrap();
            let mut dialog =
                RemoveDialog::new(WorktreeId::new(self.worktree()), assessment).unwrap();
            for key in keys {
                dialog.handle_key(*key, std::time::Instant::now());
            }
            let mut state = state(&[]);
            state.remove_dialog = Some(dialog);
            state
        }
    }

    /// The dialog rows below the Path row, which holds a temporary directory.
    fn below_path(state: &mut AppState) -> Vec<String> {
        let screen = screen(state, 60, 16);
        assert_eq!(
            screen[2],
            "  ╭ Remove worktree ─────────────────────────────────────╮"
        );
        assert_eq!(
            screen[3],
            "  │ Branch  feature/auth                                 │"
        );
        assert!(screen[4].starts_with("  │ Path    …"), "{}", screen[4]);
        assert!(screen[4].ends_with("/wt │"), "{}", screen[4]);
        let mut rows = screen[5..12].to_vec();
        rows.push(screen[15].clone());
        rows
    }

    #[test]
    fn a_clean_merged_worktree_offers_the_branch_and_names_the_only_key_that_removes() {
        let fixture = Fixture::new();

        assert_eq!(
            below_path(&mut fixture.open(&[])),
            [
                "  │                                                      │",
                "  │ [ ] Also delete branch feature/auth (merged)         │",
                "  │                                                      │",
                "  │ Removes the directory. This cannot be undone.        │",
                "  │                                                      │",
                "  │                                                      │",
                "  │                                                      │",
                " y remove  Space delete branch too  Esc cancel",
            ]
        );
    }

    #[test]
    fn every_risk_is_listed_before_any_key_is_pressed() {
        let fixture = Fixture::new();
        fixture.commit_on_branch();
        std::fs::write(fixture.worktree().join("dirty.txt"), "dirty\n").unwrap();

        assert_eq!(
            below_path(&mut fixture.open(&[RemoveKey::Space])),
            [
                "  │                                                      │",
                "  │ [x] Also delete branch feature/auth (not merged)     │",
                "  │                                                      │",
                "  │ ! Uncommitted changes will be lost.                  │",
                "  │ ! Commits only on feature/auth will be lost.         │",
                "  │                                                      │",
                "  │                                                      │",
                " y remove anyway  Space keep branch  Esc cancel",
            ]
        );
    }

    #[test]
    fn a_missing_directory_says_only_gits_record_is_removed() {
        let fixture = Fixture::new();
        std::fs::remove_dir_all(fixture.worktree()).unwrap();

        let rows = below_path(&mut fixture.open(&[]));

        assert_eq!(
            rows[3],
            "  │ The directory is already gone. Only Git's record is  │"
        );
        assert_eq!(
            rows[4],
            "  │ removed.                                             │"
        );
    }

    #[test]
    fn a_deleted_branch_has_no_checkbox_and_explains_the_unverifiable_changes() {
        let fixture = Fixture::new();
        git(
            fixture.root.path(),
            &["update-ref", "-d", "refs/heads/feature/auth"],
        );

        assert_eq!(
            below_path(&mut fixture.open(&[])),
            [
                "  │                                                      │",
                "  │ Branch feature/auth no longer exists.                │",
                "  │                                                      │",
                "  │ ! Branch feature/auth no longer exists, so changes   │",
                "  │   here cannot be checked and may be lost.            │",
                "  │                                                      │",
                "  │                                                      │",
                " y remove anyway  Esc cancel",
            ]
        );
    }

    #[test]
    fn a_detached_worktree_has_no_branch_to_delete() {
        let fixture = Fixture::new();
        git(&fixture.worktree(), &["checkout", "--detach"]);

        let screen = screen(&mut fixture.open(&[]), 60, 16);

        assert!(
            screen[3].starts_with("  │ Branch  detached@"),
            "{}",
            screen[3]
        );
        assert_eq!(
            screen[6],
            "  │ Detached: there is no branch to delete.              │"
        );
        assert_eq!(screen[15], " y remove  Esc cancel");
    }

    #[test]
    fn a_stale_review_lists_the_live_risks_before_the_reason() {
        let fixture = Fixture::new();
        let mut state = fixture.open(&[]);
        std::fs::write(fixture.worktree().join("late.txt"), "late\n").unwrap();
        let dialog = state.remove_dialog.as_mut().unwrap();
        let error = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None)
            .unwrap_err();
        dialog.set_validation_error(Some(error.to_string()));

        let rows = below_path(&mut state);

        assert_eq!(
            rows[3..5],
            [
                "  │ ! Uncommitted changes will be lost.                  │",
                "  │ × The worktree changed. Review it and press y again. │",
            ]
        );
    }

    fn risky(fixture: &Fixture) -> AppState {
        fixture.commit_on_branch();
        std::fs::write(fixture.worktree().join("dirty.txt"), "dirty\n").unwrap();
        fixture.open(&[RemoveKey::Space])
    }

    #[test]
    fn a_narrow_terminal_wraps_every_line_instead_of_cutting_it() {
        let fixture = Fixture::new();
        let mut state = risky(&fixture);
        let screen = screen(&mut state, 40, 14);

        assert_eq!(
            screen[4..6],
            [
                " │ [x] Also delete branch             │",
                " │     feature/auth (not merged)      │",
            ]
        );
        assert_eq!(
            screen[7..9],
            [
                " │ ! Uncommitted changes will be      │",
                " │   lost.                            │",
            ]
        );
        // Wrapped lines differ by width; the sentences must survive whole.
        let text = screen.join("").replace(['│', ' '], "");
        assert!(
            text.contains("!Uncommittedchangeswillbelost."),
            "{screen:#?}"
        );
        assert!(
            text.contains("!Commitsonlyonfeature/authwillbelost."),
            "{screen:#?}"
        );
        assert_eq!(screen[13], " y remove  Space branch  Esc cancel");
        assert!(super::shows_every_risk(
            state.remove_dialog.as_ref().unwrap(),
            ratatui::layout::Rect::new(0, 0, 40, 14)
        ));
    }

    #[test]
    fn a_terminal_too_short_for_the_risks_says_so_and_turns_y_off() {
        let fixture = Fixture::new();
        let mut state = risky(&fixture);
        let area = ratatui::layout::Rect::new(0, 0, 40, 10);
        let buffer = crate::tui::view::testing::draw(&mut state, 40, 10, "terminal");
        let screen = crate::tui::view::testing::lines(&buffer);

        assert!(!super::shows_every_risk(
            state.remove_dialog.as_ref().unwrap(),
            area
        ));
        assert_eq!(
            screen[3..5],
            [
                " │ × Too small to show what would be  │",
                " │   lost. Enlarge the terminal.      │",
            ]
        );
        assert!(
            screen.iter().all(|row| !row.contains("will be lost")),
            "a risk is shown whole or not at all: {screen:#?}"
        );
        assert!(buffer[(1, 9)]
            .modifier
            .contains(ratatui::style::Modifier::DIM));
    }

    #[test]
    fn at_the_minimum_size_an_error_line_gives_way_to_the_risk_it_is_about() {
        let fixture = Fixture::new();
        let mut state = fixture.open(&[]);
        std::fs::write(fixture.worktree().join("late.txt"), "late\n").unwrap();
        let dialog = state.remove_dialog.as_mut().unwrap();
        let error = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None)
            .unwrap_err();
        dialog.set_validation_error(Some(error.to_string()));
        let area = ratatui::layout::Rect::new(0, 0, 40, 10);

        let screen = screen(&mut state, 40, 10);
        let text = screen.join("").replace(['│', ' '], "");

        assert!(
            text.contains("!Uncommittedchangeswillbelost."),
            "the new risk fits once the error line is dropped: {screen:#?}"
        );
        assert!(!text.contains("Toosmall"), "{screen:#?}");
        assert!(super::shows_every_risk(
            state.remove_dialog.as_ref().unwrap(),
            area
        ));
    }

    #[test]
    fn y_is_dimmed_while_it_is_paused() {
        let fixture = Fixture::new();
        std::fs::write(fixture.worktree().join("dirty.txt"), "dirty\n").unwrap();
        let mut state = fixture.open(&[]);
        let buffer = crate::tui::view::testing::draw(&mut state, 60, 16, "terminal");

        assert_eq!(
            crate::tui::view::testing::lines(&buffer)[15],
            " y remove anyway  Space delete branch too  Esc cancel"
        );
        assert!(
            buffer[(1, 15)]
                .modifier
                .contains(ratatui::style::Modifier::DIM),
            "the dialog just opened with a risk, so y does nothing yet"
        );
    }

    #[test]
    fn a_stale_review_never_pushes_a_risk_off_the_dialog() {
        let fixture = Fixture::new();
        fixture.commit_on_branch();
        let mut state = fixture.open(&[RemoveKey::Space]);
        std::fs::write(fixture.worktree().join("late.txt"), "late\n").unwrap();
        let dialog = state.remove_dialog.as_mut().unwrap();
        let error = dialog
            .revalidate_request(fixture.root.path(), Some("main"), None)
            .unwrap_err();
        dialog.set_validation_error(Some(error.to_string()));

        let screen = screen(&mut state, 52, 24).join("\n");

        assert!(
            screen.contains("! Uncommitted changes will be lost."),
            "{screen}"
        );
        assert!(
            screen.contains("! Commits only on feature/auth will be lost."),
            "{screen}"
        );
        assert!(screen.contains("× The worktree changed."), "{screen}");
    }

    #[test]
    fn a_branch_checked_out_elsewhere_names_the_whole_path_at_any_width() {
        let fixture = Fixture::new();
        let second = fixture.root.path().join("second");
        git(
            fixture.root.path(),
            &[
                "worktree",
                "add",
                "--force",
                second.to_str().unwrap(),
                "feature/auth",
            ],
        );
        let mut state = fixture.open(&[]);
        let screen = screen(&mut state, 40, 30).join("");
        let text = screen.replace(['│', ' '], "");

        assert!(
            text.contains("Branchfeature/authisalsocheckedoutat"),
            "{screen}"
        );
        assert!(text.contains("/second,soitstays."), "{screen}");
    }

    #[test]
    fn a_clean_worktree_fits_the_minimum_terminal_with_its_whole_message() {
        let fixture = Fixture::new();
        let mut state = fixture.open(&[]);
        let screen = screen(&mut state, 40, 10);

        assert_eq!(screen[0], " ╭ Remove worktree ───────────────────╮");
        assert_eq!(screen[1], " │ Branch  feature/auth               │");
        assert_eq!(
            screen[3..8],
            [
                " │ [ ] Also delete branch             │",
                " │     feature/auth (merged)          │",
                " │ Removes the directory. This cannot │",
                " │ be undone.                         │",
                " │                                    │",
            ]
        );
        assert_eq!(screen[8], " ╰────────────────────────────────────╯");
        assert_eq!(screen[9], " y remove  Space branch  Esc cancel");
        assert!(super::shows_every_risk(
            state.remove_dialog.as_ref().unwrap(),
            ratatui::layout::Rect::new(0, 0, 40, 10)
        ));
    }
}
