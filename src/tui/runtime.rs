use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind};

use crate::{
    config,
    navigation::EditorCommand,
    tui::{
        app::{self, AppState, Effect, Event, WorktreeId},
        cockpit,
        keymap::Key,
        refresh_runtime::RefreshRuntime,
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
    Search,
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

    super::install_panic_hook();
    let mut terminal = ratatui::init();
    let result = (|| -> Result<TuiExit> {
        'event_loop: loop {
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
                    Effect::OpenCreate => dialogs.register(DialogRequest::Create),
                    Effect::OpenSync(id) => dialogs.register(DialogRequest::Sync(id)),
                    Effect::OpenRemove(id) => dialogs.register(DialogRequest::Remove(id)),
                    Effect::OpenSearch => dialogs.register(DialogRequest::Search),
                    Effect::Refresh => {
                        refresh.manual()?;
                        apply_refresh_publications(&mut refresh, &mut state);
                    }
                    Effect::Quit => break 'event_loop Ok(TuiExit::Quit),
                    Effect::Unavailable { .. } => {}
                }
            }

            // Later cockpit layers consume the typed request and render their overlay.
            // Keeping the registry live here makes unhandled actions observable to them.
            let _ = dialogs.pending();
        }
    })();

    ratatui::restore();
    super::restore_panic_hook();
    result
}

fn apply_refresh_publications(refresh: &mut RefreshRuntime, state: &mut AppState) {
    for publication in refresh.drain_publications() {
        let _ = app::reduce(state, Event::RefreshPublished(publication));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

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
    fn dialog_registry_is_a_replaceable_operation_seam() {
        let mut dialogs = DialogRegistry::default();
        dialogs.register(DialogRequest::Search);
        assert_eq!(dialogs.pending(), Some(&DialogRequest::Search));
        assert_eq!(dialogs.take(), Some(DialogRequest::Search));
        assert_eq!(dialogs.pending(), None);
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
