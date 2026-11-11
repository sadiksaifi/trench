use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};

use crate::{
    cli::commands::sync::stateless::{HookPolicy as SyncHookPolicy, SyncPlanner},
    cli::report,
    config::{self, HooksConfig},
    create_plan::{CreateAction, CreatePlanner, HookPolicy},
    navigation::EditorCommand,
    operation::{CreateRequest, OperationOutcome, OperationRequest, SyncRequest},
    tui::{
        app::{self, AppState, Effect, Event, HookNames, NoticeKind, WorktreeId},
        create_flow::{
            BranchRow, CreateDialog, CreateEffect, CreateKey, CreateSubmission, ExistingWorktree,
            OriginRefresh, RowKind,
        },
        keymap::Key,
        line_input::LineEdit,
        operation_modal::{ModalEffect, ModalKey},
        operation_runtime::{
            OperationRuntime, OperationRuntimeEffect, SystemRuntimeClock, ThreadOperationLauncher,
        },
        refresh::RefreshPublication,
        refresh_runtime::RefreshRuntime,
        remove_flow::{RemoveDialog, RemoveEffect, RemoveKey},
        sync_flow::{SyncDialog, SyncEffect, SyncKey, SyncSubmission},
        theme, view,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiExit {
    Quit,
    Switch(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshCause {
    EditorReturn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorStatus {
    pub success: bool,
    pub code: Option<i32>,
}

pub trait TerminalDriver {
    fn suspend(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
}

pub trait EditorLauncher {
    fn launch(&mut self, command: &EditorCommand, path: &Path) -> Result<EditorStatus>;
}

trait RefreshDriver {
    fn refresh(&mut self, cause: RefreshCause) -> Result<()>;
}

fn open_editor(
    terminal: &mut impl TerminalDriver,
    launcher: &mut impl EditorLauncher,
    refresh: &mut impl RefreshDriver,
    command: &EditorCommand,
    path: &Path,
) -> Result<()> {
    terminal.suspend().context("failed to suspend terminal")?;
    let editor_result = launcher.launch(command, path);
    let resume_result = terminal.resume();
    let refresh_result = refresh.refresh(RefreshCause::EditorReturn);

    let status = editor_result.context("failed to launch editor")?;
    if !status.success {
        anyhow::bail!(
            "editor exited with status {}",
            status
                .code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
    }
    resume_result.context("failed to resume terminal")?;
    refresh_result.context("failed to refresh after editor return")?;
    Ok(())
}

struct RatatuiTerminalDriver<'a> {
    terminal: &'a mut ratatui::DefaultTerminal,
}

impl TerminalDriver for RatatuiTerminalDriver<'_> {
    fn suspend(&mut self) -> Result<()> {
        ratatui::restore();
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        *self.terminal = ratatui::init();
        Ok(())
    }
}

struct ProcessEditorLauncher;

impl EditorLauncher for ProcessEditorLauncher {
    fn launch(&mut self, command: &EditorCommand, path: &Path) -> Result<EditorStatus> {
        let status = command
            .status(path)
            .with_context(|| format!("could not start {}", command.program().to_string_lossy()))?;
        Ok(EditorStatus {
            success: status.success(),
            code: status.code(),
        })
    }
}

struct RuntimeRefreshDriver<'a> {
    runtime: &'a mut RefreshRuntime,
    state: &'a mut AppState,
}

impl RefreshDriver for RuntimeRefreshDriver<'_> {
    fn refresh(&mut self, _cause: RefreshCause) -> Result<()> {
        let result = self.runtime.editor_return();
        apply_refresh_publications(self.runtime, self.state);
        result?;
        Ok(())
    }
}

#[derive(Debug)]
enum CreateDispatch {
    Navigate(WorktreeId),
    Run(Box<OperationRequest>),
}

impl CreateDispatch {
    /// Whether this plan does exactly what the dialog showed for `row`: the
    /// same kind of action, on the same branch, in the same directory, and
    /// for a new branch from the same base.
    fn matches(&self, dialog: &CreateDialog, row: &BranchRow) -> bool {
        let plan = match (self, &row.kind) {
            // Nothing is created, so only the branch has to agree. The
            // worktree's directory may have moved since the list was read.
            (Self::Navigate(_), RowKind::HasWorktree { .. }) => return true,
            (Self::Run(request), _) => match &**request {
                OperationRequest::Create(create) => &create.plan,
                _ => return false,
            },
            _ => return false,
        };
        let same_action = match (&plan.action, &row.kind) {
            (CreateAction::NewBranch(base), RowKind::New) => dialog.base() == Some(base.as_str()),
            (CreateAction::ExistingLocal, RowKind::Local) => true,
            (CreateAction::TrackRemote(planned), RowKind::Remote { upstream }) => {
                planned == upstream
            }
            _ => false,
        };
        same_action && plan.branch == row.branch && plan.path == dialog.planned_path(&row.branch)
    }
}

/// Shown when live Git no longer matches the create dialog's summary.
const STALE_CREATE_SUMMARY: &str =
    "That branch changed while this dialog was open. Pick it again to see what Enter will do.";

#[derive(Debug)]
enum CreateInputEffect {
    Navigate(WorktreeId),
    Start(Box<OperationRequest>),
    /// The summary was stale: reload branches and worktrees.
    Refresh,
}

#[derive(Debug)]
enum SyncInputEffect {
    RefreshOrigin,
    Start(Box<OperationRequest>),
}

#[derive(Debug)]
enum RemoveInputEffect {
    Start(Box<OperationRequest>),
}

trait PostOperationRefresh {
    fn request_post_operation(&mut self) -> Result<()>;
    fn apply_publications(&mut self, state: &mut AppState);
}

impl PostOperationRefresh for RefreshRuntime {
    fn request_post_operation(&mut self) -> Result<()> {
        self.post_operation()?;
        Ok(())
    }

    fn apply_publications(&mut self, state: &mut AppState) {
        apply_refresh_publications(self, state);
    }
}

/// Every worktree the create dialog must know about: the branches that are
/// taken and the directories that are in use.
fn existing_worktrees(state: &AppState) -> Vec<ExistingWorktree> {
    state
        .identities
        .iter()
        .map(|identity| {
            ExistingWorktree::new(
                identity.branch.as_deref(),
                identity.id.clone(),
                identity.path.clone(),
            )
        })
        .collect()
}

fn open_create_dialog(
    state: &mut AppState,
    repository: &str,
    worktree_root: &Path,
    configured_base: Option<&str>,
) -> Result<()> {
    let refs = state
        .refs
        .clone()
        .context("Branches are still loading. Try again in a moment.")?;
    state.help_open = false;
    state.create_dialog = Some(CreateDialog::new(
        repository,
        worktree_root,
        refs,
        configured_base,
        existing_worktrees(state),
    ));
    Ok(())
}

fn open_sync_dialog(
    state: &mut AppState,
    target: &WorktreeId,
    configured_base: Option<&str>,
) -> Result<()> {
    let identity = state
        .selected_visible()
        .filter(|identity| &identity.id == target)
        .cloned()
        .context("selected worktree is no longer visible")?;
    if let Some(reason) = crate::tui::sync_flow::unavailable_reason(&identity) {
        anyhow::bail!(reason);
    }
    if crate::git::dirty_count(&identity.path)? > 0 {
        anyhow::bail!(
            "{} has uncommitted changes. Commit or stash them, then sync.",
            identity.label()
        );
    }
    let refs = state
        .refs
        .clone()
        .context("Branches are still loading. Try again in a moment.")?;
    state.help_open = false;
    state.sync_dialog = Some(SyncDialog::new(&identity, refs, configured_base));
    Ok(())
}

fn open_remove_dialog(
    state: &mut AppState,
    cwd: &Path,
    target: &WorktreeId,
    configured_base: Option<&str>,
) -> Result<()> {
    let selected = state
        .selected_visible()
        .filter(|identity| &identity.id == target)
        .context("selected worktree is no longer visible")?;
    if selected.is_main {
        anyhow::bail!("The main worktree cannot be removed.");
    }
    state.help_open = false;
    state.remove_dialog = Some(RemoveDialog::discover_selected(
        state,
        cwd,
        configured_base,
    )?);
    Ok(())
}

fn build_create_dispatch(
    submission: &CreateSubmission,
    cwd: &Path,
    worktree_root: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
) -> Result<CreateDispatch> {
    let repo = crate::git::discover_repo(cwd)?;
    let plan = CreatePlanner::discover(cwd, worktree_root, configured_base, HookPolicy::Run)?
        .plan(&submission.branch, submission.from.as_deref())?;
    if matches!(plan.action, CreateAction::Navigate(_)) {
        return Ok(CreateDispatch::Navigate(WorktreeId::new(plan.path)));
    }
    Ok(CreateDispatch::Run(Box::new(OperationRequest::Create(
        CreateRequest {
            plan,
            repo_path: repo.path,
            worktree_root: worktree_root.to_path_buf(),
            hooks,
        },
    ))))
}

fn build_sync_request(
    submission: &SyncSubmission,
    cwd: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
) -> Result<OperationRequest> {
    let target = submission.target.as_path().to_string_lossy();
    let plan = SyncPlanner::discover(cwd, configured_base)?.plan(
        &target,
        Some(&submission.base),
        submission.strategy,
        SyncHookPolicy::Run,
    )?;
    Ok(OperationRequest::Sync(SyncRequest { plan, hooks }))
}

fn handle_create_input(
    state: &mut AppState,
    key: CreateKey,
    cwd: &Path,
    worktree_root: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
) -> Result<Option<CreateInputEffect>> {
    let effect = state
        .create_dialog
        .as_mut()
        .and_then(|dialog| dialog.handle_key(key));
    match effect {
        Some(CreateEffect::Close) => {
            state.create_dialog = None;
            state.help_open = false;
        }
        Some(CreateEffect::Navigate(_)) | Some(CreateEffect::Submit(_)) => {
            let submission = match effect {
                Some(CreateEffect::Submit(submission)) => submission,
                Some(CreateEffect::Navigate(_)) => state
                    .create_dialog
                    .as_ref()
                    .and_then(CreateDialog::submission)
                    .context("selected branch is no longer available")?,
                _ => unreachable!("matched create submission effects"),
            };
            let dispatch = match build_create_dispatch(
                &submission,
                cwd,
                worktree_root,
                configured_base,
                hooks,
            ) {
                Ok(dispatch) => dispatch,
                // A base was sent for a "new" branch that now exists: the
                // summary is stale, which is not a mistake in the input.
                Err(error)
                    if matches!(
                        error.downcast_ref::<crate::create_plan::CreatePlanError>(),
                        Some(crate::create_plan::CreatePlanError::FromRequiresNewBranch { .. })
                    ) =>
                {
                    if let Some(dialog) = state.create_dialog.as_mut() {
                        dialog.set_validation_error(Some(STALE_CREATE_SUMMARY.to_string()));
                    }
                    return Ok(Some(CreateInputEffect::Refresh));
                }
                Err(error) => {
                    if let Some(dialog) = state.create_dialog.as_mut() {
                        dialog.set_validation_error(Some(error.to_string()));
                    }
                    return Ok(None);
                }
            };
            // The plan comes from live Git; the summary came from the last
            // refresh. When they name different actions, Enter would do
            // something the screen did not say, so it does nothing instead.
            let as_shown = state.create_dialog.as_ref().is_some_and(|dialog| {
                dialog
                    .highlighted_row()
                    .is_some_and(|row| dispatch.matches(dialog, &row))
            });
            if !as_shown {
                if let Some(dialog) = state.create_dialog.as_mut() {
                    dialog.set_validation_error(Some(STALE_CREATE_SUMMARY.to_string()));
                }
                return Ok(Some(CreateInputEffect::Refresh));
            }
            match dispatch {
                CreateDispatch::Navigate(id) => {
                    state.help_open = false;
                    return Ok(Some(CreateInputEffect::Navigate(id)));
                }
                CreateDispatch::Run(request) => {
                    state.help_open = false;
                    return Ok(Some(CreateInputEffect::Start(request)));
                }
            }
        }
        None => {}
    }
    Ok(None)
}

fn handle_sync_input(
    state: &mut AppState,
    key: SyncKey,
    cwd: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
) -> Result<Option<SyncInputEffect>> {
    match state
        .sync_dialog
        .as_mut()
        .and_then(|dialog| dialog.handle_key(key))
    {
        Some(SyncEffect::Close) => {
            state.sync_dialog = None;
            state.help_open = false;
        }
        Some(SyncEffect::RefreshOrigin) => {
            if let Some(dialog) = state.sync_dialog.as_mut() {
                dialog.set_origin_refresh(OriginRefresh::Loading);
            }
            return Ok(Some(SyncInputEffect::RefreshOrigin));
        }
        Some(SyncEffect::Submit(submission)) => {
            match build_sync_request(&submission, cwd, configured_base, hooks) {
                Ok(request) => {
                    state.help_open = false;
                    return Ok(Some(SyncInputEffect::Start(Box::new(request))));
                }
                Err(error) => {
                    if let Some(dialog) = state.sync_dialog.as_mut() {
                        dialog.set_validation_error(Some(error.to_string()));
                    }
                }
            }
        }
        None => {}
    }
    Ok(None)
}

fn handle_remove_input(
    state: &mut AppState,
    key: RemoveKey,
    cwd: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
    now: Instant,
) -> Result<Option<RemoveInputEffect>> {
    // `y` accepts the risks on screen. If the terminal cannot show them all,
    // there is nothing to accept yet.
    let area = ratatui::layout::Rect::new(0, 0, state.viewport.width, state.viewport.height);
    if key == RemoveKey::Confirm
        && state
            .remove_dialog
            .as_ref()
            .is_some_and(|dialog| !view::remove_shows_every_risk(dialog, area))
    {
        return Ok(None);
    }
    match state
        .remove_dialog
        .as_mut()
        .and_then(|dialog| dialog.handle_key(key, now))
    {
        Some(RemoveEffect::Close) => {
            state.remove_dialog = None;
            state.help_open = false;
        }
        Some(RemoveEffect::Submit) => {
            let result = state
                .remove_dialog
                .as_mut()
                .expect("remove effect requires dialog")
                .revalidate_request(cwd, configured_base, hooks);
            match result {
                Ok(request) => {
                    state.help_open = false;
                    return Ok(Some(RemoveInputEffect::Start(Box::new(request))));
                }
                Err(error) => {
                    if let Some(dialog) = state.remove_dialog.as_mut() {
                        dialog.set_validation_error(Some(error.to_string()));
                    }
                }
            }
        }
        None => {}
    }
    Ok(None)
}

fn finish_create_success(
    state: &mut AppState,
    refresh: &mut impl PostOperationRefresh,
    outcome: crate::operation::CreateOutcome,
    shown_at: Instant,
) {
    let refresh_result = refresh.request_post_operation();
    let (message, warning) = match refresh_result.as_ref() {
        Ok(()) => (report::created(&outcome.plan), None),
        Err(error) => {
            let warning = format!(
                "Created {}, but refresh failed; press r to refresh: {error}",
                outcome.plan.branch
            );
            (warning.clone(), Some(warning))
        }
    };
    let _ = app::reduce(
        state,
        Event::OperationSucceeded {
            // The catalog lists canonical paths. Without this, a worktree
            // root behind a symlink would never match the new row.
            select: Some(WorktreeId::new(
                crate::git::worktrees::canonicalize_lenient(&outcome.plan.path),
            )),
            message,
            shown_at,
        },
    );
    if refresh_result.is_ok() {
        refresh.apply_publications(state);
    } else {
        state.refresh.warning = warning;
    }
}

fn apply_create_input_effect(
    effect: Option<CreateInputEffect>,
    state: &mut AppState,
    refresh: &mut RefreshRuntime,
    operation: &mut OperationRuntime<ThreadOperationLauncher, SystemRuntimeClock>,
) {
    match effect {
        Some(CreateInputEffect::Navigate(id)) => match refresh.post_operation() {
            Ok(()) => {
                apply_refresh_publications(refresh, state);
                if state.identities.iter().any(|row| row.id == id) {
                    let _ = app::reduce(state, Event::Select(id));
                    state.create_dialog = None;
                    state.help_open = false;
                } else if let Some(dialog) = state.create_dialog.as_mut() {
                    dialog.set_validation_error(Some(
                        "Checked-out worktree changed; press Enter to revalidate".to_string(),
                    ));
                }
            }
            Err(error) => {
                if let Some(dialog) = state.create_dialog.as_mut() {
                    dialog.set_validation_error(Some(format!(
                        "Could not refresh checked-out worktree: {error}"
                    )));
                }
            }
        },
        Some(CreateInputEffect::Start(request)) => {
            operation.start(*request);
            state.operation_modal = operation.modal().cloned();
        }
        Some(CreateInputEffect::Refresh) => {
            if let Err(error) = refresh.post_operation() {
                tracing::warn!(%error, "failed to refresh a stale create dialog");
            }
            apply_refresh_publications(refresh, state);
        }
        None => {}
    }
}

fn apply_sync_input_effect(
    effect: Option<SyncInputEffect>,
    state: &mut AppState,
    refresh: &mut RefreshRuntime,
    operation: &mut OperationRuntime<ThreadOperationLauncher, SystemRuntimeClock>,
) {
    match effect {
        Some(SyncInputEffect::RefreshOrigin) => {
            if let Err(error) = refresh.ref_picker() {
                tracing::warn!(%error, "base picker origin refresh failed");
                if let Some(dialog) = state.sync_dialog.as_mut() {
                    dialog.set_origin_refresh(OriginRefresh::Failed);
                }
            }
            apply_refresh_publications(refresh, state);
        }
        Some(SyncInputEffect::Start(request)) => {
            operation.start(*request);
            state.operation_modal = operation.modal().cloned();
        }
        None => {}
    }
}

fn finish_sync_success(
    state: &mut AppState,
    refresh: &mut impl PostOperationRefresh,
    outcome: crate::cli::commands::sync::stateless::SyncOutcome,
    shown_at: Instant,
) {
    let refresh_result = refresh.request_post_operation();
    let summary = report::synced(&outcome);
    let (message, warning) = match refresh_result.as_ref() {
        Ok(()) => (summary, None),
        Err(error) => {
            let warning = format!("{summary}, but refresh failed; press r to refresh: {error}");
            (warning.clone(), Some(warning))
        }
    };
    let _ = app::reduce(
        state,
        Event::OperationSucceeded {
            select: Some(WorktreeId::new(outcome.path)),
            message,
            shown_at,
        },
    );
    if refresh_result.is_ok() {
        refresh.apply_publications(state);
    } else {
        state.refresh.warning = warning;
    }
}

fn finish_remove_success(
    state: &mut AppState,
    refresh: &mut impl PostOperationRefresh,
    outcome: crate::cli::commands::remove::stateless::RemovalOutcome,
    shown_at: Instant,
) {
    let refresh_result = refresh.request_post_operation();
    // The list names worktrees by branch, so the message does too.
    let label = outcome.branch.as_deref().unwrap_or(&outcome.worktree);
    let summary = if outcome.branch_deleted {
        format!("Removed {label} and deleted its branch")
    } else {
        format!("Removed {label}")
    };
    let (message, warning) = match refresh_result.as_ref() {
        Ok(()) => (summary, outcome.warning.clone()),
        Err(error) => {
            let warning = format!("{summary}, but refresh failed; press r to refresh: {error}");
            (warning.clone(), Some(warning))
        }
    };
    state.remove_dialog = None;
    let _ = app::reduce(
        state,
        Event::OperationSucceeded {
            select: None,
            message,
            shown_at,
        },
    );
    if refresh_result.is_ok() {
        refresh.apply_publications(state);
    }
    if warning.is_some() {
        state.refresh.warning = warning;
    }
}

fn return_to_create_form(
    state: &mut AppState,
    cwd: &Path,
    worktree_root: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
) {
    state.operation_modal = None;
    state.help_open = false;
    let Some(submission) = state
        .create_dialog
        .as_ref()
        .and_then(CreateDialog::submission)
    else {
        return;
    };
    let validation_error =
        match build_create_dispatch(&submission, cwd, worktree_root, configured_base, hooks) {
            Ok(CreateDispatch::Run(_)) => None,
            Ok(CreateDispatch::Navigate(_)) => {
                Some("Branch is now checked out in another worktree".to_string())
            }
            Err(error) => Some(error.to_string()),
        };
    if let Some(dialog) = state.create_dialog.as_mut() {
        dialog.set_validation_error(validation_error);
    }
}

fn return_to_sync_form(
    state: &mut AppState,
    cwd: &Path,
    configured_base: Option<&str>,
    hooks: Option<HooksConfig>,
) {
    state.operation_modal = None;
    state.help_open = false;
    let Some(submission) = state.sync_dialog.as_ref().and_then(SyncDialog::submission) else {
        return;
    };
    let validation_error = build_sync_request(&submission, cwd, configured_base, hooks)
        .err()
        .map(|error| error.to_string());
    if let Some(dialog) = state.sync_dialog.as_mut() {
        dialog.set_validation_error(validation_error);
    }
}

fn return_to_remove_form(state: &mut AppState, cwd: &Path, configured_base: Option<&str>) {
    state.operation_modal = None;
    state.help_open = false;
    let Some(dialog) = state.remove_dialog.as_mut() else {
        return;
    };
    // The failed attempt may have changed the worktree (a hook ran). Showing
    // fresh facts keeps the next `y` honest.
    let error = dialog
        .refresh(cwd, configured_base)
        .err()
        .map(|error| error.to_string());
    dialog.set_validation_error(error);
}

/// Leave a finished failure: drop the forms and show what Git has now.
fn close_after_failure(state: &mut AppState, refresh: &mut RefreshRuntime) {
    state.operation_modal = None;
    state.create_dialog = None;
    state.sync_dialog = None;
    state.remove_dialog = None;
    state.help_open = false;
    if let Err(error) = refresh.post_operation() {
        tracing::warn!(%error, "failed to refresh after a failed operation");
    }
    apply_refresh_publications(refresh, state);
}

/// Route a key to the operation dialog.
///
/// Esc cancels only while the operation runs and has changed nothing. Once
/// it has finished, Esc belongs to the dialog, which closes a failure.
fn operation_modal_effect(
    operation: &mut OperationRuntime<ThreadOperationLauncher, SystemRuntimeClock>,
    key: ModalKey,
    area: ratatui::layout::Rect,
) -> Option<ModalEffect> {
    let cancellable = operation
        .modal()
        .is_some_and(crate::tui::operation_modal::OperationModal::can_cancel);
    if key == ModalKey::Escape && cancellable {
        return operation.cancel().then_some(ModalEffect::Cancel);
    }
    let modal = operation.modal_mut()?;
    // The dialog can only scroll as far as the terminal hides lines.
    let (overflow, page) = view::operation_report_geometry(modal, area);
    modal.set_report_geometry(overflow, page);
    modal.handle_key(key)
}

struct OperationFormContext<'a> {
    cwd: &'a Path,
    worktree_root: &'a Path,
    configured_base: Option<&'a str>,
    hooks: Option<HooksConfig>,
}

fn apply_operation_modal_effect(
    effect: Option<ModalEffect>,
    state: &mut AppState,
    operation: &mut OperationRuntime<ThreadOperationLauncher, SystemRuntimeClock>,
    refresh: &mut RefreshRuntime,
    context: OperationFormContext<'_>,
) {
    match effect {
        Some(ModalEffect::Cancel) => {
            operation.dismiss();
            state.operation_modal = None;
        }
        Some(ModalEffect::ReturnToForm) => {
            operation.dismiss();
            if let Err(error) = refresh.post_operation() {
                tracing::warn!(%error, "failed to refresh before operation revalidation");
            }
            apply_refresh_publications(refresh, state);
            if state.remove_dialog.is_some() {
                return_to_remove_form(state, context.cwd, context.configured_base);
            } else if state.sync_dialog.is_some() {
                return_to_sync_form(state, context.cwd, context.configured_base, context.hooks);
            } else {
                return_to_create_form(
                    state,
                    context.cwd,
                    context.worktree_root,
                    context.configured_base,
                    context.hooks,
                );
            }
        }
        Some(ModalEffect::Close) => {
            operation.dismiss();
            close_after_failure(state, refresh);
        }
        None => {}
    }
    state.operation_modal = operation.modal().cloned();
}

fn notify(state: &mut AppState, kind: NoticeKind, message: impl Into<String>) {
    let _ = app::reduce(
        state,
        Event::NotificationShown {
            kind,
            message: message.into(),
            shown_at: Instant::now(),
        },
    );
}

/// The status-line wording for an editor that cannot be resolved.
fn editor_command_notice(error: &crate::navigation::EditorCommandError) -> String {
    match error {
        crate::navigation::EditorCommandError::Missing => {
            "No editor configured. Set $EDITOR or [editor] command.".to_string()
        }
        other => other.to_string(),
    }
}

/// Open the selected worktree in the editor and report any failure in the
/// status line. A missing or failing editor must never end the session.
fn open_in_editor(
    state: &mut AppState,
    terminal: &mut ratatui::DefaultTerminal,
    refresh: &mut RefreshRuntime,
    configured_editor: Option<&str>,
    path: &Path,
) {
    let command = match EditorCommand::from_environment(configured_editor) {
        Ok(command) => command,
        Err(error) => {
            notify(state, NoticeKind::Error, editor_command_notice(&error));
            return;
        }
    };
    let result = open_editor(
        &mut RatatuiTerminalDriver { terminal },
        &mut ProcessEditorLauncher,
        &mut RuntimeRefreshDriver {
            runtime: refresh,
            state,
        },
        &command,
        path,
    );
    if let Err(error) = result {
        tracing::warn!(error = %error, "editor navigation failed");
        notify(state, NoticeKind::Error, format!("{error:#}"));
    }
}

pub fn run() -> Result<TuiExit> {
    let cwd = std::env::current_dir()?;
    let repo = crate::git::discover_repo(&cwd)?;
    let global = config::load_global_config()?;
    let project = config::load_project_config(&repo.path)?;
    let resolved = config::resolve_config(None, project.as_ref(), &global);
    let mut refresh = RefreshRuntime::new(cwd.clone(), repo.path.clone());
    refresh.launch()?;
    let mut state = AppState::new(Vec::new());
    state.repository = repo.name.clone();
    state.hooks = HookNames {
        create: report::create_hooks(resolved.hooks.as_ref()),
        sync: report::sync_hooks(resolved.hooks.as_ref()),
        remove: report::remove_hooks(resolved.hooks.as_ref()),
    };
    apply_refresh_publications(&mut refresh, &mut state);
    let selected_theme =
        theme::resolve(&resolved.ui.theme, std::env::var_os("NO_COLOR").as_deref());
    let mut operation =
        OperationRuntime::new(ThreadOperationLauncher, SystemRuntimeClock::default());

    super::install_panic_hook();
    let mut terminal = ratatui::init();
    let result = (|| -> Result<TuiExit> {
        'event_loop: loop {
            for effect in operation.tick() {
                match effect {
                    OperationRuntimeEffect::Succeeded(OperationOutcome::Create(outcome)) => {
                        operation.dismiss();
                        finish_create_success(&mut state, &mut refresh, outcome, Instant::now());
                    }
                    OperationRuntimeEffect::Succeeded(OperationOutcome::Sync(outcome)) => {
                        operation.dismiss();
                        finish_sync_success(&mut state, &mut refresh, outcome, Instant::now());
                    }
                    OperationRuntimeEffect::Succeeded(OperationOutcome::Remove(outcome)) => {
                        operation.dismiss();
                        finish_remove_success(&mut state, &mut refresh, outcome, Instant::now());
                    }
                    OperationRuntimeEffect::Failed { stage, message } => {
                        tracing::warn!(?stage, %message, "cockpit operation failed");
                    }
                }
            }
            state.operation_modal = operation.modal().cloned();
            let _ = app::reduce(&mut state, Event::NotificationTick(Instant::now()));
            let (width, height) = crossterm::terminal::size()?;
            let _ = app::reduce(&mut state, Event::ViewportChanged { width, height });
            terminal.draw(|frame| {
                view::render(&state, frame, frame.area(), &selected_theme);
            })?;

            let _ = app::reduce(&mut state, Event::RefreshTick);
            refresh.tick();
            apply_refresh_publications(&mut refresh, &mut state);

            if !event::poll(std::time::Duration::from_millis(50))? {
                continue;
            }
            let TerminalEvent::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break 'event_loop Ok(TuiExit::Quit);
            }
            let Some(key) = visible_surface_key(&state, key) else {
                continue;
            };
            if state.viewport.is_tiny() && key.code == KeyCode::Char('q') {
                break 'event_loop Ok(TuiExit::Quit);
            }

            if state.operation_modal.is_some() {
                let Some(modal_key) = translate_modal_key(key) else {
                    continue;
                };
                let effect = operation_modal_effect(
                    &mut operation,
                    modal_key,
                    ratatui::layout::Rect::new(0, 0, width, height),
                );
                apply_operation_modal_effect(
                    effect,
                    &mut state,
                    &mut operation,
                    &mut refresh,
                    OperationFormContext {
                        cwd: &cwd,
                        worktree_root: &resolved.worktrees.root,
                        configured_base: resolved.git.default_base.as_deref(),
                        hooks: resolved.hooks.clone(),
                    },
                );
                continue;
            }

            if state.create_dialog.is_some() {
                let Some(create_key) = translate_create_key(key) else {
                    continue;
                };
                let effect = handle_create_input(
                    &mut state,
                    create_key,
                    &cwd,
                    &resolved.worktrees.root,
                    resolved.git.default_base.as_deref(),
                    resolved.hooks.clone(),
                )?;
                apply_create_input_effect(effect, &mut state, &mut refresh, &mut operation);
                continue;
            }

            if state.sync_dialog.is_some() {
                let Some(sync_key) = translate_sync_key(key) else {
                    continue;
                };
                let effect = handle_sync_input(
                    &mut state,
                    sync_key,
                    &cwd,
                    resolved.git.default_base.as_deref(),
                    resolved.hooks.clone(),
                )?;
                apply_sync_input_effect(effect, &mut state, &mut refresh, &mut operation);
                continue;
            }

            if state.remove_dialog.is_some() {
                let Some(remove_key) = translate_remove_key(key) else {
                    continue;
                };
                if let Some(RemoveInputEffect::Start(request)) = handle_remove_input(
                    &mut state,
                    remove_key,
                    &cwd,
                    resolved.git.default_base.as_deref(),
                    resolved.hooks.clone(),
                    Instant::now(),
                )? {
                    operation.start(*request);
                    state.operation_modal = operation.modal().cloned();
                }
                continue;
            }

            let Some(key) = translate_key(key) else {
                continue;
            };
            for effect in app::reduce(&mut state, Event::Input(key)) {
                match effect {
                    Effect::Switch(id) => {
                        break 'event_loop Ok(TuiExit::Switch(id.as_path().to_path_buf()));
                    }
                    Effect::Open(id) => open_in_editor(
                        &mut state,
                        &mut terminal,
                        &mut refresh,
                        resolved.editor_command.as_deref(),
                        id.as_path(),
                    ),
                    Effect::OpenCreate => {
                        if let Err(error) = open_create_dialog(
                            &mut state,
                            &repo.name,
                            &resolved.worktrees.root,
                            resolved.git.default_base.as_deref(),
                        ) {
                            notify(&mut state, NoticeKind::Warning, error.to_string());
                        }
                    }
                    Effect::OpenSync(id) => {
                        if let Err(error) =
                            open_sync_dialog(&mut state, &id, resolved.git.default_base.as_deref())
                        {
                            notify(&mut state, NoticeKind::Warning, error.to_string());
                        }
                    }
                    Effect::OpenRemove(id) => {
                        if let Err(error) = open_remove_dialog(
                            &mut state,
                            &cwd,
                            &id,
                            resolved.git.default_base.as_deref(),
                        ) {
                            notify(&mut state, NoticeKind::Warning, error.to_string());
                        }
                    }
                    Effect::Refresh => {
                        refresh.manual()?;
                        apply_refresh_publications(&mut refresh, &mut state);
                    }
                    Effect::Quit => break 'event_loop Ok(TuiExit::Quit),
                    Effect::Unavailable { reason, .. } => {
                        notify(&mut state, NoticeKind::Warning, reason);
                    }
                }
            }
        }
    })();

    ratatui::restore();
    super::restore_panic_hook();
    result
}

fn apply_refresh_publications(refresh: &mut RefreshRuntime, state: &mut AppState) {
    for publication in refresh.drain_publications() {
        apply_refresh_publication(state, publication);
    }
}

fn apply_refresh_publication(state: &mut AppState, publication: RefreshPublication) {
    let refs = publication.refs.clone();
    let origin_refresh = if publication.updating_refs {
        OriginRefresh::Loading
    } else if publication.warning.is_some() {
        OriginRefresh::Failed
    } else {
        OriginRefresh::Idle
    };
    let _ = app::reduce(state, Event::RefreshPublished(publication));
    let worktrees = existing_worktrees(state);
    if let Some(dialog) = state.create_dialog.as_mut() {
        if let Some(refs) = refs.clone() {
            dialog.update_refs(refs);
        }
        dialog.update_worktrees(worktrees);
        dialog.set_origin_refresh(origin_refresh);
    }
    if let Some(dialog) = state.sync_dialog.as_mut() {
        if let Some(refs) = refs {
            dialog.update_refs(refs);
        }
        dialog.set_origin_refresh(origin_refresh);
    }
}

fn translate_key(key: KeyEvent) -> Option<Key> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => return Some(Key::Down),
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => return Some(Key::Up),
        _ => {}
    }
    if let Some(edit) = translate_line_edit(key) {
        return Some(Key::Edit(edit));
    }

    match key.code {
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Escape),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Key::Char(character))
        }
        _ => None,
    }
}

fn translate_line_edit(key: KeyEvent) -> Option<LineEdit> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('a'), KeyModifiers::CONTROL) => Some(LineEdit::Start),
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => Some(LineEdit::End),
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => Some(LineEdit::PreviousCharacter),
        (KeyCode::Char('f'), KeyModifiers::CONTROL) => Some(LineEdit::NextCharacter),
        (KeyCode::Char('h'), KeyModifiers::CONTROL) => Some(LineEdit::DeletePreviousCharacter),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(LineEdit::DeleteNextCharacter),
        (KeyCode::Left, KeyModifiers::NONE) => Some(LineEdit::PreviousCharacter),
        (KeyCode::Right, KeyModifiers::NONE) => Some(LineEdit::NextCharacter),
        (KeyCode::Home, KeyModifiers::NONE) => Some(LineEdit::Start),
        (KeyCode::End, KeyModifiers::NONE) => Some(LineEdit::End),
        (KeyCode::Backspace, KeyModifiers::NONE) => Some(LineEdit::DeletePreviousCharacter),
        (KeyCode::Delete, KeyModifiers::NONE) => Some(LineEdit::DeleteNextCharacter),
        _ => None,
    }
}

fn visible_surface_key(state: &AppState, key: KeyEvent) -> Option<KeyEvent> {
    (!state.viewport.is_tiny() || key.code == KeyCode::Char('q')).then_some(key)
}

fn translate_create_key(key: KeyEvent) -> Option<CreateKey> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => return Some(CreateKey::Down),
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => return Some(CreateKey::Up),
        _ => {}
    }
    if let Some(edit) = translate_line_edit(key) {
        return Some(CreateKey::Edit(edit));
    }

    match key.code {
        KeyCode::Enter => Some(CreateKey::Enter),
        KeyCode::Esc => Some(CreateKey::Escape),
        KeyCode::Up => Some(CreateKey::Up),
        KeyCode::Down => Some(CreateKey::Down),
        KeyCode::Tab => Some(CreateKey::Tab),
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(CreateKey::Character(character))
        }
        _ => None,
    }
}

fn translate_sync_key(key: KeyEvent) -> Option<SyncKey> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => return Some(SyncKey::Down),
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => return Some(SyncKey::Up),
        (KeyCode::Left, KeyModifiers::NONE) => return Some(SyncKey::Left),
        (KeyCode::Right, KeyModifiers::NONE) => return Some(SyncKey::Right),
        _ => {}
    }
    if let Some(edit) = translate_line_edit(key) {
        return Some(SyncKey::Edit(edit));
    }

    match key.code {
        KeyCode::Enter => Some(SyncKey::Enter),
        KeyCode::Esc => Some(SyncKey::Escape),
        KeyCode::Up => Some(SyncKey::Up),
        KeyCode::Down => Some(SyncKey::Down),
        KeyCode::Tab => Some(SyncKey::Tab),
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(SyncKey::Character(character))
        }
        _ => None,
    }
}

/// `y` is the only key that removes. Enter is deliberately not mapped: it is
/// the list's "switch" key, and a habit must never delete a worktree.
fn translate_remove_key(key: KeyEvent) -> Option<RemoveKey> {
    match key.code {
        KeyCode::Char('y' | 'Y') => Some(RemoveKey::Confirm),
        KeyCode::Esc => Some(RemoveKey::Escape),
        KeyCode::Char(' ') => Some(RemoveKey::Space),
        _ => None,
    }
}

fn translate_modal_key(key: KeyEvent) -> Option<ModalKey> {
    match key.code {
        KeyCode::Enter => Some(ModalKey::Enter),
        KeyCode::Esc => Some(ModalKey::Escape),
        KeyCode::Up => Some(ModalKey::Up),
        KeyCode::Down => Some(ModalKey::Down),
        KeyCode::PageUp => Some(ModalKey::PageUp),
        KeyCode::PageDown => Some(ModalKey::PageDown),
        KeyCode::Home => Some(ModalKey::Home),
        KeyCode::End => Some(ModalKey::End),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::create_flow::CreateFocus;
    use std::{cell::RefCell, rc::Rc};

    use tempfile::TempDir;

    use crate::{
        config::{HookDef, HooksConfig},
        create_plan::{CreateAction, HookPolicy},
        operation::OperationRequest,
        ref_catalog::RefSnapshot,
        tui::{
            app::WorktreeIdentity,
            create_flow::{CreateKey, CreateSubmission},
            sync_flow::{SyncKey, SyncMode},
        },
    };

    #[derive(Clone)]
    struct EventLog(Rc<RefCell<Vec<String>>>);

    impl EventLog {
        fn new() -> Self {
            Self(Rc::new(RefCell::new(Vec::new())))
        }

        fn push(&self, event: impl Into<String>) {
            self.0.borrow_mut().push(event.into());
        }

        fn events(&self) -> Vec<String> {
            self.0.borrow().clone()
        }
    }

    struct FakeTerminal {
        log: EventLog,
    }

    impl TerminalDriver for FakeTerminal {
        fn suspend(&mut self) -> Result<()> {
            self.log.push("suspend");
            Ok(())
        }

        fn resume(&mut self) -> Result<()> {
            self.log.push("resume");
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    enum FakeEditorResult {
        Status(EditorStatus),
        SpawnFailure,
    }

    struct FakeEditor {
        log: EventLog,
        result: FakeEditorResult,
    }

    impl EditorLauncher for FakeEditor {
        fn launch(&mut self, command: &EditorCommand, path: &Path) -> Result<EditorStatus> {
            self.log.push(format!(
                "launch:{}:{:?}:{}",
                command.program().to_string_lossy(),
                command.args(),
                path.display()
            ));
            match self.result {
                FakeEditorResult::Status(status) => Ok(status),
                FakeEditorResult::SpawnFailure => anyhow::bail!("spawn failed"),
            }
        }
    }

    struct FakeRefresh {
        log: EventLog,
    }

    impl RefreshDriver for FakeRefresh {
        fn refresh(&mut self, cause: RefreshCause) -> Result<()> {
            self.log.push(format!("refresh:{cause:?}"));
            Ok(())
        }
    }

    fn editor_effect(result: FakeEditorResult) -> (Result<()>, Vec<String>) {
        let log = EventLog::new();
        let mut terminal = FakeTerminal { log: log.clone() };
        let mut launcher = FakeEditor {
            log: log.clone(),
            result,
        };
        let mut refresh = FakeRefresh { log: log.clone() };
        let command =
            EditorCommand::from_environment(Some("editor --configured 'two words'")).unwrap();
        let result = open_editor(
            &mut terminal,
            &mut launcher,
            &mut refresh,
            &command,
            Path::new("/tmp/work tree"),
        );
        (result, log.events())
    }

    #[test]
    fn terminal_keys_translate_to_the_pure_reducer_vocabulary() {
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )),
            Some(Key::Enter)
        );
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::Char('?'),
                crossterm::event::KeyModifiers::NONE,
            )),
            Some(Key::Char('?'))
        );
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::Backspace,
                crossterm::event::KeyModifiers::NONE,
            )),
            Some(Key::Edit(LineEdit::DeletePreviousCharacter))
        );
        assert_eq!(
            translate_key(KeyEvent::new(
                KeyCode::PageDown,
                crossterm::event::KeyModifiers::NONE,
            )),
            None
        );
    }

    #[test]
    fn terminal_keys_preserve_standard_single_line_editing_commands() {
        use crossterm::event::KeyModifiers;

        let cases = [
            (KeyCode::Char('a'), KeyModifiers::CONTROL, LineEdit::Start),
            (KeyCode::Char('e'), KeyModifiers::CONTROL, LineEdit::End),
            (
                KeyCode::Char('b'),
                KeyModifiers::CONTROL,
                LineEdit::PreviousCharacter,
            ),
            (
                KeyCode::Char('f'),
                KeyModifiers::CONTROL,
                LineEdit::NextCharacter,
            ),
            (
                KeyCode::Char('h'),
                KeyModifiers::CONTROL,
                LineEdit::DeletePreviousCharacter,
            ),
            (
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
                LineEdit::DeleteNextCharacter,
            ),
            (
                KeyCode::Left,
                KeyModifiers::NONE,
                LineEdit::PreviousCharacter,
            ),
            (KeyCode::Right, KeyModifiers::NONE, LineEdit::NextCharacter),
            (KeyCode::Home, KeyModifiers::NONE, LineEdit::Start),
            (KeyCode::End, KeyModifiers::NONE, LineEdit::End),
            (
                KeyCode::Backspace,
                KeyModifiers::NONE,
                LineEdit::DeletePreviousCharacter,
            ),
            (
                KeyCode::Delete,
                KeyModifiers::NONE,
                LineEdit::DeleteNextCharacter,
            ),
        ];

        for (code, modifiers, expected) in cases {
            assert_eq!(
                translate_key(KeyEvent::new(code, modifiers)),
                Some(Key::Edit(expected))
            );
        }
    }

    #[test]
    fn unrecognized_control_characters_do_not_become_text_or_cockpit_actions() {
        for character in ['k', 'o', 'q', 's', 'u', 'x'] {
            let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL);
            assert_eq!(
                translate_key(key),
                None,
                "Control-{character} must not lose its modifier",
            );
            assert_eq!(translate_create_key(key), None);
            assert_eq!(translate_sync_key(key), None);
        }
    }

    #[test]
    fn control_navigation_maps_to_candidates_on_every_editable_surface() {
        let next = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        let previous = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);

        assert_eq!(translate_key(next), Some(Key::Down));
        assert_eq!(translate_key(previous), Some(Key::Up));
        assert_eq!(translate_create_key(next), Some(CreateKey::Down));
        assert_eq!(translate_create_key(previous), Some(CreateKey::Up));
        assert_eq!(translate_sync_key(next), Some(SyncKey::Down));
        assert_eq!(translate_sync_key(previous), Some(SyncKey::Up));
    }

    #[test]
    fn terminal_keys_translate_to_create_form_vocabulary() {
        let modifiers = crossterm::event::KeyModifiers::NONE;
        assert_eq!(
            translate_create_key(KeyEvent::new(KeyCode::Tab, modifiers)),
            Some(CreateKey::Tab)
        );
        assert_eq!(
            translate_create_key(KeyEvent::new(KeyCode::Backspace, modifiers)),
            Some(CreateKey::Edit(LineEdit::DeletePreviousCharacter))
        );
        assert_eq!(
            translate_create_key(KeyEvent::new(KeyCode::Char('x'), modifiers)),
            Some(CreateKey::Character('x'))
        );
    }

    #[test]
    fn terminal_keys_translate_to_sync_form_vocabulary() {
        let modifiers = crossterm::event::KeyModifiers::NONE;
        assert_eq!(
            translate_sync_key(KeyEvent::new(KeyCode::Left, modifiers)),
            Some(SyncKey::Left)
        );
        assert_eq!(
            translate_sync_key(KeyEvent::new(KeyCode::Right, modifiers)),
            Some(SyncKey::Right)
        );
        assert_eq!(
            translate_sync_key(KeyEvent::new(KeyCode::Tab, modifiers)),
            Some(SyncKey::Tab)
        );
        assert_eq!(
            translate_sync_key(KeyEvent::new(KeyCode::Char('x'), modifiers)),
            Some(SyncKey::Character('x'))
        );
    }

    #[test]
    fn open_create_request_builds_a_live_form_from_cockpit_state_and_config() {
        let checked_out = WorktreeIdentity {
            id: WorktreeId::new("/worktrees/trench/release"),
            worktree: "release".to_string(),
            branch: Some("release".to_string()),
            path: PathBuf::from("/worktrees/trench/release"),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![checked_out]);
        state.refs = Some(RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/release"],
            Some("origin/main"),
            Some("main"),
            true,
        ));

        open_create_dialog(
            &mut state,
            "trench",
            Path::new("/worktrees"),
            Some("release"),
        )
        .unwrap();

        let dialog = state.create_dialog.as_mut().expect("create form opened");
        dialog.set_branch("feature/auth");
        assert_eq!(dialog.base(), Some("release"));
        dialog.set_branch("release");
        assert!(matches!(
            dialog.handle_key(CreateKey::Enter),
            Some(crate::tui::create_flow::CreateEffect::Navigate(_))
        ));
    }

    #[test]
    fn create_submission_builds_a_run_hooks_operation_from_live_repo_state() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let hooks = HooksConfig {
            pre_create: Some(HookDef {
                shell: Some("true".to_string()),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let submission = CreateSubmission {
            branch: "feature/auth".to_string(),
            from: None,
        };

        let dispatch = build_create_dispatch(
            &submission,
            repository.path(),
            &root,
            Some("main"),
            Some(hooks.clone()),
        )
        .unwrap();

        let CreateDispatch::Run(request) = dispatch else {
            panic!("new branch should start a create operation")
        };
        let OperationRequest::Create(request) = *request else {
            panic!("new branch should start a create operation")
        };
        assert_eq!(
            request.plan.action,
            CreateAction::NewBranch("main".to_string())
        );
        assert_eq!(request.plan.hook_policy, HookPolicy::Run);
        assert_eq!(
            request.repo_path,
            crate::git::discover_repo(repository.path()).unwrap().path
        );
        assert_eq!(request.worktree_root, root);
        assert_eq!(request.hooks, Some(hooks));
    }

    #[test]
    fn sync_submission_replans_the_exact_path_with_explicit_strategy_and_hooks() {
        let repository = init_repo();
        let target = repository.path().canonicalize().unwrap();
        let hooks = HooksConfig {
            pre_sync: Some(HookDef {
                shell: Some("true".to_string()),
                ..HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let submission = crate::tui::sync_flow::SyncSubmission {
            target: WorktreeId::new(target.clone()),
            base: "main".to_string(),
            strategy: crate::cli::commands::sync::stateless::SyncStrategy::Merge,
        };

        let request = build_sync_request(
            &submission,
            repository.path(),
            Some("main"),
            Some(hooks.clone()),
        )
        .unwrap();

        let OperationRequest::Sync(request) = request else {
            panic!("sync submission should start a sync operation")
        };
        assert_eq!(request.plan.path, target);
        assert_eq!(
            request.plan.strategy,
            crate::cli::commands::sync::stateless::SyncStrategy::Merge
        );
        assert_eq!(
            request.plan.hook_policy,
            crate::cli::commands::sync::stateless::HookPolicy::Run
        );
        assert_eq!(request.hooks, Some(hooks));
    }

    #[test]
    fn sync_submission_preserves_a_remote_only_base() {
        let repository = init_repo();
        let git = git2::Repository::open(repository.path()).unwrap();
        let head = git.head().unwrap().target().unwrap();
        git.reference(
            "refs/remotes/origin/release",
            head,
            true,
            "test remote base",
        )
        .unwrap();
        drop(git);
        let target = repository.path().canonicalize().unwrap();

        let request = build_sync_request(
            &SyncSubmission {
                target: WorktreeId::new(target),
                base: "origin/release".to_string(),
                strategy: crate::cli::commands::sync::stateless::SyncStrategy::Rebase,
            },
            repository.path(),
            Some("main"),
            None,
        )
        .unwrap();

        let OperationRequest::Sync(request) = request else {
            panic!("sync submission should start a sync operation")
        };
        assert_eq!(request.plan.base, "origin/release");
    }

    #[test]
    fn sync_dialog_rechecks_selected_worktree_before_opening() {
        let repository = init_repo();
        let path = repository.path().canonicalize().unwrap();
        let identity = WorktreeIdentity {
            id: WorktreeId::new(path.clone()),
            worktree: "main".into(),
            branch: Some("main".into()),
            path: path.clone(),
            head: None,
            is_main: true,
            is_current: true,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![identity.clone()]);
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        std::fs::write(path.join("untracked.txt"), "dirty").unwrap();
        let error = open_sync_dialog(&mut state, &identity.id, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has uncommitted changes. Commit or stash them, then sync."),
            "{error}"
        );
        assert!(state.sync_dialog.is_none());
    }

    #[test]
    fn sync_dialog_opens_the_exact_visible_target_and_routes_picker_and_submit() {
        let repository = init_repo();
        let target = repository.path().canonicalize().unwrap();
        let identity = WorktreeIdentity {
            id: WorktreeId::new(target.clone()),
            worktree: "main".to_string(),
            branch: Some("main".to_string()),
            path: target,
            head: None,
            is_main: true,
            is_current: true,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![identity.clone()]);
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        ));

        open_sync_dialog(&mut state, &identity.id, Some("main")).unwrap();
        let dialog = state.sync_dialog.as_ref().expect("sync form opened");
        assert_eq!(dialog.target(), &identity.id);
        assert_eq!(dialog.base(), Some("main"));

        assert!(matches!(
            handle_sync_input(
                &mut state,
                SyncKey::Tab,
                repository.path(),
                Some("main"),
                None,
            )
            .unwrap(),
            Some(SyncInputEffect::RefreshOrigin)
        ));
        assert_eq!(
            state.sync_dialog.as_ref().unwrap().mode(),
            SyncMode::BasePicker
        );

        apply_refresh_publication(
            &mut state,
            RefreshPublication {
                identities: vec![identity],
                refs: Some(RefSnapshot::from_parts(
                    ["main"],
                    ["origin/main", "origin/release"],
                    Some("origin/main"),
                    Some("main"),
                    true,
                )),
                updating_refs: false,
                warning: None,
            },
        );
        assert!(state
            .sync_dialog
            .as_ref()
            .unwrap()
            .base_candidates()
            .iter()
            .any(|candidate| candidate.name == "origin/release"));

        handle_sync_input(
            &mut state,
            SyncKey::Escape,
            repository.path(),
            Some("main"),
            None,
        )
        .unwrap();
        let effect = handle_sync_input(
            &mut state,
            SyncKey::Enter,
            repository.path(),
            Some("main"),
            None,
        )
        .unwrap();
        assert!(matches!(
            effect,
            Some(SyncInputEffect::Start(request))
                if matches!(*request, OperationRequest::Sync(_))
        ));
    }

    #[test]
    fn create_dialog_opens_on_the_branch_and_enter_starts_only_a_highlighted_row() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        let press = |state: &mut AppState, key| {
            handle_create_input(state, key, repository.path(), &root, Some("main"), None).unwrap()
        };
        assert_eq!(
            state.create_dialog.as_ref().unwrap().focus(),
            CreateFocus::Branch
        );
        assert!(press(&mut state, CreateKey::Enter).is_none());
        assert!(state.create_dialog.is_some(), "nothing is highlighted yet");

        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_branch("feature/auth");
        assert!(press(&mut state, CreateKey::Tab).is_none());
        assert_eq!(
            state.create_dialog.as_ref().unwrap().focus(),
            CreateFocus::Base
        );
        assert!(press(&mut state, CreateKey::Escape).is_none());
        assert_eq!(
            state.create_dialog.as_ref().unwrap().focus(),
            CreateFocus::Branch
        );

        let start = press(&mut state, CreateKey::Enter);
        assert!(matches!(
            start,
            Some(CreateInputEffect::Start(request))
                if matches!(*request, OperationRequest::Create(_))
        ));
    }

    #[test]
    fn ref_picker_publications_replace_refs_and_retain_choices_on_fetch_failure() {
        let mut state = AppState::new(Vec::new());
        let initial = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        state.refs = Some(initial.clone());
        open_create_dialog(&mut state, "trench", Path::new("/worktrees"), None).unwrap();
        let dialog = state.create_dialog.as_mut().unwrap();
        dialog.set_branch("feature/new");
        dialog.handle_key(CreateKey::Tab);
        assert_eq!(dialog.focus(), CreateFocus::Base);

        let updated = RefSnapshot::from_parts(
            ["main"],
            ["origin/main", "origin/release"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        apply_refresh_publication(
            &mut state,
            RefreshPublication {
                identities: Vec::new(),
                refs: Some(updated),
                updating_refs: false,
                warning: None,
            },
        );
        let dialog = state.create_dialog.as_ref().unwrap();
        assert!(dialog
            .base_candidates()
            .iter()
            .any(|candidate| candidate.name == "origin/release"));
        assert!(!dialog.origin_spinner_visible());

        apply_refresh_publication(
            &mut state,
            RefreshPublication {
                identities: Vec::new(),
                refs: None,
                updating_refs: false,
                warning: Some("Could not update origin; showing local refs".to_string()),
            },
        );
        let dialog = state.create_dialog.as_ref().unwrap();
        assert!(dialog
            .base_candidates()
            .iter()
            .any(|candidate| candidate.name == "origin/release"));
        assert!(dialog.warning().is_some());
    }

    #[test]
    fn failed_enter_returns_to_the_form_and_reports_live_revalidation_changes() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_branch("feature/auth");
        let planned = build_create_dispatch(
            &CreateSubmission {
                branch: "feature/auth".to_string(),
                from: None,
            },
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();
        let CreateDispatch::Run(planned) = planned else {
            panic!("initial plan should create")
        };
        let OperationRequest::Create(planned) = *planned else {
            panic!("initial plan should create")
        };
        std::fs::create_dir_all(planned.plan.path.parent().unwrap()).unwrap();
        let git_repo = git2::Repository::open(repository.path()).unwrap();
        let commit = git_repo.head().unwrap().peel_to_commit().unwrap();
        git_repo.branch("feature/auth", &commit, false).unwrap();
        let reference = git_repo.find_reference("refs/heads/feature/auth").unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&reference));
        git_repo
            .worktree("feature-auth", &planned.plan.path, Some(&options))
            .unwrap();

        return_to_create_form(&mut state, repository.path(), &root, Some("main"), None);

        let dialog = state.create_dialog.as_ref().unwrap();
        assert_eq!(dialog.focus(), CreateFocus::Branch);
        assert_eq!(dialog.branch_input().value(), "feature/auth");
        assert!(dialog.validation_error().is_some());
    }

    #[test]
    fn a_name_the_dialog_already_knows_is_invalid_never_reaches_planning() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_branch("bad~branch");

        assert!(handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap()
        .is_none());
        let dialog = state.create_dialog.as_ref().unwrap();
        assert!(!dialog.can_submit());
        assert!(dialog.validation_error().is_none());
    }

    #[test]
    fn a_planning_failure_stays_in_the_form_with_its_message() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_branch("feature/taken");
        // Only live planning can see a directory that is not a worktree.
        let occupied = root.join(&repo_info.name).join("feature-taken");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("leftover"), "x").unwrap();

        assert!(handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap()
        .is_none());
        assert!(state
            .create_dialog
            .as_ref()
            .unwrap()
            .validation_error()
            .is_some());
    }

    #[test]
    fn enter_does_not_create_when_the_summary_said_the_branch_already_has_a_worktree() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let old_path = root.join(&repo_info.name).join("release-old");
        add_worktree(repository.path(), "release", "release-old", &old_path);
        let old = WorktreeIdentity {
            id: WorktreeId::new(old_path.clone()),
            worktree: "release-old".to_string(),
            branch: Some("release".to_string()),
            path: old_path.clone(),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![old]);
        state.refs = Some(RefSnapshot::from_parts(
            ["main", "release"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state.create_dialog.as_mut().unwrap().set_branch("release");
        prune_worktree(repository.path(), "release-old", &old_path);

        let effect = handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();

        // The dialog said "already has a worktree, nothing will be created".
        // Live Git now says creating is possible. Enter must not silently do
        // the other thing: it reports the change and asks again.
        assert!(
            matches!(effect, Some(CreateInputEffect::Refresh)),
            "{effect:?}"
        );
        let dialog = state.create_dialog.as_ref().unwrap();
        assert_eq!(dialog.validation_error(), Some(STALE_CREATE_SUMMARY));
        assert!(!root.join(&repo_info.name).join("release").exists());
    }

    #[test]
    fn enter_on_a_local_branch_named_like_a_remote_creates_exactly_that_branch() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let git_repo = git2::Repository::open(repository.path()).unwrap();
        let commit = git_repo.head().unwrap().peel_to_commit().unwrap();
        git_repo.branch("x", &commit, false).unwrap();
        git_repo.branch("origin/x", &commit, false).unwrap();
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main", "x", "origin/x"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state.create_dialog.as_mut().unwrap().set_branch("origin/x");
        let row = state
            .create_dialog
            .as_ref()
            .unwrap()
            .highlighted_row()
            .unwrap();
        assert_eq!(row.branch, "origin/x");

        let effect = handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();

        let Some(CreateInputEffect::Start(request)) = effect else {
            panic!("the highlighted branch should be created: {effect:?}")
        };
        let OperationRequest::Create(request) = *request else {
            panic!("expected a create request")
        };
        assert_eq!(request.plan.branch, "origin/x");
        assert_eq!(
            request.plan.path,
            root.join(&repo_info.name).join("origin-x")
        );
    }

    #[test]
    fn the_enter_gate_compares_the_branch_the_directory_and_the_base() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let git_repo = git2::Repository::open(repository.path()).unwrap();
        let commit = git_repo.head().unwrap().peel_to_commit().unwrap();
        git_repo.branch("release", &commit, false).unwrap();
        git_repo.branch("other", &commit, false).unwrap();
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main", "release", "other"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        let dialog = state.create_dialog.as_mut().unwrap();
        let plan_for = |branch: &str, from: Option<&str>, root: &Path| {
            build_create_dispatch(
                &CreateSubmission {
                    branch: branch.to_string(),
                    from: from.map(ToOwned::to_owned),
                },
                repository.path(),
                root,
                Some("main"),
                None,
            )
            .unwrap()
        };

        dialog.set_branch("release");
        let row = dialog.highlighted_row().unwrap();
        assert!(plan_for("release", None, &root).matches(dialog, &row));
        assert!(
            !plan_for("other", None, &root).matches(dialog, &row),
            "same action on another branch"
        );
        assert!(
            !plan_for("release", None, &repository.path().join("elsewhere")).matches(dialog, &row),
            "same branch in another directory"
        );

        dialog.set_branch("feature/new");
        let row = dialog.highlighted_row().unwrap();
        assert!(plan_for("feature/new", Some("main"), &root).matches(dialog, &row));
        assert!(
            !plan_for("feature/new", Some("release"), &root).matches(dialog, &row),
            "a new branch from a base the dialog did not show"
        );
    }

    #[test]
    fn enter_does_not_check_out_an_existing_branch_when_the_summary_said_new_branch() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let mut state = AppState::new(Vec::new());
        state.refs = Some(RefSnapshot::from_parts(
            ["main"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_branch("feature/raced");
        // Someone creates the branch before Enter; the dialog's refs are stale.
        let git_repo = git2::Repository::open(repository.path()).unwrap();
        let commit = git_repo.head().unwrap().peel_to_commit().unwrap();
        git_repo.branch("feature/raced", &commit, false).unwrap();

        let effect = handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();

        assert!(
            matches!(effect, Some(CreateInputEffect::Refresh)),
            "{effect:?}"
        );
        assert_eq!(
            state.create_dialog.as_ref().unwrap().validation_error(),
            Some(STALE_CREATE_SUMMARY)
        );
    }

    #[test]
    fn checked_out_enter_uses_the_live_path_when_identity_changes_while_form_is_open() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let repo_info = crate::git::discover_repo(repository.path()).unwrap();
        let old_path = root.join(&repo_info.name).join("release-old");
        let new_path = root.join(&repo_info.name).join("release-new");
        add_worktree(repository.path(), "release", "release-old", &old_path);
        let old = WorktreeIdentity {
            id: WorktreeId::new(old_path.clone()),
            worktree: "release-old".to_string(),
            branch: Some("release".to_string()),
            path: old_path.clone(),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![old]);
        state.refs = Some(RefSnapshot::from_parts(
            ["main", "release"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_create_dialog(&mut state, &repo_info.name, &root, Some("main")).unwrap();
        state.create_dialog.as_mut().unwrap().set_branch("release");
        prune_worktree(repository.path(), "release-old", &old_path);
        add_worktree(repository.path(), "release", "release-new", &new_path);

        let effect = handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();

        assert!(matches!(
            effect,
            Some(CreateInputEffect::Navigate(id))
                if id == WorktreeId::new(new_path.canonicalize().unwrap())
        ));
    }

    #[test]
    fn applied_create_success_survives_post_operation_refresh_failure() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let dispatch = build_create_dispatch(
            &CreateSubmission {
                branch: "feature/auth".to_string(),
                from: None,
            },
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();
        let CreateDispatch::Run(request) = dispatch else {
            panic!("expected create request")
        };
        let OperationRequest::Create(request) = *request else {
            panic!("expected create request")
        };
        let mut state = AppState::new(Vec::new());
        let mut refresh = FailingPostOperationRefresh::default();

        finish_create_success(
            &mut state,
            &mut refresh,
            crate::operation::CreateOutcome {
                plan: request.plan,
                mutation_state: crate::operation::MutationState::Applied,
            },
            Instant::now(),
        );

        assert!(refresh.requested);
        assert!(state.operation_modal.is_none());
        assert!(state
            .notification
            .as_ref()
            .is_some_and(|notice| notice.text.contains("Created feature/auth")));
        assert!(state
            .notification
            .as_ref()
            .is_some_and(|notice| notice.text.contains("refresh failed")));
        assert!(state
            .refresh
            .warning
            .as_ref()
            .is_some_and(|warning| warning.contains("press r to refresh")));
    }

    #[test]
    fn sync_success_refreshes_comparisons_keeps_filtered_selection_and_notifies() {
        let target = WorktreeIdentity {
            id: WorktreeId::new("/worktrees/feature-auth"),
            worktree: "feature-auth".to_string(),
            branch: Some("feature/auth".to_string()),
            path: PathBuf::from("/worktrees/feature-auth"),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let refs = RefSnapshot::from_parts(
            ["main", "feature/auth"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        );
        let mut state = AppState::new(vec![target.clone()]);
        state.refs = Some(refs.clone());
        let _ = app::reduce(&mut state, Event::Input(Key::Char('/')));
        for character in "auth".chars() {
            let _ = app::reduce(&mut state, Event::Input(Key::Char(character)));
        }
        state.sync_dialog = Some(SyncDialog::new(&target, refs.clone(), Some("main")));
        state.operation_modal = Some(crate::tui::operation_modal::OperationModal::new(
            crate::operation::OperationKind::Sync,
        ));
        let publication = RefreshPublication {
            identities: vec![target.clone()],
            refs: Some(refs),
            updating_refs: false,
            warning: None,
        };
        let mut refresh = SuccessfulPostOperationRefresh::new(publication);
        let shown_at = Instant::now();

        finish_sync_success(
            &mut state,
            &mut refresh,
            crate::cli::commands::sync::stateless::SyncOutcome {
                target: target.worktree.clone(),
                branch: "feature/auth".to_string(),
                path: target.path.clone(),
                base: "main".to_string(),
                strategy: crate::cli::commands::sync::stateless::SyncStrategy::Rebase,
                before: crate::cli::commands::sync::stateless::AheadBehind {
                    ahead: 1,
                    behind: 2,
                },
                after: crate::cli::commands::sync::stateless::AheadBehind {
                    ahead: 0,
                    behind: 0,
                },
                mutation_state: crate::cli::commands::sync::stateless::MutationState::Applied,
                elapsed: std::time::Duration::from_millis(12),
            },
            shown_at,
        );

        assert!(refresh.requested);
        assert_eq!(
            state.selected_visible().map(|row| &row.id),
            Some(&target.id)
        );
        assert_eq!(
            state.search.as_ref().map(|query| query.as_str()),
            Some("auth")
        );
        assert!(state.sync_dialog.is_none());
        assert!(state.operation_modal.is_none());
        assert!(state.notification.as_ref().is_some_and(|notice| {
            notice.text
                == "Rebased feature/auth onto main. Was 1 ahead, 2 behind; now 0 ahead, 0 behind."
        }));
    }

    #[test]
    fn failed_sync_enter_returns_to_form_and_revalidates_live_dirty_state() {
        let repository = init_repo();
        let worktree_path = repository.path().join("worktrees/feature-auth");
        add_worktree(
            repository.path(),
            "feature/auth",
            "feature-auth",
            &worktree_path,
        );
        let target_path = worktree_path.canonicalize().unwrap();
        let target = WorktreeIdentity {
            id: WorktreeId::new(target_path.clone()),
            worktree: "feature-auth".to_string(),
            branch: Some("feature/auth".to_string()),
            path: target_path,
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![target.clone()]);
        state.refs = Some(RefSnapshot::from_parts(
            ["main", "feature/auth"],
            [] as [&str; 0],
            None,
            Some("main"),
            false,
        ));
        open_sync_dialog(&mut state, &target.id, Some("main")).unwrap();
        state.operation_modal = Some(crate::tui::operation_modal::OperationModal::new(
            crate::operation::OperationKind::Sync,
        ));
        std::fs::write(worktree_path.join("dirty.txt"), "dirty").unwrap();

        return_to_sync_form(&mut state, repository.path(), Some("main"), None);

        assert!(state.operation_modal.is_none());
        let error = state
            .sync_dialog
            .as_ref()
            .and_then(SyncDialog::validation_error)
            .expect("live dirty target should fail revalidation");
        assert!(error.to_lowercase().contains("uncommitted"), "{error}");
    }

    #[tokio::test]
    async fn temp_repo_sync_request_executes_through_the_shared_operation_adapter() {
        let repository = init_repo();
        let worktree_path = repository.path().join("worktrees/feature-auth");
        add_worktree(
            repository.path(),
            "feature/auth",
            "feature-auth",
            &worktree_path,
        );
        let target_path = worktree_path.canonicalize().unwrap();
        let request = build_sync_request(
            &SyncSubmission {
                target: WorktreeId::new(target_path.clone()),
                base: "main".to_string(),
                strategy: crate::cli::commands::sync::stateless::SyncStrategy::Rebase,
            },
            repository.path(),
            Some("main"),
            None,
        )
        .unwrap();
        let emitter = crate::operation::RecordingEmitter::default();

        let outcome = crate::operation::execute(request, &emitter).await.unwrap();

        assert!(matches!(
            outcome,
            OperationOutcome::Sync(ref outcome) if outcome.path == target_path
        ));
        assert!(emitter
            .events()
            .contains(&crate::operation::OperationEvent::MutationStarted));
        assert!(emitter.events().iter().any(|event| matches!(
            event,
            crate::operation::OperationEvent::StageStarted {
                stage: crate::operation::OperationStage::Sync
            }
        )));
    }

    #[tokio::test]
    async fn remove_controller_opens_selected_executes_shared_request_and_refreshes_notice() {
        let repository = init_repo();
        let target_path = repository.path().join("worktrees").join("feature");
        add_worktree(repository.path(), "feature", "feature", &target_path);
        let target = WorktreeIdentity {
            id: WorktreeId::new(&target_path),
            worktree: "feature".to_string(),
            branch: Some("feature".to_string()),
            path: target_path.clone(),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![target.clone()]);
        open_remove_dialog(&mut state, repository.path(), &target.id, Some("main")).unwrap();

        let Some(RemoveInputEffect::Start(request)) = handle_remove_input(
            &mut state,
            RemoveKey::Confirm,
            repository.path(),
            Some("main"),
            None,
            Instant::now(),
        )
        .unwrap() else {
            panic!("clean removal should start")
        };
        let outcome = crate::operation::execute(*request, &crate::operation::NoopEmitter)
            .await
            .unwrap();
        let OperationOutcome::Remove(outcome) = outcome else {
            panic!("expected removal outcome")
        };
        assert_eq!(outcome.confirmation.to_string(), "interactive");
        let publication = RefreshPublication {
            identities: Vec::new(),
            refs: None,
            updating_refs: false,
            warning: None,
        };
        let mut refresh = SuccessfulPostOperationRefresh::new(publication);
        let shown_at = Instant::now();
        finish_remove_success(&mut state, &mut refresh, outcome, shown_at);

        assert!(refresh.requested);
        assert!(state.identities.is_empty());
        assert!(state.remove_dialog.is_none());
        assert!(state
            .notification
            .as_ref()
            .is_some_and(|notice| notice.text == "Removed feature"));
        let _ = app::reduce(
            &mut state,
            Event::NotificationTick(shown_at + std::time::Duration::from_secs(5)),
        );
        assert!(state.notification.is_none());
    }

    #[test]
    fn returning_from_a_failed_remove_shows_live_risks() {
        let repository = init_repo();
        let target_path = repository.path().join("worktrees").join("late-dirty");
        add_worktree(repository.path(), "late-dirty", "late-dirty", &target_path);
        let target = WorktreeIdentity {
            id: WorktreeId::new(&target_path),
            worktree: "late-dirty".to_string(),
            branch: Some("late-dirty".to_string()),
            path: target_path.clone(),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![target.clone()]);
        open_remove_dialog(&mut state, repository.path(), &target.id, Some("main")).unwrap();
        std::fs::write(target_path.join("late.txt"), "late\n").unwrap();

        return_to_remove_form(&mut state, repository.path(), Some("main"));

        let dialog = state.remove_dialog.as_ref().unwrap();
        assert_eq!(
            dialog.risks(),
            [crate::tui::remove_flow::Risk::UncommittedChanges],
            "the form shows what Git has now"
        );
        assert_eq!(dialog.validation_error(), None);

        // `Enter y` typed in one go: the `y` arrives with the new risk
        // on screen for no time at all, so it must not remove anything.
        let _ = app::reduce(
            &mut state,
            Event::ViewportChanged {
                width: 80,
                height: 24,
            },
        );
        let mut press = |at: Instant| {
            handle_remove_input(
                &mut state,
                RemoveKey::Confirm,
                repository.path(),
                Some("main"),
                None,
                at,
            )
            .unwrap()
        };
        assert!(press(Instant::now()).is_none());
        assert!(target_path.join("late.txt").exists());
        assert!(matches!(
            press(Instant::now() + crate::tui::remove_flow::CONFIRM_DELAY_AFTER_CHANGE),
            Some(RemoveInputEffect::Start(_))
        ));
    }

    #[test]
    fn a_dirty_worktree_is_removed_only_after_y_and_never_by_enter() {
        let repository = init_repo();
        let target_path = repository.path().join("worktrees").join("dirty");
        add_worktree(repository.path(), "dirty", "dirty", &target_path);
        std::fs::write(target_path.join("dirty.txt"), "dirty\n").unwrap();
        let target = WorktreeIdentity {
            id: WorktreeId::new(&target_path),
            worktree: "dirty".to_string(),
            branch: Some("dirty".to_string()),
            path: target_path,
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![target.clone()]);
        open_remove_dialog(&mut state, repository.path(), &target.id, Some("main")).unwrap();

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(translate_remove_key(key(KeyCode::Enter)), None);
        assert_eq!(
            translate_remove_key(key(KeyCode::Char('y'))),
            Some(RemoveKey::Confirm)
        );
        assert_eq!(
            translate_remove_key(key(KeyCode::Char(' '))),
            Some(RemoveKey::Space)
        );
        assert_eq!(
            translate_remove_key(key(KeyCode::Esc)),
            Some(RemoveKey::Escape)
        );

        let Some(RemoveInputEffect::Start(request)) = handle_remove_input(
            &mut state,
            RemoveKey::Confirm,
            repository.path(),
            Some("main"),
            None,
            Instant::now() + crate::tui::remove_flow::CONFIRM_DELAY_AFTER_CHANGE,
        )
        .unwrap() else {
            panic!("y should start the removal")
        };
        let OperationRequest::Remove(request) = *request else {
            panic!("expected a remove request")
        };
        let plan = serde_json::to_value(&request.plan).unwrap();
        assert_eq!(plan["force_worktree_applied"], true);
    }

    #[test]
    fn a_missing_editor_is_a_short_status_message_that_names_the_fix() {
        let notice = editor_command_notice(&crate::navigation::EditorCommandError::Missing);

        assert_eq!(
            notice,
            "No editor configured. Set $EDITOR or [editor] command."
        );
        assert!(notice.chars().count() <= 60, "fits one status line");

        let mut state = AppState::new(Vec::new());
        notify(&mut state, NoticeKind::Error, notice.clone());
        let shown = state.notification.as_ref().unwrap();
        assert_eq!(shown.kind, NoticeKind::Error);
        assert_eq!(shown.text, notice);
    }

    #[test]
    fn opening_create_before_branches_load_explains_instead_of_doing_nothing() {
        let mut state = AppState::new(Vec::new());

        let error =
            open_create_dialog(&mut state, "trench", Path::new("/worktrees"), None).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Branches are still loading. Try again in a moment."
        );
        assert!(state.create_dialog.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_created_worktree_is_selected_when_its_root_is_behind_a_symlink() {
        let repository = init_repo();
        let real_root = repository.path().canonicalize().unwrap().join("real-root");
        std::fs::create_dir_all(real_root.join("trench")).unwrap();
        let linked_root = repository.path().join("linked-root");
        std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();
        let created = real_root.join("trench").join("feature-auth");
        std::fs::create_dir(&created).unwrap();
        let listed = WorktreeIdentity {
            id: WorktreeId::new(&created),
            worktree: "feature-auth".to_string(),
            branch: Some("feature/auth".to_string()),
            path: created.clone(),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let main = WorktreeIdentity {
            id: WorktreeId::new(repository.path()),
            worktree: "trench".to_string(),
            branch: Some("main".to_string()),
            path: repository.path().to_path_buf(),
            is_main: true,
            ..listed.clone()
        };
        let mut state = AppState::new(vec![main.clone()]);
        let mut refresh = SuccessfulPostOperationRefresh::new(RefreshPublication {
            identities: vec![main, listed.clone()],
            refs: None,
            updating_refs: false,
            warning: None,
        });

        finish_create_success(
            &mut state,
            &mut refresh,
            crate::operation::CreateOutcome {
                plan: crate::create_plan::CreatePlan {
                    dry_run: false,
                    action: CreateAction::NewBranch("main".to_string()),
                    branch: "feature/auth".to_string(),
                    worktree: "feature-auth".to_string(),
                    // The plan spells the path through the symlink, as the
                    // configured root does.
                    path: linked_root.join("trench").join("feature-auth"),
                    base: Some("main".to_string()),
                    tracking: None,
                    hook_policy: HookPolicy::Run,
                    preconditions: Vec::new(),
                    source_oid: None,
                },
                mutation_state: crate::operation::MutationState::Applied,
            },
            Instant::now(),
        );

        assert_eq!(state.selected.as_ref(), Some(&listed.id));
        assert_eq!(
            state
                .notification
                .as_ref()
                .map(|notice| notice.text.as_str()),
            Some("Created feature/auth (new branch from main)")
        );
    }

    #[test]
    fn y_does_nothing_while_the_terminal_is_too_small_to_show_every_risk() {
        let repository = init_repo();
        let target_path = repository.path().join("worktrees").join("risky");
        add_worktree(repository.path(), "risky", "risky", &target_path);
        std::fs::write(target_path.join("feature.txt"), "feature\n").unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&target_path)
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["add", "feature.txt"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "feature",
        ]);
        std::fs::write(target_path.join("dirty.txt"), "dirty\n").unwrap();
        let target = WorktreeIdentity {
            id: WorktreeId::new(&target_path),
            worktree: "risky".to_string(),
            branch: Some("risky".to_string()),
            path: target_path.clone(),
            head: None,
            is_main: false,
            is_current: false,
            detached: false,
            missing: false,
        };
        let mut state = AppState::new(vec![target.clone()]);
        open_remove_dialog(&mut state, repository.path(), &target.id, Some("main")).unwrap();
        let press = |state: &mut AppState, key| {
            handle_remove_input(
                state,
                key,
                repository.path(),
                Some("main"),
                None,
                Instant::now() + crate::tui::remove_flow::CONFIRM_DELAY_AFTER_CHANGE * 4,
            )
            .unwrap()
        };
        assert!(press(&mut state, RemoveKey::Space).is_none());

        let _ = app::reduce(
            &mut state,
            Event::ViewportChanged {
                width: 40,
                height: 10,
            },
        );
        assert!(press(&mut state, RemoveKey::Confirm).is_none());
        assert!(target_path.join("dirty.txt").exists());
        assert!(state.remove_dialog.is_some());

        let _ = app::reduce(
            &mut state,
            Event::ViewportChanged {
                width: 80,
                height: 24,
            },
        );
        assert!(matches!(
            press(&mut state, RemoveKey::Confirm),
            Some(RemoveInputEffect::Start(_))
        ));
    }

    #[test]
    fn escape_and_enter_both_leave_a_failed_operation_through_the_runtime() {
        let repository = init_repo();
        let root = repository.path().join("worktrees");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let failing = HooksConfig {
            pre_create: Some(crate::config::HookDef {
                run: Some(vec!["false".to_string()]),
                ..crate::config::HookDef::default()
            }),
            ..HooksConfig::default()
        };
        let start = || {
            let dispatch = build_create_dispatch(
                &CreateSubmission {
                    branch: "feature/fails".to_string(),
                    from: None,
                },
                repository.path(),
                &root,
                Some("main"),
                Some(failing.clone()),
            )
            .unwrap();
            let CreateDispatch::Run(request) = dispatch else {
                panic!("expected a create request")
            };
            let mut operation =
                OperationRuntime::new(ThreadOperationLauncher, SystemRuntimeClock::default());
            operation.start(*request);
            let deadline = Instant::now() + std::time::Duration::from_secs(20);
            while !operation.modal().is_some_and(|modal| {
                matches!(
                    modal.status(),
                    crate::tui::operation_modal::ModalStatus::Failed { .. }
                )
            }) {
                assert!(Instant::now() < deadline, "the hook never failed");
                let _ = operation.tick();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            operation
        };

        let mut operation = start();
        assert_eq!(
            operation_modal_effect(&mut operation, ModalKey::Escape, area),
            Some(ModalEffect::Close),
            "Esc is on the hint bar, so it must work"
        );

        let mut operation = start();
        assert_eq!(
            operation_modal_effect(&mut operation, ModalKey::Enter, area),
            Some(ModalEffect::ReturnToForm)
        );
    }

    #[test]
    fn a_terminal_below_the_minimum_routes_only_quit() {
        let mut state = AppState::new(Vec::new());
        let _ = app::reduce(
            &mut state,
            Event::ViewportChanged {
                width: 39,
                height: 10,
            },
        );

        let confirm = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(visible_surface_key(&state, confirm), None);
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(visible_surface_key(&state, quit), Some(quit));
    }

    struct ControllerHarness {
        repository: TempDir,
        state: AppState,
    }

    impl ControllerHarness {
        fn new() -> Self {
            Self {
                repository: init_repo(),
                state: AppState::new(Vec::new()),
            }
        }

        fn root(&self) -> &Path {
            self.repository.path()
        }

        fn add(&mut self, branch: &str) -> WorktreeIdentity {
            let name = branch.replace('/', "-");
            let path = self.root().join("worktrees").join(&name);
            add_worktree(self.root(), branch, &name, &path);
            let path = path.canonicalize().unwrap();
            let identity = WorktreeIdentity {
                id: WorktreeId::new(&path),
                worktree: name,
                branch: Some(branch.to_string()),
                path,
                head: None,
                is_main: false,
                is_current: false,
                detached: false,
                missing: false,
            };
            self.state.identities.push(identity.clone());
            self.state.selected = Some(identity.id.clone());
            identity
        }

        fn refs(&mut self, branches: &[&str]) {
            self.state.refs = Some(RefSnapshot::from_parts(
                branches.iter().copied(),
                [] as [&str; 0],
                None,
                Some("main"),
                false,
            ));
        }

        fn search(&mut self, query: &str) {
            let _ = app::reduce(&mut self.state, Event::Input(Key::Char('/')));
            for character in query.chars() {
                let _ = app::reduce(&mut self.state, Event::Input(Key::Char(character)));
            }
        }
    }

    #[tokio::test]
    async fn controller_harness_covers_cross_boundary_workflow_matrix() {
        // Launcher and search route every action to the filtered identity.
        let mut launcher = ControllerHarness::new();
        launcher.add("alpha");
        let beta = launcher.add("beta");
        launcher.search("bet");
        let _ = app::reduce(&mut launcher.state, Event::Input(Key::Escape));
        for (key, expected) in [
            (Key::Enter, app::Effect::Switch(beta.id.clone())),
            (Key::Char('o'), app::Effect::Open(beta.id.clone())),
            (Key::Char('s'), app::Effect::OpenSync(beta.id.clone())),
            (Key::Char('d'), app::Effect::OpenRemove(beta.id.clone())),
        ] {
            assert_eq!(
                app::reduce(&mut launcher.state, Event::Input(key)),
                vec![expected]
            );
        }

        // Create uses the live planner and preserves refresh failure truth.
        let mut create = ControllerHarness::new();
        create.refs(&["main"]);
        let worktree_root = create.root().join("worktrees");
        let request = build_create_dispatch(
            &CreateSubmission {
                branch: "feature/new".to_string(),
                from: None,
            },
            create.root(),
            &worktree_root,
            Some("main"),
            None,
        )
        .unwrap();
        assert!(
            matches!(request, CreateDispatch::Run(ref request) if matches!(**request, OperationRequest::Create(_)))
        );
        let CreateDispatch::Run(request) = request else {
            unreachable!()
        };
        let outcome = crate::operation::execute(*request, &crate::operation::NoopEmitter)
            .await
            .unwrap();
        let OperationOutcome::Create(outcome) = outcome else {
            unreachable!()
        };
        let mut failed_refresh = FailingPostOperationRefresh::default();
        finish_create_success(
            &mut create.state,
            &mut failed_refresh,
            outcome,
            Instant::now(),
        );
        assert!(failed_refresh.requested);
        assert!(create.state.refresh.warning.is_some());

        // Sync uses the planner and operation adapter, then refreshes comparisons.
        let mut sync = ControllerHarness::new();
        let target = sync.add("feature/sync");
        sync.refs(&["main", "feature/sync"]);
        let request = build_sync_request(
            &SyncSubmission {
                target: target.id.clone(),
                base: "main".to_string(),
                strategy: crate::cli::commands::sync::stateless::SyncStrategy::Rebase,
            },
            sync.root(),
            Some("main"),
            None,
        )
        .unwrap();
        let events = crate::operation::RecordingEmitter::default();
        assert!(matches!(
            crate::operation::execute(request, &events).await.unwrap(),
            OperationOutcome::Sync(_)
        ));
        assert!(events
            .events()
            .contains(&crate::operation::OperationEvent::MutationStarted));

        // Remove exercises clean execution, stale-risk review, and retained recovery truth.
        let mut remove = ControllerHarness::new();
        let clean = remove.add("feature/remove");
        let remove_root = remove.root().to_path_buf();
        open_remove_dialog(&mut remove.state, &remove_root, &clean.id, Some("main")).unwrap();
        let Some(RemoveInputEffect::Start(request)) = handle_remove_input(
            &mut remove.state,
            RemoveKey::Confirm,
            &remove_root,
            Some("main"),
            None,
            Instant::now(),
        )
        .unwrap() else {
            panic!("clean remove should start")
        };
        assert!(matches!(
            crate::operation::execute(*request, &crate::operation::NoopEmitter)
                .await
                .unwrap(),
            OperationOutcome::Remove(_)
        ));
        let mut modal = crate::tui::operation_modal::OperationModal::new(
            crate::operation::OperationKind::Remove,
        );
        modal.fail(&crate::operation::OperationFailure {
            stage: crate::operation::OperationStage::RemoveWorktree,
            mutation_state: crate::operation::MutationState::PartiallyApplied,
            class: crate::operation::ErrorClass::Git,
            message: "restore manually".to_string(),
            retained_quarantine: Some(remove_root.join("retained")),
        });
        assert_eq!(
            modal.handle_key(crate::tui::operation_modal::ModalKey::Enter),
            Some(crate::tui::operation_modal::ModalEffect::Close)
        );
    }

    fn init_repo() -> TempDir {
        let directory = TempDir::new().unwrap();
        let repository = git2::Repository::init(directory.path()).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repository.index().unwrap().write_tree().unwrap();
        let tree = repository.find_tree(tree_id).unwrap();
        repository
            .commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .unwrap();
        drop(tree);
        drop(repository);
        directory
    }

    fn add_worktree(repo_path: &Path, branch: &str, name: &str, path: &Path) {
        let repository = git2::Repository::open(repo_path).unwrap();
        if repository
            .find_branch(branch, git2::BranchType::Local)
            .is_err()
        {
            let commit = repository.head().unwrap().peel_to_commit().unwrap();
            repository.branch(branch, &commit, false).unwrap();
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let reference = repository
            .find_reference(&format!("refs/heads/{branch}"))
            .unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&reference));
        repository.worktree(name, path, Some(&options)).unwrap();
    }

    fn prune_worktree(repo_path: &Path, name: &str, path: &Path) {
        let repository = git2::Repository::open(repo_path).unwrap();
        let worktree = repository.find_worktree(name).unwrap();
        std::fs::remove_dir_all(path).unwrap();
        worktree.prune(None).unwrap();
    }

    #[derive(Default)]
    struct FailingPostOperationRefresh {
        requested: bool,
    }

    struct SuccessfulPostOperationRefresh {
        requested: bool,
        publication: Option<RefreshPublication>,
    }

    impl SuccessfulPostOperationRefresh {
        fn new(publication: RefreshPublication) -> Self {
            Self {
                requested: false,
                publication: Some(publication),
            }
        }
    }

    impl PostOperationRefresh for SuccessfulPostOperationRefresh {
        fn request_post_operation(&mut self) -> Result<()> {
            self.requested = true;
            Ok(())
        }

        fn apply_publications(&mut self, state: &mut AppState) {
            apply_refresh_publication(state, self.publication.take().unwrap());
        }
    }

    impl PostOperationRefresh for FailingPostOperationRefresh {
        fn request_post_operation(&mut self) -> Result<()> {
            self.requested = true;
            anyhow::bail!("injected refresh failure")
        }

        fn apply_publications(&mut self, _state: &mut AppState) {}
    }

    #[test]
    fn editor_effect_suspends_launches_waits_resumes_then_requests_local_refresh() {
        let (result, events) = editor_effect(FakeEditorResult::Status(EditorStatus {
            success: true,
            code: Some(0),
        }));

        result.unwrap();
        assert_eq!(
            events,
            [
                "suspend",
                "launch:editor:[\"--configured\", \"two words\"]:/tmp/work tree",
                "resume",
                "refresh:EditorReturn",
            ]
        );
    }

    #[test]
    fn editor_spawn_failure_still_resumes_and_requests_local_refresh() {
        let (result, events) = editor_effect(FakeEditorResult::SpawnFailure);

        assert!(result.is_err());
        assert_eq!(
            events,
            [
                "suspend",
                "launch:editor:[\"--configured\", \"two words\"]:/tmp/work tree",
                "resume",
                "refresh:EditorReturn",
            ]
        );
    }

    #[test]
    fn editor_nonzero_exit_still_resumes_and_requests_local_refresh() {
        let (result, events) = editor_effect(FakeEditorResult::Status(EditorStatus {
            success: false,
            code: Some(17),
        }));

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("editor exited with status 17"));
        assert_eq!(
            events,
            [
                "suspend",
                "launch:editor:[\"--configured\", \"two words\"]:/tmp/work tree",
                "resume",
                "refresh:EditorReturn",
            ]
        );
    }
}
