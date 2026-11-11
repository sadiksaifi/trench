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
const HEIGHT: u16 = 11;

pub fn render(dialog: &RemoveDialog, frame: &mut Frame, area: Rect, theme: &Theme) {
    let layout = widgets::dialog_layout(area, HEIGHT);
    let width = usize::from(layout.content.width);
    let room = width.saturating_sub(LABEL_WIDTH + 2);
    let target = dialog.target();
    widgets::dialog_frame(frame, layout, "Remove worktree", None, theme);

    let branch = target.branch.as_deref();
    let home = paths::home_dir();
    let mut lines = vec![
        widgets::field(
            "Branch",
            LABEL_WIDTH,
            vec![Span::styled(
                text::truncate_middle(branch.unwrap_or(&target.worktree), room),
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
        Line::default(),
    ];

    let wrapped = |value: String, style| {
        widgets::wrap(&value, width)
            .into_iter()
            .take(2)
            .map(move |line| Line::styled(line, style))
            .collect::<Vec<_>>()
    };
    let branch = branch.unwrap_or_default();
    match dialog.branch_choice() {
        BranchChoice::Offered { merged } => {
            let mark = if dialog.delete_branch() { "[x]" } else { "[ ]" };
            let status = match merged {
                Some(true) => "merged",
                Some(false) => "not merged",
                None => "merge status unknown",
            };
            let fixed = format!("{mark} Also delete branch  ({status})");
            let name = text::truncate_middle(branch, width.saturating_sub(text::width(&fixed)));
            lines.push(Line::from(vec![
                Span::styled(format!("{mark} "), theme.accent),
                Span::styled(format!("Also delete branch {name} "), theme.text),
                Span::styled(format!("({status})"), theme.muted),
            ]));
        }
        BranchChoice::Detached => lines.extend(wrapped(
            "Detached: there is no branch to delete.".to_string(),
            theme.muted,
        )),
        BranchChoice::AlreadyGone => {
            lines.extend(wrapped(
                format!("Branch {branch} no longer exists."),
                theme.muted,
            ));
        }
        BranchChoice::CheckedOutElsewhere(path) => lines.extend(wrapped(
            format!(
                "Branch {branch} is also checked out at {}, so it stays.",
                text::home_relative(&path, home.as_deref())
            ),
            theme.muted,
        )),
    }
    lines.push(Line::default());

    let risks = dialog.risks();
    if let Some(error) = dialog.validation_error() {
        lines.extend(wrapped(format!("{} {error}", widgets::FAILED), theme.error));
    }
    for risk in &risks {
        let message = match risk {
            Risk::UncommittedChanges => {
                "Uncommitted changes in this worktree will be lost.".to_string()
            }
            Risk::UnverifiableChanges => format!(
                "Branch {branch} no longer exists, so changes here cannot be checked and may be lost."
            ),
            Risk::UnmergedCommits => format!("Commits only on {branch} will be lost."),
            Risk::UnknownMergeStatus => {
                format!("The base was not found, so commits only on {branch} may be lost.")
            }
        };
        lines.extend(wrapped(
            format!("{} {message}", widgets::WARNING),
            theme.warning,
        ));
    }
    if risks.is_empty() && dialog.validation_error().is_none() {
        let deletes_branch = if dialog.delete_branch() {
            format!(" Deletes branch {branch}.")
        } else {
            String::new()
        };
        lines.extend(wrapped(
            if target.directory_missing {
                format!(
                    "The directory is already gone. Only Git's record is removed.{deletes_branch}"
                )
            } else {
                format!("Removes the directory.{deletes_branch} This cannot be undone.")
            },
            theme.muted,
        ));
    }

    frame.render_widget(
        Paragraph::new(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(
        frame,
        layout.hints,
        &hints(dialog, !risks.is_empty()),
        theme,
    );
}

fn hints(dialog: &RemoveDialog, risky: bool) -> Vec<Hint> {
    let mut hints =
        vec![Hint::new("y", if risky { "remove anyway" } else { "remove" }, 0).tone(Tone::Danger)];
    if dialog.can_delete_branch() {
        hints.push(Hint::new(
            "Space",
            if dialog.delete_branch() {
                "keep branch"
            } else {
                "delete branch too"
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
                dialog.handle_key(*key);
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
                "  │ ! Uncommitted changes in this worktree will be lost. │",
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
                "  │ here cannot be checked and may be lost.              │",
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
    fn a_stale_review_shows_the_reason_above_the_live_risks() {
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
                "  │ × The worktree changed. Review it and press y again. │",
                "  │ ! Uncommitted changes in this worktree will be lost. │",
            ]
        );
    }

    #[test]
    fn the_dialog_fits_the_minimum_terminal() {
        let fixture = Fixture::new();
        let screen = screen(&mut fixture.open(&[]), 40, 10);

        assert_eq!(screen[0], " ╭ Remove worktree ───────────────────╮");
        assert_eq!(screen[1], " │ Branch  feature/auth               │");
        assert_eq!(screen[8], " ╰────────────────────────────────────╯");
        assert_eq!(screen[9], " y remove  Esc cancel");
        assert!(screen.iter().all(|row| row.chars().count() <= 40));
    }
}
