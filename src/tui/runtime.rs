use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind};

use crate::{
    cli::commands::sync::stateless::{HookPolicy as SyncHookPolicy, SyncPlanner},
    config::{self, HooksConfig},
    create_plan::{CreateAction, CreatePlanner, HookPolicy},
    navigation::EditorCommand,
    operation::{CreateRequest, OperationOutcome, OperationRequest, SyncRequest},
    tui::{
        app::{self, AppState, Effect, Event, WorktreeId},
        cockpit,
        create_flow::{
            CheckedOutBranch, CreateDialog, CreateEffect, CreateKey, CreateSubmission,
            OriginRefresh,
        },
        keymap::Key,
        operation_modal::{ModalEffect, ModalKey},
        operation_runtime::{
            OperationRuntime, OperationRuntimeEffect, SystemRuntimeClock, ThreadOperationLauncher,
        },
        refresh::RefreshPublication,
        refresh_runtime::RefreshRuntime,
        sync_flow::{SyncDialog, SyncEffect, SyncKey, SyncSubmission},
        theme,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogRequest {
    Create,
    Sync(WorktreeId),
    Remove(WorktreeId),
}

#[derive(Debug, Default)]
pub struct DialogRegistry {
    pending: Option<DialogRequest>,
}

impl DialogRegistry {
    pub fn pending(&self) -> Option<&DialogRequest> {
        self.pending.as_ref()
    }

    pub fn take(&mut self) -> Option<DialogRequest> {
        self.pending.take()
    }

    pub fn register(&mut self, request: DialogRequest) {
        self.pending = Some(request);
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

#[derive(Debug)]
enum CreateInputEffect {
    RefreshOrigin,
    Navigate(WorktreeId),
    Start(Box<OperationRequest>),
}

#[derive(Debug)]
enum SyncInputEffect {
    RefreshOrigin,
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

fn open_create_dialog(
    state: &mut AppState,
    repository: &str,
    worktree_root: &Path,
    configured_base: Option<&str>,
) -> Result<()> {
    let refs = state.refs.clone().context("references are still loading")?;
    let checked_out = state
        .identities
        .iter()
        .filter_map(|identity| {
            identity.branch.as_ref().map(|branch| {
                CheckedOutBranch::new(branch, identity.id.clone(), identity.path.clone())
            })
        })
        .collect::<Vec<_>>();
    state.help_open = false;
    state.create_dialog = Some(CreateDialog::new_with_configured_base(
        repository,
        worktree_root,
        refs,
        configured_base,
        checked_out,
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
    if let Some(reason) =
        crate::tui::sync_flow::unavailable_reason(&identity, state.statuses.get(&identity.id))
    {
        anyhow::bail!(reason);
    }
    let refs = state.refs.clone().context("references are still loading")?;
    state.help_open = false;
    state.sync_dialog = Some(SyncDialog::new(&identity, refs, configured_base));
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
        Some(CreateEffect::RefreshOrigin) => {
            if let Some(dialog) = state.create_dialog.as_mut() {
                dialog.set_origin_refresh(OriginRefresh::Loading);
            }
            return Ok(Some(CreateInputEffect::RefreshOrigin));
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
                Err(error) => {
                    if let Some(dialog) = state.create_dialog.as_mut() {
                        dialog.set_validation_error(Some(error.to_string()));
                    }
                    return Ok(None);
                }
            };
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

fn finish_create_success(
    state: &mut AppState,
    refresh: &mut impl PostOperationRefresh,
    outcome: crate::operation::CreateOutcome,
    shown_at: Instant,
) {
    let refresh_result = refresh.request_post_operation();
    let (message, warning) = match refresh_result.as_ref() {
        Ok(()) => (format!("Created {}", outcome.plan.branch), None),
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
            select: Some(WorktreeId::new(outcome.plan.path)),
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

pub fn run() -> Result<TuiExit> {
    let cwd = std::env::current_dir()?;
    let repo = crate::git::discover_repo(&cwd)?;
    let global = config::load_global_config()?;
    let project = config::load_project_config(&repo.path)?;
    let resolved = config::resolve_config(None, project.as_ref(), &global);
    let mut refresh = RefreshRuntime::new(
        cwd.clone(),
        repo.path.clone(),
        resolved.git.default_base.clone(),
    );
    refresh.launch()?;
    let mut state = AppState::new(Vec::new());
    apply_refresh_publications(&mut refresh, &mut state);
    let selected_theme = theme::from_name(&resolved.ui.theme);
    let mut dialogs = DialogRegistry::default();
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
                    OperationRuntimeEffect::Succeeded(OperationOutcome::Sync(_)) => {
                        operation.dismiss();
                    }
                    OperationRuntimeEffect::Succeeded(OperationOutcome::Remove(_)) => {
                        operation.dismiss();
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
                cockpit::render(&state, frame, frame.area(), &selected_theme);
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
            if key.code == KeyCode::Char('c')
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
            {
                break 'event_loop Ok(TuiExit::Quit);
            }

            if (state.operation_modal.is_some()
                || state.create_dialog.is_some()
                || state.sync_dialog.is_some())
                && state.help_open
            {
                if matches!(key.code, KeyCode::Char('?') | KeyCode::Esc) {
                    state.help_open = false;
                }
                continue;
            }

            if state.operation_modal.is_some() {
                if key.code == KeyCode::Char('?') {
                    state.help_open = true;
                    continue;
                }
                let Some(modal_key) = translate_modal_key(key) else {
                    continue;
                };
                let modal_effect = if modal_key == ModalKey::Escape {
                    operation.cancel().then_some(ModalEffect::Cancel)
                } else {
                    operation
                        .modal_mut()
                        .and_then(|modal| modal.handle_key(modal_key))
                };
                match modal_effect {
                    Some(ModalEffect::Cancel) => {
                        operation.dismiss();
                        state.operation_modal = None;
                    }
                    Some(ModalEffect::ReturnToForm) => {
                        operation.dismiss();
                        if let Err(error) = refresh.post_operation() {
                            tracing::warn!(%error, "failed to refresh before create revalidation");
                        }
                        apply_refresh_publications(&mut refresh, &mut state);
                        return_to_create_form(
                            &mut state,
                            &cwd,
                            &resolved.worktrees.root,
                            resolved.git.default_base.as_deref(),
                            resolved.hooks.clone(),
                        );
                    }
                    None => {}
                }
                state.operation_modal = operation.modal().cloned();
                continue;
            }

            if state.create_dialog.is_some() {
                if key.code == KeyCode::Char('?') {
                    state.help_open = true;
                    continue;
                }
                let Some(create_key) = translate_create_key(key) else {
                    continue;
                };
                match handle_create_input(
                    &mut state,
                    create_key,
                    &cwd,
                    &resolved.worktrees.root,
                    resolved.git.default_base.as_deref(),
                    resolved.hooks.clone(),
                )? {
                    Some(CreateInputEffect::RefreshOrigin) => {
                        if let Err(error) = refresh.ref_picker() {
                            tracing::warn!(%error, "base picker origin refresh failed");
                            if let Some(dialog) = state.create_dialog.as_mut() {
                                dialog.set_origin_refresh(OriginRefresh::Failed);
                            }
                        }
                        apply_refresh_publications(&mut refresh, &mut state);
                    }
                    Some(CreateInputEffect::Navigate(id)) => match refresh.post_operation() {
                        Ok(()) => {
                            apply_refresh_publications(&mut refresh, &mut state);
                            if state.identities.iter().any(|row| row.id == id) {
                                let _ = app::reduce(&mut state, Event::Select(id));
                                state.create_dialog = None;
                                state.help_open = false;
                            } else if let Some(dialog) = state.create_dialog.as_mut() {
                                dialog.set_validation_error(Some(
                                    "Checked-out worktree changed; press Enter to revalidate"
                                        .to_string(),
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
                    None => {}
                }
                continue;
            }

            if state.sync_dialog.is_some() {
                if key.code == KeyCode::Char('?') {
                    state.help_open = true;
                    continue;
                }
                let Some(sync_key) = translate_sync_key(key) else {
                    continue;
                };
                match handle_sync_input(
                    &mut state,
                    sync_key,
                    &cwd,
                    resolved.git.default_base.as_deref(),
                    resolved.hooks.clone(),
                )? {
                    Some(SyncInputEffect::RefreshOrigin) => {
                        if let Err(error) = refresh.ref_picker() {
                            tracing::warn!(%error, "base picker origin refresh failed");
                            if let Some(dialog) = state.sync_dialog.as_mut() {
                                dialog.set_origin_refresh(OriginRefresh::Failed);
                            }
                        }
                        apply_refresh_publications(&mut refresh, &mut state);
                    }
                    Some(SyncInputEffect::Start(request)) => {
                        operation.start(*request);
                        state.operation_modal = operation.modal().cloned();
                    }
                    None => {}
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
                    Effect::Open(id) => {
                        let command =
                            EditorCommand::from_environment(resolved.editor_command.as_deref())?;
                        let mut terminal_driver = RatatuiTerminalDriver {
                            terminal: &mut terminal,
                        };
                        let mut launcher = ProcessEditorLauncher;
                        let mut refresh_driver = RuntimeRefreshDriver {
                            runtime: &mut refresh,
                            state: &mut state,
                        };
                        if let Err(error) = open_editor(
                            &mut terminal_driver,
                            &mut launcher,
                            &mut refresh_driver,
                            &command,
                            id.as_path(),
                        ) {
                            tracing::warn!(error = %error, "editor navigation failed");
                        }
                    }
                    Effect::OpenCreate => {
                        if let Err(error) = open_create_dialog(
                            &mut state,
                            &repo.name,
                            &resolved.worktrees.root,
                            resolved.git.default_base.as_deref(),
                        ) {
                            tracing::warn!(%error, "create dialog unavailable");
                        }
                    }
                    Effect::OpenSync(id) => {
                        if let Err(error) =
                            open_sync_dialog(&mut state, &id, resolved.git.default_base.as_deref())
                        {
                            let _ = app::reduce(
                                &mut state,
                                Event::NotificationShown {
                                    message: error.to_string(),
                                    shown_at: Instant::now(),
                                },
                            );
                        }
                    }
                    Effect::OpenRemove(id) => dialogs.register(DialogRequest::Remove(id)),
                    Effect::Refresh => {
                        refresh.manual()?;
                        apply_refresh_publications(&mut refresh, &mut state);
                    }
                    Effect::Quit => break 'event_loop Ok(TuiExit::Quit),
                    Effect::Unavailable { reason, .. } => {
                        let _ = app::reduce(
                            &mut state,
                            Event::NotificationShown {
                                message: reason,
                                shown_at: Instant::now(),
                            },
                        );
                    }
                }
            }

            // Sync and remove layers consume their typed requests independently.
            let _ = dialogs.pending();
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
    if let Some(dialog) = state.create_dialog.as_mut() {
        if let Some(refs) = refs.clone() {
            dialog.update_refs(refs);
        }
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
    match key.code {
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Escape),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Char(character) => Some(Key::Char(character)),
        _ => None,
    }
}

fn translate_create_key(key: KeyEvent) -> Option<CreateKey> {
    match key.code {
        KeyCode::Enter => Some(CreateKey::Enter),
        KeyCode::Esc => Some(CreateKey::Escape),
        KeyCode::Up => Some(CreateKey::Up),
        KeyCode::Down => Some(CreateKey::Down),
        KeyCode::Tab => Some(CreateKey::Tab),
        KeyCode::Backspace => Some(CreateKey::Backspace),
        KeyCode::Char(character) => Some(CreateKey::Character(character)),
        _ => None,
    }
}

fn translate_sync_key(key: KeyEvent) -> Option<SyncKey> {
    match key.code {
        KeyCode::Enter => Some(SyncKey::Enter),
        KeyCode::Esc => Some(SyncKey::Escape),
        KeyCode::Up => Some(SyncKey::Up),
        KeyCode::Down => Some(SyncKey::Down),
        KeyCode::Left => Some(SyncKey::Left),
        KeyCode::Right => Some(SyncKey::Right),
        KeyCode::Tab => Some(SyncKey::Tab),
        KeyCode::Backspace => Some(SyncKey::Backspace),
        KeyCode::Char(character) => Some(SyncKey::Character(character)),
        _ => None,
    }
}

fn translate_modal_key(key: KeyEvent) -> Option<ModalKey> {
    match key.code {
        KeyCode::Enter => Some(ModalKey::Enter),
        KeyCode::Esc => Some(ModalKey::Escape),
        KeyCode::Up => Some(ModalKey::Up),
        KeyCode::Down => Some(ModalKey::Down),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

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
            Some(Key::Backspace)
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
    fn terminal_keys_translate_to_create_form_vocabulary() {
        let modifiers = crossterm::event::KeyModifiers::NONE;
        assert_eq!(
            translate_create_key(KeyEvent::new(KeyCode::Tab, modifiers)),
            Some(CreateKey::Tab)
        );
        assert_eq!(
            translate_create_key(KeyEvent::new(KeyCode::Backspace, modifiers)),
            Some(CreateKey::Backspace)
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
        assert_eq!(dialog.preview().unwrap().base.as_deref(), Some("release"));
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
        };
        let mut state = AppState::new(vec![identity.clone()]);
        state
            .statuses
            .insert(identity.id.clone(), Default::default());
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
                statuses: BTreeMap::from([(
                    WorktreeId::new(repository.path()),
                    Default::default(),
                )]),
                waiting_rows: BTreeSet::new(),
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
    fn create_form_routes_submit_and_ref_picker_without_leaking_into_cockpit_keys() {
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

        assert!(matches!(
            handle_create_input(
                &mut state,
                CreateKey::Tab,
                repository.path(),
                &root,
                Some("main"),
                None,
            )
            .unwrap(),
            Some(CreateInputEffect::RefreshOrigin)
        ));
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .handle_key(CreateKey::Escape);
        let start = handle_create_input(
            &mut state,
            CreateKey::Enter,
            repository.path(),
            &root,
            Some("main"),
            None,
        )
        .unwrap();
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
        state.create_dialog.as_mut().unwrap().open_base_picker();

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
                statuses: BTreeMap::new(),
                waiting_rows: BTreeSet::new(),
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
                statuses: BTreeMap::new(),
                waiting_rows: BTreeSet::new(),
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

        assert!(state
            .create_dialog
            .as_ref()
            .unwrap()
            .validation_error()
            .is_some());
    }

    #[test]
    fn invalid_submission_stays_in_the_form_with_a_validation_message() {
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
        assert!(state
            .create_dialog
            .as_ref()
            .unwrap()
            .validation_error()
            .is_some());
    }

    #[test]
    fn checked_out_enter_replans_when_the_worktree_disappears_while_form_is_open() {
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

        let Some(CreateInputEffect::Start(request)) = effect else {
            panic!("disappeared checkout should be replanned")
        };
        let OperationRequest::Create(request) = *request else {
            panic!("expected create request")
        };
        assert_eq!(request.plan.action, CreateAction::ExistingLocal);
        assert_ne!(request.plan.path, old_path);
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
    fn dialog_registry_is_a_replaceable_operation_seam() {
        let mut dialogs = DialogRegistry::default();
        dialogs.register(DialogRequest::Create);
        assert_eq!(dialogs.pending(), Some(&DialogRequest::Create));
        assert_eq!(dialogs.take(), Some(DialogRequest::Create));
        assert_eq!(dialogs.pending(), None);
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
