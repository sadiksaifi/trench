use std::{path::PathBuf, time::Duration};

use crate::operation::{
    MutationState, OperationEvent, OperationFailure, OperationKind, OperationStage,
};

/// The most characters of one hook output line the dialog keeps.
///
/// The dialog is redrawn many times a second, and every stored line may be
/// measured and wrapped on each frame. A hook that prints megabytes without
/// a newline must not make that work unbounded.
pub const MAX_OUTPUT_LINE_CHARS: usize = 2_000;

/// The fewest newest hook output lines the dialog keeps. Older lines are
/// dropped in batches once twice this many are stored, so the store stays
/// bounded however long a hook prints.
pub const MAX_OUTPUT_LINES: usize = 1_000;

fn shorten_output_line(line: String) -> String {
    match line.char_indices().nth(MAX_OUTPUT_LINE_CHARS) {
        None => line,
        Some((cut, _)) => {
            let dropped = line[cut..].chars().count();
            format!("{}… ({dropped} more characters)", &line[..cut])
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageView {
    pub stage: OperationStage,
    pub duration: Option<Duration>,
    pub success: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalStatus {
    Running,
    Succeeded,
    Failed {
        stage: OperationStage,
        mutation_state: MutationState,
        message: String,
        retained_quarantine: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKey {
    Enter,
    Escape,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalEffect {
    Cancel,
    ReturnToForm,
    /// Dismiss a finished failure and go back to the list.
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationModal {
    operation: OperationKind,
    stages: Vec<StageView>,
    current_stage: Option<OperationStage>,
    elapsed: Duration,
    spinner_tick: u64,
    mutation_started: bool,
    hook_lines: Vec<String>,
    /// Older hook output lines that were dropped to keep the store bounded.
    dropped_hook_lines: usize,
    hook_scroll: usize,
    /// Rows the failure report is scrolled down by.
    report_scroll: usize,
    /// How far the failure report can scroll at the current terminal size.
    report_scroll_limit: usize,
    /// Rows one PgUp or PgDn moves: the height of the dialog's text area.
    page: usize,
    /// The operation removes a worktree whose directory is already gone.
    directory_missing: bool,
    warnings: Vec<String>,
    status: ModalStatus,
}

impl OperationModal {
    pub fn new(operation: OperationKind) -> Self {
        Self {
            operation,
            stages: Vec::new(),
            current_stage: None,
            elapsed: Duration::ZERO,
            spinner_tick: 0,
            mutation_started: operation == OperationKind::Sync,
            hook_lines: Vec::new(),
            dropped_hook_lines: 0,
            hook_scroll: 0,
            report_scroll: 0,
            report_scroll_limit: 0,
            page: 1,
            directory_missing: false,
            warnings: Vec::new(),
            status: ModalStatus::Running,
        }
    }

    pub fn apply(&mut self, event: OperationEvent) {
        match event {
            OperationEvent::Started { operation } => {
                self.operation = operation;
            }
            OperationEvent::StageStarted { stage } => {
                self.current_stage = Some(stage);
                self.stages.push(StageView {
                    stage,
                    duration: None,
                    success: None,
                });
            }
            OperationEvent::MutationStarted => self.mutation_started = true,
            OperationEvent::Output { line, .. } => {
                if self.hook_scroll > 0 {
                    self.hook_scroll = self.hook_scroll.saturating_add(1);
                }
                self.push_hook_line(shorten_output_line(line));
            }
            OperationEvent::StageFinished {
                stage,
                duration,
                success,
            } => {
                if let Some(view) = self
                    .stages
                    .iter_mut()
                    .rev()
                    .find(|view| view.stage == stage)
                {
                    view.duration = Some(duration);
                    view.success = Some(success);
                }
                if self.current_stage == Some(stage) {
                    self.current_stage = None;
                }
            }
            OperationEvent::Warning { message, .. } => self.warnings.push(message),
            OperationEvent::Finished { duration, .. } => {
                self.elapsed = duration;
                self.current_stage = None;
            }
        }
    }

    pub fn tick(&mut self, elapsed: Duration) {
        if self.status == ModalStatus::Running {
            self.elapsed = elapsed;
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
        }
    }

    pub fn succeed(&mut self) {
        self.current_stage = None;
        self.status = ModalStatus::Succeeded;
    }

    pub fn fail(&mut self, failure: &OperationFailure) {
        self.current_stage = Some(failure.stage);
        self.status = ModalStatus::Failed {
            stage: failure.stage,
            mutation_state: failure.mutation_state,
            message: failure.message.clone(),
            retained_quarantine: failure.retained_quarantine.clone(),
        };
    }

    pub fn handle_key(&mut self, key: ModalKey) -> Option<ModalEffect> {
        match key {
            ModalKey::Escape if self.status == ModalStatus::Running && !self.mutation_started => {
                Some(ModalEffect::Cancel)
            }
            ModalKey::Enter if self.can_return_to_form() => Some(ModalEffect::ReturnToForm),
            // A failure never traps the user: once the operation has stopped,
            // Enter and Esc always lead somewhere.
            ModalKey::Enter | ModalKey::Escape
                if matches!(self.status, ModalStatus::Failed { .. }) =>
            {
                Some(ModalEffect::Close)
            }
            // A failure is a report read from the top: what failed, what
            // state things are in, then progress and output. It scrolls as
            // one document so every line of it can be reached.
            ModalKey::Up
            | ModalKey::Down
            | ModalKey::PageUp
            | ModalKey::PageDown
            | ModalKey::Home
            | ModalKey::End => {
                self.scroll(key);
                None
            }
            ModalKey::Enter | ModalKey::Escape => None,
        }
    }

    fn scroll(&mut self, key: ModalKey) {
        let page = self.page.max(1);
        if matches!(self.status, ModalStatus::Failed { .. }) {
            // The report reads top to bottom, so Down moves toward its end.
            let limit = self.report_scroll_limit;
            self.report_scroll = match key {
                ModalKey::Up => self.report_scroll.saturating_sub(1),
                ModalKey::Down => self.report_scroll + 1,
                ModalKey::PageUp => self.report_scroll.saturating_sub(page),
                ModalKey::PageDown => self.report_scroll + page,
                ModalKey::Home => 0,
                ModalKey::End => limit,
                ModalKey::Enter | ModalKey::Escape => self.report_scroll,
            }
            .min(limit);
        } else {
            // Live output follows its newest line; Up looks back from there.
            let limit = self.hook_lines.len().saturating_sub(1);
            self.hook_scroll = match key {
                ModalKey::Up => self.hook_scroll + 1,
                ModalKey::Down => self.hook_scroll.saturating_sub(1),
                ModalKey::PageUp => self.hook_scroll + page,
                ModalKey::PageDown => self.hook_scroll.saturating_sub(page),
                ModalKey::Home => limit,
                ModalKey::End => 0,
                ModalKey::Enter | ModalKey::Escape => self.hook_scroll,
            }
            .min(limit);
        }
    }

    /// The failure changed nothing, so the form that started it is still valid.
    pub fn can_return_to_form(&self) -> bool {
        matches!(
            self.status,
            ModalStatus::Failed {
                mutation_state: MutationState::NotStarted | MutationState::RolledBack,
                ..
            }
        )
    }

    /// Esc stops the operation: it is running and has not changed anything.
    pub fn can_cancel(&self) -> bool {
        self.status == ModalStatus::Running && !self.mutation_started
    }

    pub fn operation(&self) -> OperationKind {
        self.operation
    }

    pub fn stages(&self) -> &[StageView] {
        &self.stages
    }

    pub fn current_stage(&self) -> Option<OperationStage> {
        self.current_stage
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn spinner_tick(&self) -> u64 {
        self.spinner_tick
    }

    pub fn spinner_visible(&self) -> bool {
        self.status == ModalStatus::Running && self.current_stage.is_some()
    }

    pub fn directory_missing(&self) -> bool {
        self.directory_missing
    }

    fn push_hook_line(&mut self, line: String) {
        self.hook_lines.push(line);
        if self.hook_lines.len() <= MAX_OUTPUT_LINES * 2 {
            return;
        }
        let drop = self.hook_lines.len() - MAX_OUTPUT_LINES;
        self.hook_lines.drain(..drop);
        self.dropped_hook_lines += drop;
        self.hook_scroll = self
            .hook_scroll
            .min(self.hook_lines.len().saturating_sub(1));
    }

    /// How many of the oldest hook output lines are no longer stored.
    pub fn dropped_hook_lines(&self) -> usize {
        self.dropped_hook_lines
    }

    pub fn set_directory_missing(&mut self, missing: bool) {
        self.directory_missing = missing;
    }

    pub fn hook_lines(&self) -> &[String] {
        &self.hook_lines
    }

    pub fn report_scroll(&self) -> usize {
        self.report_scroll.min(self.report_scroll_limit)
    }

    /// Tell the modal how many rows of the failure report are off screen
    /// and how many rows one page is. The view knows the geometry; the modal
    /// only keeps the position.
    pub fn set_report_geometry(&mut self, overflow: usize, page: usize) {
        self.report_scroll_limit = overflow;
        self.report_scroll = self.report_scroll.min(overflow);
        self.page = page;
    }

    pub fn visible_hook_lines(&self, height: usize) -> Vec<&str> {
        let end = self.hook_lines.len().saturating_sub(self.hook_scroll);
        let start = end.saturating_sub(height);
        self.hook_lines[start..end]
            .iter()
            .map(String::as_str)
            .collect()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn status(&self) -> &ModalStatus {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        hooks::{
            types::{HookStep, OutputStream},
            HookEvent,
        },
        operation::{
            ErrorClass, MutationState, OperationEvent, OperationFailure, OperationKind,
            OperationStage,
        },
    };

    #[test]
    fn modal_tracks_stages_elapsed_hooks_and_the_mutation_cancellation_boundary() {
        let mut modal = OperationModal::new(OperationKind::Create);
        modal.apply(OperationEvent::Started {
            operation: OperationKind::Create,
        });
        modal.apply(OperationEvent::StageStarted {
            stage: OperationStage::Revalidate,
        });
        modal.tick(Duration::from_millis(1_250));

        assert_eq!(modal.current_stage(), Some(OperationStage::Revalidate));
        assert_eq!(modal.elapsed(), Duration::from_millis(1_250));
        assert!(modal.spinner_visible());
        assert_eq!(
            modal.handle_key(ModalKey::Escape),
            Some(ModalEffect::Cancel)
        );

        modal.apply(OperationEvent::MutationStarted);
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PreCreate,
            step: HookStep::Run,
            stream: OutputStream::Stdout,
            line: "setting up".to_string(),
        });
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PreCreate,
            step: HookStep::Shell,
            stream: OutputStream::Stderr,
            line: "checking tools".to_string(),
        });

        assert!(!modal.can_cancel());
        assert_eq!(modal.handle_key(ModalKey::Escape), None);
        assert_eq!(modal.visible_hook_lines(1), ["checking tools"]);
        modal.handle_key(ModalKey::Up);
        assert_eq!(modal.visible_hook_lines(1), ["setting up"]);

        modal.fail(&OperationFailure {
            stage: OperationStage::PostHook,
            mutation_state: MutationState::RolledBack,
            class: ErrorClass::Hook,
            message: "post-create hook failed".to_string(),
            retained_quarantine: None,
        });
        assert_eq!(
            modal.status(),
            &ModalStatus::Failed {
                stage: OperationStage::PostHook,
                mutation_state: MutationState::RolledBack,
                message: "post-create hook failed".to_string(),
                retained_quarantine: None,
            }
        );
        assert_eq!(
            modal.handle_key(ModalKey::Enter),
            Some(ModalEffect::ReturnToForm)
        );
    }

    #[test]
    fn partially_applied_failure_keeps_recovery_truth_and_cannot_return_to_form() {
        let quarantine = PathBuf::from("/tmp/retained-quarantine");
        let mut modal = OperationModal::new(OperationKind::Remove);
        modal.fail(&OperationFailure {
            stage: OperationStage::RemoveWorktree,
            mutation_state: MutationState::PartiallyApplied,
            class: ErrorClass::Git,
            message: "restoration failed".to_string(),
            retained_quarantine: Some(quarantine.clone()),
        });

        assert_eq!(
            modal.status(),
            &ModalStatus::Failed {
                stage: OperationStage::RemoveWorktree,
                mutation_state: MutationState::PartiallyApplied,
                message: "restoration failed".to_string(),
                retained_quarantine: Some(quarantine),
            }
        );
        assert!(!modal.can_return_to_form());
        assert_eq!(modal.handle_key(ModalKey::Enter), Some(ModalEffect::Close));
        assert_eq!(modal.handle_key(ModalKey::Escape), Some(ModalEffect::Close));
    }

    #[test]
    fn an_enormous_output_line_is_stored_shortened_and_says_how_much_was_cut() {
        let mut modal = OperationModal::new(OperationKind::Create);
        modal.apply(OperationEvent::Output {
            hook: crate::hooks::HookEvent::PreCreate,
            step: crate::hooks::types::HookStep::Shell,
            stream: crate::hooks::types::OutputStream::Stdout,
            line: "x".repeat(1_000_000),
        });
        modal.apply(OperationEvent::Output {
            hook: crate::hooks::HookEvent::PreCreate,
            step: crate::hooks::types::HookStep::Shell,
            stream: crate::hooks::types::OutputStream::Stdout,
            line: "short".to_string(),
        });

        let stored = &modal.hook_lines()[0];
        assert!(
            stored.chars().count() < MAX_OUTPUT_LINE_CHARS + 60,
            "{}",
            stored.len()
        );
        assert!(stored.starts_with(&"x".repeat(MAX_OUTPUT_LINE_CHARS)));
        assert!(
            stored.ends_with(&format!(
                "… ({} more characters)",
                1_000_000 - MAX_OUTPUT_LINE_CHARS
            )),
            "{}",
            &stored[stored.len() - 60..]
        );
        assert_eq!(modal.hook_lines()[1], "short");
    }

    #[test]
    fn a_hook_that_prints_without_end_keeps_its_newest_lines_and_counts_the_rest() {
        let mut modal = OperationModal::new(OperationKind::Create);
        let total = MAX_OUTPUT_LINES * 5 + 7;
        for index in 0..total {
            modal.apply(OperationEvent::Output {
                hook: crate::hooks::HookEvent::PreCreate,
                step: crate::hooks::types::HookStep::Shell,
                stream: crate::hooks::types::OutputStream::Stdout,
                line: format!("line {index}"),
            });
        }

        let lines = modal.hook_lines();
        assert!(lines.len() <= MAX_OUTPUT_LINES * 2, "{}", lines.len());
        assert!(lines.len() >= MAX_OUTPUT_LINES);
        assert_eq!(modal.dropped_hook_lines(), total - lines.len());
        for (offset, line) in lines.iter().enumerate() {
            assert_eq!(
                *line,
                format!("line {}", modal.dropped_hook_lines() + offset)
            );
        }
    }

    #[test]
    fn a_failure_report_scrolls_as_one_document_within_its_limit() {
        let mut modal = OperationModal::new(OperationKind::Remove);
        modal.fail(&OperationFailure {
            stage: OperationStage::RemoveWorktree,
            mutation_state: MutationState::PartiallyApplied,
            class: crate::operation::ErrorClass::Io,
            message: "I/O error".to_string(),
            retained_quarantine: None,
        });

        modal.handle_key(ModalKey::Down);
        assert_eq!(modal.report_scroll(), 0, "nothing is off screen yet");

        modal.set_report_geometry(2, 7);
        for _ in 0..5 {
            modal.handle_key(ModalKey::Down);
        }
        assert_eq!(modal.report_scroll(), 2);
        modal.handle_key(ModalKey::Up);
        assert_eq!(modal.report_scroll(), 1);

        modal.set_report_geometry(0, 7);
        assert_eq!(
            modal.report_scroll(),
            0,
            "a taller terminal resets the view"
        );

        // A long report is crossed a page at a time, or in one key.
        modal.set_report_geometry(40, 7);
        modal.handle_key(ModalKey::PageDown);
        modal.handle_key(ModalKey::PageDown);
        assert_eq!(modal.report_scroll(), 14);
        modal.handle_key(ModalKey::PageUp);
        assert_eq!(modal.report_scroll(), 7);
        modal.handle_key(ModalKey::End);
        assert_eq!(modal.report_scroll(), 40);
        modal.handle_key(ModalKey::PageDown);
        assert_eq!(modal.report_scroll(), 40, "the end is the end");
        modal.handle_key(ModalKey::Home);
        assert_eq!(modal.report_scroll(), 0);
    }

    #[test]
    fn a_hook_failure_after_the_change_can_always_be_closed() {
        let mut modal = OperationModal::new(OperationKind::Sync);
        modal.fail(&OperationFailure {
            stage: OperationStage::PostHook,
            mutation_state: MutationState::Applied,
            class: crate::operation::ErrorClass::Hook,
            message: "command failed: `make gen` exited with code 9".to_string(),
            retained_quarantine: None,
        });

        assert!(!modal.can_cancel());
        assert_eq!(modal.handle_key(ModalKey::Enter), Some(ModalEffect::Close));
        assert_eq!(modal.handle_key(ModalKey::Escape), Some(ModalEffect::Close));
    }

    #[test]
    fn a_failure_that_changed_nothing_offers_the_form_on_enter_and_the_list_on_escape() {
        let mut modal = OperationModal::new(OperationKind::Create);
        modal.fail(&OperationFailure {
            stage: OperationStage::PreHook,
            mutation_state: MutationState::NotStarted,
            class: crate::operation::ErrorClass::Hook,
            message: "command failed: `false` exited with code 1".to_string(),
            retained_quarantine: None,
        });

        assert!(modal.can_return_to_form());
        assert_eq!(
            modal.handle_key(ModalKey::Enter),
            Some(ModalEffect::ReturnToForm)
        );
        assert_eq!(modal.handle_key(ModalKey::Escape), Some(ModalEffect::Close));
    }

    #[test]
    fn remove_modal_tracks_all_shared_stages_and_locks_cancellation_at_mutation() {
        let mut modal = OperationModal::new(OperationKind::Remove);
        for stage in [
            OperationStage::Revalidate,
            OperationStage::PreHook,
            OperationStage::RemoveWorktree,
            OperationStage::Prune,
            OperationStage::DeleteBranch,
            OperationStage::PostHook,
        ] {
            modal.apply(OperationEvent::StageStarted { stage });
            modal.apply(OperationEvent::StageFinished {
                stage,
                duration: Duration::from_millis(10),
                success: true,
            });
        }
        modal.tick(Duration::from_millis(750));
        assert_eq!(modal.elapsed(), Duration::from_millis(750));
        assert_eq!(modal.stages().len(), 6);
        assert_eq!(
            modal.handle_key(ModalKey::Escape),
            Some(ModalEffect::Cancel)
        );
        modal.apply(OperationEvent::MutationStarted);
        assert_eq!(modal.handle_key(ModalKey::Escape), None);
    }

    #[test]
    fn sync_modal_is_non_cancellable_from_its_first_frame() {
        let mut modal = OperationModal::new(OperationKind::Sync);

        assert!(!modal.can_cancel());
        assert_eq!(modal.handle_key(ModalKey::Escape), None);
    }
}
