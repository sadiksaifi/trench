//! Progress and outcome of a running create, sync or remove.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    cli::report,
    operation::{MutationState, OperationKind, OperationStage},
    output::text,
    paths,
    tui::{
        operation_modal::{ModalStatus, OperationModal, StageView},
        theme::Theme,
        view::{
            list::spinner,
            widgets::{self, Hint},
        },
    },
};

const HEIGHT: u16 = 16;

/// The newest hook output lines kept in a failure report. The lines that
/// explain a failure are its last ones, and a bounded tail keeps the report
/// short enough to scroll and cheap to rebuild every frame.
const OUTPUT_TAIL: usize = 100;

/// The scroll geometry of the dialog drawn in `area`: how many rows of a
/// failure report do not fit, and how many rows one page is. The runtime
/// passes both to the modal so scrolling stops at the end.
pub fn report_geometry(modal: &OperationModal, area: Rect) -> (usize, usize) {
    let layout = widgets::dialog_layout(area, HEIGHT);
    let rows = usize::from(layout.content.height);
    let theme = crate::tui::theme::from_name("mono");
    let overflow = failure_report(modal, usize::from(layout.content.width), &theme)
        .map_or(0, |report| report.len().saturating_sub(rows));
    (overflow, rows.max(1))
}

pub fn render(modal: &OperationModal, frame: &mut Frame, area: Rect, theme: &Theme) {
    let layout = widgets::dialog_layout(area, HEIGHT);
    let width = usize::from(layout.content.width);
    let rows = usize::from(layout.content.height);
    widgets::dialog_frame(
        frame,
        layout,
        title(modal),
        Some(Line::styled(
            format!(" {:.1}s ", modal.elapsed().as_secs_f64()),
            theme.on_dialog(theme.muted),
        )),
        theme,
    );

    let (lines, scrollable) = match failure_report(modal, width, theme) {
        Some(report) => {
            let overflow = report.len().saturating_sub(rows);
            let offset = modal.report_scroll().min(overflow);
            (
                report.into_iter().skip(offset).take(rows).collect(),
                overflow > 0,
            )
        }
        None => (progress(modal, width, rows, theme), true),
    };
    frame.render_widget(
        Paragraph::new::<Vec<Line>>(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(
        frame,
        layout.hints,
        &hints(modal, scrollable, area.width),
        theme,
    );
}

/// A running or finished operation: stages, then the newest hook output.
fn progress(
    modal: &OperationModal,
    width: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = stages(modal)
        .into_iter()
        .map(|stage| stage_line(modal, stage, theme))
        .collect::<Vec<_>>();
    for warning in modal.warnings() {
        lines.extend(
            widgets::wrap(&format!("{} {warning}", widgets::WARNING), width)
                .into_iter()
                .map(|line| Line::styled(line, theme.warning)),
        );
    }
    // Hook output takes whatever rows are left, newest at the bottom.
    let output = modal.visible_hook_lines(rows.saturating_sub(lines.len() + 1));
    if !output.is_empty() {
        lines.push(Line::default());
        lines.extend(
            output
                .into_iter()
                .map(|line| output_line(line, width, theme)),
        );
    }
    lines
}

/// A failed operation, as one document read from the top: what failed, what
/// is left behind, what state things are in, then progress and all output.
///
/// The lines that tell the user what to do come first, and nothing is cut:
/// the whole report is reachable by scrolling. `None` unless it failed.
fn failure_report(
    modal: &OperationModal,
    width: usize,
    theme: &Theme,
) -> Option<Vec<Line<'static>>> {
    let ModalStatus::Failed {
        stage,
        mutation_state,
        message,
        retained_quarantine,
    } = modal.status()
    else {
        return None;
    };
    let wrapped = |value: String, style| {
        widgets::wrap(&value, width)
            .into_iter()
            .map(move |line| Line::styled(line, style))
    };
    let headline = match report::hook_name(modal.operation(), *stage) {
        Some(hook) => report::hook_failure_message(hook, message),
        None => message.clone(),
    };
    let mut lines =
        wrapped(format!("{} {headline}", widgets::FAILED), theme.error).collect::<Vec<_>>();
    if let Some(path) = retained_quarantine {
        let home = paths::home_dir();
        lines.extend(wrapped(
            format!(
                "Kept for recovery: {}",
                text::home_relative(path, home.as_deref())
            ),
            theme.text,
        ));
    }
    // Without this sentence the user cannot tell whether a failed operation
    // left anything behind.
    let state = report::failure_state(
        modal.operation(),
        *stage,
        *mutation_state,
        modal.directory_missing(),
        true,
    )
    .unwrap_or("nothing was changed");
    lines.extend(wrapped(sentence(state), theme.text));
    for warning in modal.warnings() {
        lines.extend(wrapped(
            format!("{} {warning}", widgets::WARNING),
            theme.warning,
        ));
    }

    let stage_rows = stages(modal);
    if !stage_rows.is_empty() {
        lines.push(Line::default());
        lines.extend(
            stage_rows
                .into_iter()
                .map(|stage| stage_line(modal, stage, theme)),
        );
    }
    let output = modal.hook_lines();
    if !output.is_empty() {
        lines.push(Line::default());
        let earlier = output.len().saturating_sub(OUTPUT_TAIL);
        let not_shown = earlier + modal.dropped_hook_lines();
        if not_shown > 0 {
            lines.push(Line::styled(
                format!("  … {not_shown} earlier lines not shown"),
                theme.muted,
            ));
        }
        // Wrapped, not cut: the end of an error line is often the part
        // that says what went wrong.
        for line in &output[earlier..] {
            let wrapped = widgets::wrap(line, width.saturating_sub(2));
            if wrapped.is_empty() {
                lines.push(Line::default());
            }
            lines.extend(
                wrapped
                    .into_iter()
                    .map(|part| Line::styled(format!("  {part}"), theme.muted)),
            );
        }
    }
    Some(lines)
}

fn output_line(line: &str, width: usize, theme: &Theme) -> Line<'static> {
    Line::styled(
        format!("  {}", text::truncate_end(line, width.saturating_sub(2))),
        theme.muted,
    )
}

fn title(modal: &OperationModal) -> &'static str {
    let applied = matches!(
        modal.status(),
        ModalStatus::Failed {
            mutation_state: MutationState::Applied,
            ..
        }
    );
    match (modal.operation(), modal.status()) {
        (OperationKind::Create, _) if applied => "Created, but a hook failed",
        (OperationKind::Sync, _) if applied => "Synced, but a hook failed",
        (OperationKind::Remove, _) if applied => "Removed, but a hook failed",
        (OperationKind::Create, ModalStatus::Failed { .. }) => "Create failed",
        (OperationKind::Sync, ModalStatus::Failed { .. }) => "Sync failed",
        (OperationKind::Remove, ModalStatus::Failed { .. }) => "Remove failed",
        (OperationKind::Create, _) => "Creating worktree",
        (OperationKind::Sync, _) => "Syncing worktree",
        (OperationKind::Remove, _) => "Removing worktree",
    }
}

/// The stages worth a row.
///
/// Trench revalidates before and after hooks. Those checks are bookkeeping
/// unless one fails, so only a failed check is listed, and repeats of a stage
/// collapse into its latest run.
fn stages(modal: &OperationModal) -> Vec<&StageView> {
    let mut shown: Vec<&StageView> = Vec::new();
    for stage in modal.stages() {
        if stage.stage == OperationStage::Revalidate && stage.success != Some(false) {
            continue;
        }
        match shown.iter_mut().find(|seen| seen.stage == stage.stage) {
            Some(seen) => *seen = stage,
            None => shown.push(stage),
        }
    }
    shown
}

fn stage_line(modal: &OperationModal, stage: &StageView, theme: &Theme) -> Line<'static> {
    let running = modal.current_stage() == Some(stage.stage) && modal.spinner_visible();
    let (marker, style) = if running {
        (spinner(modal.spinner_tick()).to_string(), theme.accent)
    } else {
        match stage.success {
            Some(true) => (widgets::OK.to_string(), theme.success),
            Some(false) => (widgets::FAILED.to_string(), theme.error),
            None => (widgets::PENDING.to_string(), theme.muted),
        }
    };
    Line::from(vec![
        Span::styled(format!("{marker} "), style),
        Span::styled(
            stage_label(modal.operation(), stage.stage),
            if stage.success.is_none() && !running {
                theme.muted
            } else {
                theme.text
            },
        ),
    ])
}

fn stage_label(operation: OperationKind, stage: OperationStage) -> String {
    if let Some(hook) = report::hook_name(operation, stage) {
        return format!("{hook} hook");
    }
    match stage {
        OperationStage::Fetch => "Fetch origin",
        OperationStage::Revalidate => "Checks",
        OperationStage::CreateWorktree => "Create worktree",
        OperationStage::Sync => "Sync",
        OperationStage::RemoveWorktree => "Remove worktree",
        OperationStage::Prune => "Clean up Git's record",
        OperationStage::DeleteBranch => "Delete branch",
        OperationStage::Rollback => "Roll back",
        OperationStage::PreHook | OperationStage::PostHook => "Hook",
    }
    .to_string()
}

/// `nothing was changed` -> `Nothing was changed.`
fn sentence(clause: &str) -> String {
    let mut characters = clause.chars();
    match characters.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), characters.as_str()),
        None => String::new(),
    }
}

/// Below this width the hints use their short wording, so the one that
/// says how to scroll is never the hint that gets dropped.
const WIDE_HINTS: u16 = 60;

fn hints(modal: &OperationModal, scrollable: bool, width: u16) -> Vec<Hint> {
    let mut hints = Vec::new();
    let failed = matches!(modal.status(), ModalStatus::Failed { .. });
    let wide = width >= WIDE_HINTS;
    match modal.status() {
        ModalStatus::Failed { .. } if modal.can_return_to_form() => {
            hints.push(Hint::new(
                "Enter",
                if wide { "back to form" } else { "back" },
                0,
            ));
            hints.push(Hint::new("Esc", "close", 1));
        }
        ModalStatus::Failed { .. } => hints.push(Hint::new("Enter", "close", 0)),
        ModalStatus::Running if modal.can_cancel() => hints.push(Hint::new("Esc", "cancel", 0)),
        ModalStatus::Running | ModalStatus::Succeeded => {}
    }
    if scrollable {
        hints.push(match (failed, wide) {
            (true, true) => Hint::new("↑/↓ PgUp/PgDn", "scroll", 2),
            (true, false) => Hint::new("↑/↓", "scroll", 2),
            (false, _) => Hint::new("↑/↓", "scroll output", 2),
        });
    }
    hints
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        hooks::{
            types::{HookStep, OutputStream},
            HookEvent,
        },
        operation::{
            ErrorClass, MutationState, OperationEvent, OperationFailure, OperationKind,
            OperationStage,
        },
        tui::{
            app::AppState,
            operation_modal::OperationModal,
            view::testing::{screen, state},
        },
    };

    fn run(modal: &mut OperationModal, stage: OperationStage, success: Option<bool>) {
        modal.apply(OperationEvent::StageStarted { stage });
        if let Some(success) = success {
            modal.apply(OperationEvent::StageFinished {
                stage,
                duration: Duration::from_millis(10),
                success,
            });
        }
    }

    fn output(modal: &mut OperationModal, line: &str) {
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PostCreate,
            step: HookStep::Shell,
            stream: OutputStream::Stdout,
            line: line.to_string(),
        });
    }

    fn show(modal: OperationModal) -> AppState {
        let mut state = state(&[]);
        state.operation_modal = Some(modal);
        state
    }

    #[test]
    fn a_running_create_lists_its_stages_without_repeated_checks() {
        let mut modal = OperationModal::new(OperationKind::Create);
        run(&mut modal, OperationStage::Revalidate, Some(true));
        run(&mut modal, OperationStage::PreHook, Some(true));
        run(&mut modal, OperationStage::Revalidate, Some(true));
        modal.apply(OperationEvent::MutationStarted);
        run(&mut modal, OperationStage::CreateWorktree, Some(true));
        run(&mut modal, OperationStage::PostHook, None);
        output(&mut modal, "bun install v1.2.4");
        output(&mut modal, "+ 412 packages installed");
        modal.tick(Duration::from_millis(4200));
        let screen = screen(&mut show(modal), 60, 20);

        assert_eq!(
            screen[1],
            "  ╭ Creating worktree ───────────────────────────── 4.2s ╮"
        );
        assert_eq!(
            screen[2],
            "  │ ✓ pre_create hook                                    │"
        );
        assert_eq!(
            screen[3],
            "  │ ✓ Create worktree                                    │"
        );
        assert!(screen[4].ends_with("post_create hook                                   │"));
        assert_eq!(
            screen[6],
            "  │   bun install v1.2.4                                 │"
        );
        assert_eq!(
            screen[7],
            "  │   + 412 packages installed                           │"
        );
        assert!(screen.iter().all(|row| !row.contains("Checks")));
        assert_eq!(screen[19], " ↑/↓ scroll output");
    }

    #[test]
    fn cancel_is_offered_only_before_anything_changed() {
        let mut modal = OperationModal::new(OperationKind::Create);
        run(&mut modal, OperationStage::PreHook, None);
        assert_eq!(
            screen(&mut show(modal.clone()), 60, 20)[19],
            " Esc cancel  ↑/↓ scroll output"
        );

        modal.apply(OperationEvent::MutationStarted);
        assert_eq!(screen(&mut show(modal), 60, 20)[19], " ↑/↓ scroll output");
    }

    #[test]
    fn a_rolled_back_failure_names_the_hook_the_state_and_the_way_back() {
        let mut modal = OperationModal::new(OperationKind::Create);
        run(&mut modal, OperationStage::CreateWorktree, Some(true));
        run(&mut modal, OperationStage::PostHook, Some(false));
        run(&mut modal, OperationStage::Rollback, Some(true));
        output(&mut modal, "error: lockfile is out of date");
        modal.fail(&OperationFailure {
            stage: OperationStage::PostHook,
            mutation_state: MutationState::RolledBack,
            class: ErrorClass::Hook,
            message: "command failed: `bun install --frozen-lockfile` exited with code 1"
                .to_string(),
            retained_quarantine: None,
        });
        let screen = screen(&mut show(modal), 60, 20);

        assert!(
            screen[1].starts_with("  ╭ Create failed ─"),
            "{}",
            screen[1]
        );
        assert_eq!(
            screen[2..12],
            [
                "  │ × post_create hook failed: `bun install              │",
                "  │ --frozen-lockfile` exited with code 1                │",
                "  │ Nothing was created; the worktree and branch were    │",
                "  │ rolled back.                                         │",
                "  │                                                      │",
                "  │ ✓ Create worktree                                    │",
                "  │ × post_create hook                                   │",
                "  │ ✓ Roll back                                          │",
                "  │                                                      │",
                "  │   error: lockfile is out of date                     │",
            ]
        );
        assert_eq!(screen[19], " Enter back to form  Esc close");
    }

    #[test]
    fn a_hook_failure_after_the_change_says_the_change_succeeded_and_can_be_closed() {
        let mut modal = OperationModal::new(OperationKind::Sync);
        run(&mut modal, OperationStage::Sync, Some(true));
        run(&mut modal, OperationStage::PostHook, Some(false));
        modal.fail(&OperationFailure {
            stage: OperationStage::PostHook,
            mutation_state: MutationState::Applied,
            class: ErrorClass::Hook,
            message: "command failed: `make gen` exited with code 9".to_string(),
            retained_quarantine: None,
        });
        let screen = screen(&mut show(modal), 60, 20);

        assert!(
            screen[1].starts_with("  ╭ Synced, but a hook failed ─"),
            "{}",
            screen[1]
        );
        assert_eq!(
            screen[2..5],
            [
                "  │ × post_sync hook failed: `make gen` exited with code │",
                "  │ 9                                                    │",
                "  │ The sync itself succeeded; only the hook failed.     │",
            ]
        );
        assert_eq!(screen[19], " Enter close");
    }

    #[test]
    fn a_partial_failure_names_the_recovery_path() {
        let mut modal = OperationModal::new(OperationKind::Remove);
        run(&mut modal, OperationStage::Revalidate, Some(false));
        modal.fail(&OperationFailure {
            stage: OperationStage::RemoveWorktree,
            mutation_state: MutationState::PartiallyApplied,
            class: ErrorClass::Io,
            message: "I/O error: Permission denied (os error 13)".to_string(),
            retained_quarantine: Some("/wt/.trench-remove-1-0".into()),
        });
        let screen = screen(&mut show(modal), 60, 20).join("\n");

        assert!(screen.contains("× Checks"), "{screen}");
        assert!(screen.contains("× I/O error: Permission denied (os error 13)"));
        assert!(screen.contains("Some changes were applied; check the worktree before"));
        assert!(screen.contains("Kept for recovery: /wt/.trench-remove-1-0"));
    }

    fn partial_failure(recovery: &str) -> OperationModal {
        let mut modal = OperationModal::new(OperationKind::Remove);
        run(&mut modal, OperationStage::PreHook, Some(true));
        run(&mut modal, OperationStage::RemoveWorktree, Some(false));
        modal.fail(&OperationFailure {
            stage: OperationStage::RemoveWorktree,
            mutation_state: MutationState::PartiallyApplied,
            class: ErrorClass::Io,
            message: "I/O error: Permission denied (os error 13)".to_string(),
            retained_quarantine: Some(recovery.into()),
        });
        modal
    }

    #[test]
    fn a_failure_report_opens_on_what_failed_and_what_is_left_behind() {
        let screen = screen(&mut show(partial_failure("/wt/.trench-remove-1-0")), 40, 10);

        assert_eq!(
            screen[1..5],
            [
                " │ × I/O error: Permission denied (os │",
                " │ error 13)                          │",
                " │ Kept for recovery:                 │",
                " │ /wt/.trench-remove-1-0             │",
            ]
        );
        assert_eq!(screen[9], " Enter close  ↑/↓ scroll");
    }

    #[test]
    fn a_failure_report_keeps_the_newest_output_whole_and_within_reach() {
        let mut modal = OperationModal::new(OperationKind::Create);
        run(&mut modal, OperationStage::PostHook, Some(false));
        for index in 0..3000 {
            output(&mut modal, &format!("progress {index}"));
        }
        output(
            &mut modal,
            "error: the lockfile is out of date; run `bun install` and commit bun.lock",
        );
        modal.fail(&OperationFailure {
            stage: OperationStage::PostHook,
            mutation_state: MutationState::RolledBack,
            class: ErrorClass::Hook,
            message: "command failed: `bun install` exited with code 1".to_string(),
            retained_quarantine: None,
        });
        let area = ratatui::layout::Rect::new(0, 0, 60, 20);
        let (overflow, page) = super::report_geometry(&modal, area);
        modal.set_report_geometry(overflow, page);

        assert!(
            overflow < 130,
            "the report holds a bounded tail, not every line: {overflow}"
        );
        let top = screen(&mut show(modal.clone()), 60, 20).join("\n");
        assert!(top.contains("… 2901 earlier lines not shown"), "{top}");

        // One key reaches the end, where the last line is shown in full.
        modal.handle_key(crate::tui::operation_modal::ModalKey::End);
        let bottom = screen(&mut show(modal), 60, 20);
        let text = bottom.join("").replace(['│', ' '], "");
        assert!(
            text.contains("error:thelockfileisoutofdate;run`buninstall`andcommitbun.lock"),
            "{bottom:#?}"
        );
    }

    #[test]
    fn every_line_of_a_long_failure_report_can_be_scrolled_into_view() {
        let recovery = format!("/{}.trench-remove-77-0", "deep/".repeat(34));
        let area = ratatui::layout::Rect::new(0, 0, 40, 10);
        let mut modal = partial_failure(&recovery);
        let (overflow, page) = super::report_geometry(&modal, area);
        assert!(overflow > 0, "the report is taller than the dialog");

        let mut seen = String::new();
        for _ in 0..=overflow {
            modal.set_report_geometry(overflow, page);
            let screen = screen(&mut show(modal.clone()), 40, 10);
            seen.push_str(&screen[1..8].join("").replace(['│', ' '], ""));
            modal.handle_key(crate::tui::operation_modal::ModalKey::Down);
        }

        assert!(seen.contains(".trench-remove-77-0"), "{seen}");
        assert!(seen.contains("Somechangeswereapplied"), "{seen}");
        assert!(seen.contains("×Removeworktree"), "{seen}");
        assert_eq!(
            modal.report_scroll(),
            overflow,
            "scrolling stops at the end"
        );
    }

    #[test]
    fn a_narrow_failure_dialog_still_says_how_to_reach_the_hidden_output() {
        let mut modal = OperationModal::new(OperationKind::Create);
        run(&mut modal, OperationStage::PreHook, Some(false));
        for index in 0..12 {
            output(&mut modal, &format!("hook line {index}"));
        }
        modal.fail(&OperationFailure {
            stage: OperationStage::PreHook,
            mutation_state: MutationState::NotStarted,
            class: ErrorClass::Hook,
            message: "command failed: `false` exited with code 1".to_string(),
            retained_quarantine: None,
        });

        // Output is below the fold, so the hint that reveals it must survive
        // at the narrowest width, next to both ways out.
        assert_eq!(
            screen(&mut show(modal.clone()), 40, 10)[9],
            " Enter back  Esc close  ↑/↓ scroll"
        );
        assert_eq!(
            screen(&mut show(modal), 100, 30)[29],
            " Enter back to form  Esc close  ↑/↓ PgUp/PgDn scroll"
        );
    }

    #[test]
    fn the_prune_hint_is_shown_only_when_the_removed_worktree_had_no_directory() {
        let mut gone = partial_failure("/wt/q");
        gone.fail(&OperationFailure {
            stage: OperationStage::Prune,
            mutation_state: MutationState::PartiallyApplied,
            class: ErrorClass::Io,
            message: "I/O error".to_string(),
            retained_quarantine: None,
        });
        let present = screen(&mut show(gone.clone()), 100, 30).join("\n");
        assert!(
            present.contains("check the worktree before retrying"),
            "{present}"
        );
        assert!(!present.contains("git worktree prune"), "{present}");

        gone.set_directory_missing(true);
        let missing = screen(&mut show(gone), 100, 30).join("\n");
        assert!(missing.contains("run `git worktree prune`"), "{missing}");
    }

    #[test]
    fn a_report_that_fits_offers_no_scroll_key() {
        let screen = screen(&mut show(partial_failure("/wt/q")), 80, 24);

        assert_eq!(screen[23], " Enter close");
    }

    #[test]
    fn the_view_fits_the_minimum_terminal() {
        let mut modal = OperationModal::new(OperationKind::Remove);
        run(&mut modal, OperationStage::RemoveWorktree, None);
        let screen = screen(&mut show(modal), 40, 10);

        assert!(
            screen[0].starts_with(" ╭ Removing worktree ─"),
            "{}",
            screen[0]
        );
        assert_eq!(screen[8], " ╰────────────────────────────────────╯");
    }
}
