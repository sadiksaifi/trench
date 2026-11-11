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
    if let ModalStatus::Failed {
        stage,
        mutation_state,
        message,
        retained_quarantine,
    } = modal.status()
    {
        let hook = report::hook_name(modal.operation(), *stage);
        let headline = match hook {
            Some(hook) => report::hook_failure_message(hook, message),
            None => message.clone(),
        };
        lines.push(Line::default());
        lines.extend(
            widgets::wrap(&format!("{} {headline}", widgets::FAILED), width)
                .into_iter()
                .map(|line| Line::styled(line, theme.error)),
        );
        // Without this sentence the user cannot tell whether a failed
        // operation left anything behind.
        let state = report::state_after_failure(modal.operation(), *mutation_state, true)
            .unwrap_or("nothing was changed");
        lines.extend(
            widgets::wrap(&sentence(state), width)
                .into_iter()
                .map(|line| Line::styled(line, theme.text)),
        );
        if let Some(path) = retained_quarantine {
            lines.extend(
                widgets::wrap(&format!("Kept for recovery: {}", path.display()), width)
                    .into_iter()
                    .map(|line| Line::styled(line, theme.text)),
            );
        }
    }

    // Hook output takes whatever rows are left, newest at the bottom.
    let output_rows = rows.saturating_sub(lines.len() + 1);
    let output = modal.visible_hook_lines(output_rows);
    if !output.is_empty() {
        lines.push(Line::default());
        lines.extend(output.into_iter().map(|line| {
            Line::styled(
                format!("  {}", text::truncate_end(line, width.saturating_sub(2))),
                theme.muted,
            )
        }));
    }

    frame.render_widget(
        Paragraph::new(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(frame, layout.hints, &hints(modal), theme);
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

fn hints(modal: &OperationModal) -> Vec<Hint> {
    let mut hints = Vec::new();
    match modal.status() {
        ModalStatus::Failed { .. } if modal.can_return_to_form() => {
            hints.push(Hint::new("Enter", "back to form", 0));
            hints.push(Hint::new("Esc", "close", 1));
        }
        ModalStatus::Failed { .. } => hints.push(Hint::new("Enter", "close", 0)),
        ModalStatus::Running if modal.can_cancel() => hints.push(Hint::new("Esc", "cancel", 0)),
        ModalStatus::Running | ModalStatus::Succeeded => {}
    }
    hints.push(Hint::new("↑/↓", "scroll output", 2));
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
                "  │ ✓ Create worktree                                    │",
                "  │ × post_create hook                                   │",
                "  │ ✓ Roll back                                          │",
                "  │                                                      │",
                "  │ × post_create hook failed: `bun install              │",
                "  │ --frozen-lockfile` exited with code 1                │",
                "  │ Nothing was created; the worktree and branch were    │",
                "  │ rolled back.                                         │",
                "  │                                                      │",
                "  │   error: lockfile is out of date                     │",
            ]
        );
        assert_eq!(
            screen[19],
            " Enter back to form  Esc close  ↑/↓ scroll output"
        );
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
            screen[5..8],
            [
                "  │ × post_sync hook failed: `make gen` exited with code │",
                "  │ 9                                                    │",
                "  │ The sync itself succeeded; only the hook failed.     │",
            ]
        );
        assert_eq!(screen[19], " Enter close  ↑/↓ scroll output");
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
        assert!(screen.iter().all(|row| row.chars().count() <= 40));
    }
}
