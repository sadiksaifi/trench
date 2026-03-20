pub mod screens;
pub mod theme;
pub mod watcher;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{layout::Alignment, widgets::Paragraph, Frame};

use crate::paths;
use crate::state::Database;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    List,
    Detail,
    Create,
    Help,
    SyncPicker,
    DeleteConfirm,
    HookLog,
}

type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync;

/// Stores the pre-TUI panic hook so `restore_panic_hook` can put it back.
static PREV_PANIC_HOOK: Mutex<Option<Arc<PanicHook>>> = Mutex::new(None);

/// Launch the TUI. This is the single public entry point.
pub fn run() -> Result<()> {
    install_panic_hook();
    let mut terminal = ratatui::init();
    let mut app = App::new();

    // Load config once and apply theme + auto_refresh
    let resolved_config = if let Ok(global) = crate::config::load_global_config() {
        let project = std::env::current_dir()
            .ok()
            .and_then(|cwd| crate::git::discover_repo(&cwd).ok())
            .and_then(|ri| crate::config::load_project_config(&ri.path).ok().flatten());
        Some(crate::config::resolve_config(None, project.as_ref(), &global))
    } else {
        None
    };

    if let Some(ref resolved) = resolved_config {
        app.theme = theme::from_name(&resolved.ui.theme);
    }

    // Set auto_refresh before any refresh that may build a watcher
    app.auto_refresh = resolved_config
        .as_ref()
        .map(|c| c.ui.auto_refresh)
        .unwrap_or(true);

    // Load worktree data before entering the event loop
    app.refresh_list();

    // Restore session state (selected worktree, scroll position) from last run
    app.restore_list_session();

    let result = (|| -> Result<()> {
        while app.is_running() {
            // Process any pending hook output messages
            app.process_hook_messages();

            // Check filesystem watcher for auto-refresh
            app.check_watcher();

            terminal.draw(|frame| app.ui(frame))?;

            // Non-blocking poll: wait up to 50ms for key events, allowing
            // hook messages to be processed between frames for live streaming.
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.handle_key_event(key);
                    }
                }
            }

            if let Some(path) = app.editor_request.take() {
                ratatui::restore();
                let editor = std::env::var("EDITOR")
                    .or_else(|_| std::env::var("VISUAL"))
                    .unwrap_or_else(|_| "vi".into());
                let _ = std::process::Command::new(&editor).arg(&path).status();
                terminal = ratatui::init();
            }
        }
        Ok(())
    })();

    ratatui::restore();
    restore_panic_hook();
    result
}

fn install_panic_hook() {
    let original: Arc<PanicHook> = Arc::from(std::panic::take_hook());
    PREV_PANIC_HOOK
        .lock()
        .unwrap()
        .replace(Arc::clone(&original));
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original(info);
    }));
}

fn restore_panic_hook() {
    let _ = std::panic::take_hook();
    if let Some(hook) = PREV_PANIC_HOOK.lock().unwrap().take() {
        std::panic::set_hook(Box::new(move |info| hook(info)));
    }
}

pub struct App {
    running: bool,
    nav_stack: Vec<Screen>,
    pub theme: theme::Theme,
    pub list_state: screens::list::ListState,
    pub detail_state: Option<screens::detail::DetailState>,
    pub create_state: Option<screens::create::CreateState>,
    pub sync_picker_state: Option<screens::sync_picker::SyncPickerState>,
    pub delete_confirm_state: Option<screens::delete_confirm::DeleteConfirmState>,
    pub hook_log_state: Option<screens::hook_log::HookLogState>,
    pub hook_rx: Option<std::sync::mpsc::Receiver<screens::hook_log::HookOutputMessage>>,
    pub editor_request: Option<String>,
    pub repo_path: Option<String>,
    pub switch_path: Option<String>,
    pub auto_refresh: bool,
    pub watcher: Option<watcher::DebouncedWatcher>,
}

impl App {
    pub fn new() -> Self {
        Self {
            running: true,
            nav_stack: vec![Screen::List],
            theme: theme::from_name("catppuccin"),
            list_state: screens::list::ListState::new(vec![]),
            detail_state: None,
            create_state: None,
            sync_picker_state: None,
            delete_confirm_state: None,
            hook_log_state: None,
            hook_rx: None,
            editor_request: None,
            repo_path: None,
            switch_path: None,
            auto_refresh: true,
            watcher: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn active_screen(&self) -> Screen {
        *self
            .nav_stack
            .last()
            .expect("nav stack must never be empty")
    }

    pub fn nav_stack_depth(&self) -> usize {
        self.nav_stack.len()
    }

    pub fn push_screen(&mut self, screen: Screen) {
        self.nav_stack.push(screen);
    }

    pub fn ui(&self, frame: &mut Frame) {
        let theme = &self.theme;
        match self.active_screen() {
            Screen::List => screens::list::render(&self.list_state, frame, frame.area(), theme),
            Screen::Detail => {
                if let Some(ref detail) = self.detail_state {
                    screens::detail::render(detail, frame, frame.area(), theme);
                } else {
                    let placeholder =
                        Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
            Screen::SyncPicker => {
                if let Some(ref picker) = self.sync_picker_state {
                    screens::sync_picker::render(picker, frame, frame.area(), theme);
                } else {
                    let placeholder =
                        Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
            Screen::DeleteConfirm => {
                // Render list underneath, then overlay the dialog
                screens::list::render(&self.list_state, frame, frame.area(), theme);
                if let Some(ref confirm) = self.delete_confirm_state {
                    screens::delete_confirm::render(confirm, frame, frame.area(), theme);
                }
            }
            Screen::Help => {
                // Render underlying screen first, then overlay help
                self.render_underlying_screen(frame);
                screens::help::render(frame, frame.area(), theme);
            }
            Screen::Create => {
                if let Some(ref create) = self.create_state {
                    screens::create::render(create, frame, frame.area(), theme);
                } else {
                    let placeholder =
                        Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
            Screen::HookLog => {
                if let Some(ref hook_log) = self.hook_log_state {
                    screens::hook_log::render(hook_log, frame, frame.area(), theme);
                } else {
                    let placeholder =
                        Paragraph::new("trench TUI — press q to quit").alignment(Alignment::Center);
                    frame.render_widget(placeholder, frame.area());
                }
            }
        }
    }

    /// Render the screen underneath the current overlay (e.g. for Help).
    fn render_underlying_screen(&self, frame: &mut Frame) {
        let theme = &self.theme;
        let underlying = self.nav_stack.iter().rev().nth(1).copied();
        match underlying {
            Some(Screen::Detail) => {
                if let Some(ref detail) = self.detail_state {
                    screens::detail::render(detail, frame, frame.area(), theme);
                }
            }
            Some(Screen::Create) => {
                if let Some(ref create) = self.create_state {
                    screens::create::render(create, frame, frame.area(), theme);
                }
            }
            Some(Screen::SyncPicker) => {
                if let Some(ref picker) = self.sync_picker_state {
                    screens::sync_picker::render(picker, frame, frame.area(), theme);
                }
            }
            Some(Screen::DeleteConfirm) => {
                screens::list::render(&self.list_state, frame, frame.area(), theme);
                if let Some(ref confirm) = self.delete_confirm_state {
                    screens::delete_confirm::render(confirm, frame, frame.area(), theme);
                }
            }
            Some(Screen::HookLog) => {
                if let Some(ref hook_log) = self.hook_log_state {
                    screens::hook_log::render(hook_log, frame, frame.area(), theme);
                }
            }
            _ => screens::list::render(&self.list_state, frame, frame.area(), theme),
        }
    }

    pub fn pop_screen(&mut self) {
        if self.nav_stack.len() > 1 {
            self.nav_stack.pop();
            if self.active_screen() == Screen::List {
                self.refresh_list();
            }
        } else {
            self.running = false;
        }
    }

    /// Load hooks config from the project config.
    fn load_hooks_config(cwd: &std::path::Path) -> Option<crate::config::HooksConfig> {
        let repo_info = crate::git::discover_repo(cwd).ok()?;
        let project_config = crate::config::load_project_config(&repo_info.path).ok()?;
        let global_config = crate::config::load_global_config().ok()?;
        let resolved = crate::config::resolve_config(None, project_config.as_ref(), &global_config);
        resolved.hooks
    }

    fn open_db() -> Option<(std::path::PathBuf, Database)> {
        let cwd = std::env::current_dir().ok()?;
        let db_path = paths::data_dir().ok()?.join("trench.db");
        let db = Database::open(&db_path).ok()?;
        Some((cwd, db))
    }

    /// Save the current list selection to the session table (testable variant).
    pub fn save_list_session_to(&self, db: &Database) {
        let Some(ref repo_path) = self.repo_path else {
            return;
        };
        if let Some(row) = self.list_state.rows.get(self.list_state.selected) {
            let _ = db.save_list_session(repo_path, &row.name, self.list_state.selected);
        }
    }

    /// Restore list selection from the session table (testable variant).
    pub fn restore_list_session_from(&mut self, db: &Database) {
        let Some(ref repo_path) = self.repo_path else {
            return;
        };
        if let Ok(Some((name, pos))) = db.load_list_session(repo_path) {
            self.list_state.restore_selection(&name, pos);
        }
    }

    /// Save the current list selection to the session table.
    fn save_list_session(&self) {
        let Some((_, db)) = Self::open_db() else {
            return;
        };
        self.save_list_session_to(&db);
    }

    /// Restore list selection from the session table.
    fn restore_list_session(&mut self) {
        let Some((_, db)) = Self::open_db() else {
            return;
        };
        self.restore_list_session_from(&db);
    }

    /// Reload worktree data from git + DB for the list screen.
    pub fn refresh_list(&mut self) {
        let Some((cwd, db)) = Self::open_db() else {
            return;
        };
        // Discover and cache repo path for session scoping
        if self.repo_path.is_none() {
            if let Ok(repo_info) = crate::git::discover_repo(&cwd) {
                self.repo_path = Some(repo_info.path.to_string_lossy().to_string());
            }
        }
        if let Ok(rows) = screens::list::load_worktrees(&cwd, &db, &[]) {
            let prev_selected = self.list_state.selected;
            self.list_state = screens::list::ListState::new(rows);
            if self.list_state.rows.len() > prev_selected {
                self.list_state.selected = prev_selected;
            }
        }
        self.rebuild_watcher();
    }

    /// Rebuild the filesystem watcher from the current worktree list.
    ///
    /// Called after `refresh_list()` to keep watched paths in sync with
    /// the current set of worktrees.
    pub fn rebuild_watcher(&mut self) {
        if !self.auto_refresh {
            self.watcher = None;
            return;
        }

        let worktree_paths: Vec<std::path::PathBuf> = self
            .list_state
            .rows
            .iter()
            .map(|r| std::path::PathBuf::from(&r.path))
            .collect();
        let path_refs: Vec<&std::path::Path> =
            worktree_paths.iter().map(|p| p.as_path()).collect();

        let repo_path = self.repo_path.as_ref().map(std::path::PathBuf::from);
        let mut all_refs = path_refs;
        if let Some(ref rp) = repo_path {
            all_refs.push(rp.as_path());
        }

        if let Ok(dw) =
            watcher::DebouncedWatcher::from_worktree_paths(&all_refs, watcher::DEBOUNCE_DURATION)
        {
            self.watcher = Some(dw);
        }
    }

    /// Poll the filesystem watcher and refresh the list if needed.
    ///
    /// Only triggers a refresh when the active screen is List to avoid
    /// disrupting user interactions on other screens.
    pub fn check_watcher(&mut self) {
        if self.active_screen() != Screen::List {
            // Drain events but preserve pending refresh for when we return to List
            if let Some(ref mut w) = self.watcher {
                w.poll_events();
            }
            return;
        }
        if let Some(ref mut w) = self.watcher {
            if w.should_refresh() {
                self.refresh_list();
            }
        }
    }

    /// Test helper: poll watcher and return whether a refresh was signaled.
    /// Does NOT actually call refresh_list (safe for unit tests without a real repo).
    #[cfg(test)]
    pub fn check_watcher_returns_refresh(&mut self) -> bool {
        if self.active_screen() != Screen::List {
            if let Some(ref mut w) = self.watcher {
                w.poll_events();
            }
            return false;
        }
        if let Some(ref mut w) = self.watcher {
            return w.should_refresh();
        }
        false
    }

    /// Set up the hook log screen with a receiver for live streaming.
    pub fn start_hook_log(
        &mut self,
        title: &str,
        rx: std::sync::mpsc::Receiver<screens::hook_log::HookOutputMessage>,
    ) {
        self.hook_log_state = Some(screens::hook_log::HookLogState::new(title));
        self.hook_rx = Some(rx);
        self.push_screen(Screen::HookLog);
    }

    /// Drain pending messages from the hook output channel and update state.
    /// Continues draining even after dismiss (hook_log_state is None) so that
    /// HookCompleted triggers a final refresh_list.
    pub fn process_hook_messages(&mut self) {
        let Some(ref rx) = self.hook_rx else { return };
        let mut received = false;
        let mut completed = false;
        while let Ok(msg) = rx.try_recv() {
            if matches!(
                &msg,
                screens::hook_log::HookOutputMessage::HookCompleted { .. }
            ) {
                completed = true;
            }
            if let Some(ref mut state) = self.hook_log_state {
                state.process_message(msg);
                received = true;
            }
        }
        if received {
            if let Some(ref mut state) = self.hook_log_state {
                state.auto_scroll(state.last_body_height.get());
            }
        }
        if completed && self.hook_log_state.is_none() {
            // Hook finished after user dismissed — refresh list to reflect changes
            self.hook_rx = None;
            self.refresh_list();
        }
    }

    fn is_create_branch_text_entry_active(&self) -> bool {
        self.active_screen() == Screen::Create
            && self.create_state.as_ref().is_some_and(|s| {
                !s.is_result_mode() && s.focused_field == screens::create::CreateField::Branch
            })
    }

    fn clear_active_screen_state(&mut self) {
        match self.active_screen() {
            Screen::DeleteConfirm => self.delete_confirm_state = None,
            Screen::SyncPicker => self.sync_picker_state = None,
            Screen::Create => self.create_state = None,
            Screen::HookLog => {
                self.hook_log_state = None;
                // Keep hook_rx alive for post-dismiss draining
            }
            _ => {}
        }
    }

    /// Dismiss or pop the hook log screen. Replay mode simply pops back
    /// to the previous screen; live mode unwinds to List (clearing dialog
    /// state and keeping hook_rx alive for draining).
    fn dismiss_or_pop_hook_log(&mut self) {
        let is_replay = self
            .hook_log_state
            .as_ref()
            .is_some_and(|s| s.replay);
        if is_replay {
            self.hook_log_state = None;
            self.pop_screen();
        } else {
            self.dismiss_hook_log();
        }
    }

    /// Dismiss the hook log screen and unwind to List, clearing any
    /// intermediate source dialog state (Create/Sync/Delete) so the
    /// underlying operation cannot be re-triggered.
    fn dismiss_hook_log(&mut self) {
        self.hook_log_state = None;
        // Keep hook_rx alive — process_hook_messages will drain it and
        // call refresh_list when HookCompleted arrives after dismiss.
        self.create_state = None;
        self.sync_picker_state = None;
        self.delete_confirm_state = None;
        self.nav_stack.retain(|s| *s == Screen::List);
        if self.nav_stack.is_empty() {
            self.nav_stack.push(Screen::List);
        }
        self.refresh_list();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // Global keys handled at app level
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.running = false,
            (KeyCode::Char('?'), _) => {
                if self.active_screen() == Screen::Help {
                    self.pop_screen();
                } else {
                    self.push_screen(Screen::Help);
                }
            }
            (KeyCode::Esc, _) => {
                if self.active_screen() == Screen::HookLog {
                    self.dismiss_or_pop_hook_log();
                } else {
                    self.clear_active_screen_state();
                    self.pop_screen();
                }
            }
            (KeyCode::Char('q'), _) if !self.is_create_branch_text_entry_active() => {
                if self.active_screen() == Screen::HookLog {
                    self.dismiss_or_pop_hook_log();
                } else {
                    self.clear_active_screen_state();
                    self.pop_screen();
                }
            }
            _ => self.handle_screen_key(key),
        }
    }

    fn handle_screen_key(&mut self, key: KeyEvent) {
        match self.active_screen() {
            Screen::List => self.handle_list_key(key),
            Screen::Detail => self.handle_detail_key(key),
            Screen::SyncPicker => self.handle_sync_picker_key(key),
            Screen::DeleteConfirm => self.handle_delete_confirm_key(key),
            Screen::Create => self.handle_create_key(key),
            Screen::HookLog => self.handle_hook_log_key(key),
            Screen::Help => {}
        }
    }

    fn handle_delete_confirm_key(&mut self, key: KeyEvent) {
        let in_result_mode = self
            .delete_confirm_state
            .as_ref()
            .is_some_and(|s| s.is_result_mode());

        if in_result_mode {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.delete_confirm_state = None;
                    while self.active_screen() != Screen::List {
                        self.pop_screen();
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => self.execute_delete(),
            KeyCode::Char('n') => {
                self.delete_confirm_state = None;
                self.pop_screen();
            }
            _ => {}
        }
    }

    fn execute_delete(&mut self) {
        let confirm = match self.delete_confirm_state.as_ref() {
            Some(c) => c,
            None => return,
        };
        let worktree_name = confirm.worktree_name.clone();

        let Some((cwd, db)) = Self::open_db() else {
            if let Some(ref mut c) = self.delete_confirm_state {
                c.result = Some(screens::delete_confirm::DeleteResultMessage {
                    success: false,
                    message: "Failed to open database".into(),
                });
            }
            return;
        };

        // Check for hooks
        let hooks_config = Self::load_hooks_config(&cwd);
        let has_hooks = hooks_config
            .as_ref()
            .map(|h| h.pre_remove.is_some() || h.post_remove.is_some())
            .unwrap_or(false);

        if has_hooks {
            // Resolve repo + worktree for background hook execution
            let repo_info = match crate::git::discover_repo(&cwd) {
                Ok(r) => r,
                Err(e) => {
                    if let Some(ref mut c) = self.delete_confirm_state {
                        c.result = Some(screens::delete_confirm::DeleteResultMessage {
                            success: false,
                            message: format!("Delete failed: {e:#}"),
                        });
                    }
                    return;
                }
            };
            let resolve = crate::adopt::resolve_or_adopt(&worktree_name, &repo_info, &db);
            let (repo, wt) = match resolve {
                Ok((r, w)) => (r, w),
                Err(e) => {
                    if let Some(ref mut c) = self.delete_confirm_state {
                        c.result = Some(screens::delete_confirm::DeleteResultMessage {
                            success: false,
                            message: format!("Delete failed: {e:#}"),
                        });
                    }
                    return;
                }
            };

            let (tx, rx) = std::sync::mpsc::channel();
            let hooks = hooks_config.unwrap();
            std::thread::spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(screens::hook_log::HookOutputMessage::HookCompleted {
                            success: false,
                            duration: std::time::Duration::ZERO,
                            error: Some(format!("Failed to start hook runtime: {e}")),
                        });
                        return;
                    }
                };
                let result =
                    rt.block_on(crate::cli::commands::remove::execute_resolved_with_hooks(
                        &repo,
                        &wt,
                        &repo_info,
                        &db,
                        false,
                        Some(&hooks),
                        false,
                        Some(&tx),
                    ));
                let (success, error) = match result {
                    Ok(_) => (true, None),
                    Err(ref e) => (false, Some(format!("{e:#}"))),
                };
                let _ = tx.send(screens::hook_log::HookOutputMessage::HookCompleted {
                    success,
                    duration: std::time::Duration::ZERO,
                    error,
                });
            });
            self.start_hook_log("remove hooks", rx);
        } else {
            match crate::cli::commands::remove::execute(&worktree_name, &cwd, &db, false) {
                Ok(result) => {
                    let msg = format!("Removed '{}'", result.name);
                    if let Some(ref mut c) = self.delete_confirm_state {
                        c.result = Some(screens::delete_confirm::DeleteResultMessage {
                            success: true,
                            message: msg,
                        });
                    }
                }
                Err(e) => {
                    if let Some(ref mut c) = self.delete_confirm_state {
                        c.result = Some(screens::delete_confirm::DeleteResultMessage {
                            success: false,
                            message: format!("Delete failed: {e:#}"),
                        });
                    }
                }
            }
        }
    }

    /// If the selected worktree is unmanaged, silently adopt it into the DB.
    fn adopt_selected_if_unmanaged(&mut self) {
        let row = match self.list_state.rows.get(self.list_state.selected) {
            Some(r) if !r.managed => r,
            _ => return,
        };
        let identifier = if row.branch == "(detached)" {
            row.name.clone()
        } else {
            row.branch.clone()
        };

        let Some((cwd, db)) = Self::open_db() else {
            return;
        };
        let repo_info = match crate::git::discover_repo(&cwd) {
            Ok(r) => r,
            Err(_) => return,
        };
        let _ = crate::adopt::resolve_or_adopt(&identifier, &repo_info, &db);
        self.refresh_list();
    }

    fn load_detail(&mut self, name: &str) -> bool {
        self.detail_state = None;
        let Some((cwd, db)) = Self::open_db() else {
            return false;
        };
        self.detail_state = Some(screens::detail::load_detail(name, &cwd, &db));
        true
    }

    fn handle_detail_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('s') => {
                if let Some(ref detail) = self.detail_state {
                    self.sync_picker_state =
                        Some(screens::sync_picker::SyncPickerState::new(&detail.name));
                    self.push_screen(Screen::SyncPicker);
                }
            }
            KeyCode::Char('o') => {
                if let Some(ref detail) = self.detail_state {
                    self.editor_request = Some(detail.path.clone());
                }
            }
            KeyCode::Char('l') => {
                if let Some(ref detail) = self.detail_state {
                    let name = detail.name.clone();
                    self.load_hook_log_replay(&name);
                }
            }
            _ => {}
        }
    }

    /// Load hook log replay from DB for the given worktree and push HookLog screen.
    ///
    /// Returns `true` if the hook log was loaded, `false` if no hook history exists
    /// or the DB is unavailable.
    pub fn load_hook_log_replay(&mut self, worktree_name: &str) -> bool {
        let Some((cwd, db)) = Self::open_db() else {
            return false;
        };
        let repo_info = match crate::git::discover_repo(&cwd) {
            Ok(r) => r,
            Err(_) => return false,
        };
        let repo = match db.get_repo_by_path(&repo_info.path.to_string_lossy()) {
            Ok(Some(r)) => r,
            _ => return false,
        };
        let event = match db.get_last_hook_event_for_worktree(repo.id, worktree_name) {
            Ok(Some(e)) => e,
            Ok(None) => {
                // No hook history — show empty state
                self.hook_log_state = Some(screens::hook_log::HookLogState::no_history());
                self.push_screen(Screen::HookLog);
                return true;
            }
            Err(_) => return false,
        };
        let lines = match db.get_hook_output(event.id) {
            Ok(l) => l,
            Err(_) => return false,
        };

        let state = screens::hook_log::HookLogState::from_hook_output(
            &lines,
            &event.event_type,
            &event.payload,
        );
        self.hook_log_state = Some(state);
        self.push_screen(Screen::HookLog);
        true
    }

    fn handle_hook_log_key(&mut self, key: KeyEvent) {
        if let Some(ref mut state) = self.hook_log_state {
            let h = state.last_body_height.get();
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => state.scroll_down(h),
                KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                KeyCode::PageDown => state.page_down(h),
                KeyCode::PageUp => state.page_up(h),
                _ => {}
            }
        }
    }

    fn handle_sync_picker_key(&mut self, key: KeyEvent) {
        let in_result_mode = self
            .sync_picker_state
            .as_ref()
            .is_some_and(|p| p.is_result_mode());
        if in_result_mode {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    while self.active_screen() != Screen::List {
                        self.pop_screen();
                    }
                    self.sync_picker_state = None;
                }
                _ => {}
            }
            return;
        }

        if let Some(ref mut picker) = self.sync_picker_state {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => picker.select_next(),
                KeyCode::Up | KeyCode::Char('k') => picker.select_previous(),
                KeyCode::Enter => self.execute_sync(),
                _ => {}
            }
        }
    }

    fn execute_sync(&mut self) {
        let picker = match self.sync_picker_state.as_ref() {
            Some(p) => p,
            None => return,
        };
        let strategy = picker.confirmed_strategy();
        let worktree_name = picker.worktree_name.clone();

        let Some((cwd, db)) = Self::open_db() else {
            if let Some(ref mut p) = self.sync_picker_state {
                p.result = Some(screens::sync_picker::SyncResultMessage {
                    success: false,
                    message: "Failed to open database".into(),
                });
            }
            return;
        };

        // Check for hooks
        let hooks_config = Self::load_hooks_config(&cwd);
        let has_hooks = hooks_config
            .as_ref()
            .map(|h| h.pre_sync.is_some() || h.post_sync.is_some())
            .unwrap_or(false);

        if has_hooks {
            let (tx, rx) = std::sync::mpsc::channel();
            let hooks = hooks_config.unwrap();
            std::thread::spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(screens::hook_log::HookOutputMessage::HookCompleted {
                            success: false,
                            duration: std::time::Duration::ZERO,
                            error: Some(format!("Failed to start hook runtime: {e}")),
                        });
                        return;
                    }
                };
                let result = rt.block_on(crate::cli::commands::sync::execute_with_hooks(
                    &worktree_name,
                    &cwd,
                    &db,
                    strategy,
                    Some(&hooks),
                    false,
                    Some(&tx),
                ));
                let (success, error) = match result {
                    Ok(_) => (true, None),
                    Err(ref e) => (false, Some(format!("{e:#}"))),
                };
                let _ = tx.send(screens::hook_log::HookOutputMessage::HookCompleted {
                    success,
                    duration: std::time::Duration::ZERO,
                    error,
                });
            });
            self.start_hook_log("sync hooks", rx);
        } else {
            match crate::cli::commands::sync::execute(&worktree_name, &cwd, &db, strategy) {
                Ok(result) => {
                    let msg = format!(
                        "Synced '{}' via {}\nBefore: +{}/-{}  After: +{}/-{}",
                        result.name,
                        result.strategy,
                        result.before_ahead,
                        result.before_behind,
                        result.after_ahead,
                        result.after_behind,
                    );
                    if let Some(ref mut p) = self.sync_picker_state {
                        p.result = Some(screens::sync_picker::SyncResultMessage {
                            success: true,
                            message: msg,
                        });
                    }
                }
                Err(e) => {
                    if let Some(ref mut p) = self.sync_picker_state {
                        p.result = Some(screens::sync_picker::SyncResultMessage {
                            success: false,
                            message: format!("Sync failed: {e:#}"),
                        });
                    }
                }
            }
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let identity = self
                    .list_state
                    .rows
                    .get(self.list_state.selected)
                    .map(|r| r.name.clone());
                self.adopt_selected_if_unmanaged();
                if let Some(ref name) = identity {
                    if let Some(idx) = self.list_state.rows.iter().position(|r| r.name == *name) {
                        self.list_state.selected = idx;
                    }
                }
                // Load detail data for the selected worktree
                if let Some(name) = identity {
                    self.save_list_session();
                    if self.load_detail(&name) {
                        self.push_screen(Screen::Detail);
                    }
                }
            }
            KeyCode::Char('n') => {
                self.init_create_form();
                self.push_screen(Screen::Create);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let prev = self.list_state.selected;
                self.list_state.select_next();
                if self.list_state.selected != prev {
                    self.save_list_session();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let prev = self.list_state.selected;
                self.list_state.select_previous();
                if self.list_state.selected != prev {
                    self.save_list_session();
                }
            }
            KeyCode::Char('s') => {
                if let Some(row) = self.list_state.rows.get(self.list_state.selected) {
                    self.sync_picker_state =
                        Some(screens::sync_picker::SyncPickerState::new(&row.name));
                    self.push_screen(Screen::SyncPicker);
                }
            }
            KeyCode::Char('D') => {
                if let Some(row) = self.list_state.rows.get(self.list_state.selected) {
                    self.delete_confirm_state =
                        Some(screens::delete_confirm::DeleteConfirmState::new(
                            &row.name,
                            &row.path,
                            &row.branch,
                        ));
                    self.push_screen(Screen::DeleteConfirm);
                }
            }
            KeyCode::Char('l') => {
                if let Some(row) = self.list_state.rows.get(self.list_state.selected) {
                    let name = row.name.clone();
                    self.load_hook_log_replay(&name);
                }
            }
            KeyCode::Char('d') => {
                let identity = self
                    .list_state
                    .rows
                    .get(self.list_state.selected)
                    .map(|r| r.name.clone());
                self.adopt_selected_if_unmanaged();
                if let Some(ref name) = identity {
                    if let Some(idx) = self.list_state.rows.iter().position(|r| r.name == *name) {
                        self.list_state.selected = idx;
                    }
                }
                if let Some(name) = identity {
                    self.save_list_session();
                    if self.load_detail(&name) {
                        self.push_screen(Screen::Detail);
                    }
                }
            }
            _ => {}
        }
    }

    /// Initialize the create form state from the current repo context.
    fn init_create_form(&mut self) {
        let mut base_branches = vec!["main".to_string()];
        let mut repo_name = String::new();
        let template = paths::DEFAULT_WORKTREE_TEMPLATE.to_string();

        if let Some((cwd, _db)) = Self::open_db() {
            if let Ok(repo_info) = crate::git::discover_repo(&cwd) {
                repo_name = repo_info.name.clone();
                // Collect local branches for the base selector
                base_branches =
                    crate::git::list_local_branches(&repo_info.path).unwrap_or_default();
                // Ensure the repo default exists and is first
                if let Some(pos) = base_branches
                    .iter()
                    .position(|b| b == &repo_info.default_branch)
                {
                    if pos != 0 {
                        let default = base_branches.remove(pos);
                        base_branches.insert(0, default);
                    }
                } else {
                    base_branches.insert(0, repo_info.default_branch.clone());
                }
            }
        }
        if base_branches.is_empty() {
            base_branches.push("main".to_string());
        }

        self.create_state = Some(screens::create::CreateState::new(
            base_branches,
            repo_name,
            template,
        ));
    }

    fn handle_create_key(&mut self, key: KeyEvent) {
        use screens::create::CreateField;

        let in_result_mode = self
            .create_state
            .as_ref()
            .is_some_and(|s| s.is_result_mode());

        if in_result_mode {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.create_state = None;
                    while self.active_screen() != Screen::List {
                        self.pop_screen();
                    }
                }
                _ => {}
            }
            return;
        }

        let Some(ref mut state) = self.create_state else {
            return;
        };

        match state.focused_field {
            CreateField::Branch => match key.code {
                KeyCode::Char(c) => {
                    state.insert_char(c);
                    state.update_path_preview();
                    let _ = state.validate();
                }
                KeyCode::Backspace => {
                    state.backspace();
                    state.update_path_preview();
                    let _ = state.validate();
                }
                KeyCode::Left => state.cursor_left(),
                KeyCode::Right => state.cursor_right(),
                KeyCode::Tab => state.focus_next(),
                KeyCode::BackTab => state.focus_previous(),
                KeyCode::Enter => state.focus_next(),
                _ => {}
            },
            CreateField::Base => match key.code {
                KeyCode::Left | KeyCode::Char('h') => state.select_previous_base(),
                KeyCode::Right | KeyCode::Char('l') => state.select_next_base(),
                KeyCode::Tab => state.focus_next(),
                KeyCode::BackTab => state.focus_previous(),
                KeyCode::Enter => state.focus_next(),
                _ => {}
            },
            CreateField::Hooks => match key.code {
                KeyCode::Char(' ') => state.toggle_hooks(),
                KeyCode::Tab => state.focus_next(),
                KeyCode::BackTab => state.focus_previous(),
                KeyCode::Enter => {
                    // On last field, Enter triggers create
                    self.execute_create();
                }
                _ => {}
            },
        }
    }

    fn execute_create(&mut self) {
        let state = match self.create_state.as_mut() {
            Some(s) => s,
            None => return,
        };

        // Validate first
        if state.validate().is_err() {
            return;
        }

        let branch = state.branch_input.clone();
        let base = state.selected_base_branch().map(|s| s.to_string());
        let hooks_enabled = state.hooks_enabled;

        let Some((cwd, db)) = Self::open_db() else {
            state.result = Some(screens::create::CreateResultMessage {
                success: false,
                message: "Failed to open database".into(),
            });
            return;
        };

        let worktree_root = match paths::worktree_root() {
            Ok(r) => r,
            Err(e) => {
                state.result = Some(screens::create::CreateResultMessage {
                    success: false,
                    message: format!("Failed to resolve worktree root: {e:#}"),
                });
                return;
            }
        };

        // Load config to check for hooks
        let hooks_config = if hooks_enabled {
            Self::load_hooks_config(&cwd)
        } else {
            None
        };

        let has_hooks = hooks_config
            .as_ref()
            .map(|h| h.pre_create.is_some() || h.post_create.is_some())
            .unwrap_or(false);

        let template = state.worktree_template.clone();

        if has_hooks {
            // Background execution with live streaming
            let (tx, rx) = std::sync::mpsc::channel();
            let hooks = hooks_config.unwrap();
            let branch_clone = branch.clone();
            let base_clone = base.clone();

            std::thread::spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(screens::hook_log::HookOutputMessage::HookCompleted {
                            success: false,
                            duration: std::time::Duration::ZERO,
                            error: Some(format!("Failed to start hook runtime: {e}")),
                        });
                        return;
                    }
                };
                let result = rt.block_on(crate::cli::commands::create::execute_with_hooks(
                    &branch_clone,
                    base_clone.as_deref(),
                    &cwd,
                    &worktree_root,
                    &template,
                    &db,
                    Some(&hooks),
                    false,
                    Some(&tx),
                ));
                let (success, error) = match result {
                    Ok(_) => (true, None),
                    Err(ref e) => (false, Some(format!("{e:#}"))),
                };
                let _ = tx.send(screens::hook_log::HookOutputMessage::HookCompleted {
                    success,
                    duration: std::time::Duration::ZERO,
                    error,
                });
            });

            self.start_hook_log("create hooks", rx);
        } else {
            // Synchronous path without hooks
            match crate::cli::commands::create::execute(
                &branch,
                base.as_deref(),
                &cwd,
                &worktree_root,
                &template,
                &db,
            ) {
                Ok(result) => {
                    let msg = format!("Created '{}' at {}", result.name, result.path.display());
                    if let Some(ref mut s) = self.create_state {
                        s.result = Some(screens::create::CreateResultMessage {
                            success: true,
                            message: msg,
                        });
                    }
                }
                Err(e) => {
                    if let Some(ref mut s) = self.create_state {
                        s.result = Some(screens::create::CreateResultMessage {
                            success: false,
                            message: format!("Create failed: {e:#}"),
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serial_test::serial;

    #[test]
    fn app_has_repo_path_initially_none() {
        let app = App::new();
        assert!(app.repo_path.is_none());
    }

    #[test]
    fn app_has_switch_path_initially_none() {
        let app = App::new();
        assert!(app.switch_path.is_none());
    }

    #[test]
    fn save_list_session_to_db_persists_selection() {
        let db = crate::state::Database::open_in_memory().unwrap();
        let mut app = app_with_rows();
        app.repo_path = Some("/repos/test".into());
        app.list_state.selected = 1; // "feat-b"

        app.save_list_session_to(&db);

        let session = db.load_list_session("/repos/test").unwrap();
        assert!(session.is_some());
        let (name, pos) = session.unwrap();
        assert_eq!(name, "feat-b");
        assert_eq!(pos, 1);
    }

    #[test]
    fn save_list_session_to_db_noop_without_repo_path() {
        let db = crate::state::Database::open_in_memory().unwrap();
        let mut app = app_with_rows();
        app.list_state.selected = 2;
        // repo_path is None — should not save

        app.save_list_session_to(&db);

        let session = db.load_list_session("").unwrap();
        assert!(session.is_none(), "should not save without repo_path");
    }

    #[test]
    fn restore_list_session_from_db_restores_selection() {
        let db = crate::state::Database::open_in_memory().unwrap();
        db.save_list_session("/repos/test", "feat-b", 1).unwrap();

        let mut app = app_with_rows();
        app.repo_path = Some("/repos/test".into());
        app.restore_list_session_from(&db);

        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn restore_list_session_from_db_handles_stale_worktree() {
        let db = crate::state::Database::open_in_memory().unwrap();
        db.save_list_session("/repos/test", "deleted-worktree", 99).unwrap();

        let mut app = app_with_rows();
        app.repo_path = Some("/repos/test".into());
        app.restore_list_session_from(&db);

        assert_eq!(app.list_state.selected, 0, "should fall back to 0 for stale state");
    }

    #[test]
    fn restore_list_session_noop_without_repo_path() {
        let db = crate::state::Database::open_in_memory().unwrap();
        db.save_list_session("/repos/test", "feat-b", 1).unwrap();

        let mut app = app_with_rows();
        // repo_path is None
        app.restore_list_session_from(&db);

        assert_eq!(app.list_state.selected, 0, "should not restore without repo_path");
    }

    #[test]
    fn restore_list_session_noop_with_no_saved_session() {
        let db = crate::state::Database::open_in_memory().unwrap();

        let mut app = app_with_rows();
        app.repo_path = Some("/repos/test".into());
        app.restore_list_session_from(&db);

        assert_eq!(app.list_state.selected, 0, "should stay at 0 with no saved session");
    }

    #[test]
    fn session_full_round_trip_save_restart_restore() {
        let db = crate::state::Database::open_in_memory().unwrap();
        let repo_path = "/repos/round-trip";

        // Simulate first TUI session: user navigates to "feat-b" (index 1)
        let mut app1 = app_with_rows();
        app1.repo_path = Some(repo_path.into());
        app1.list_state.selected = 1;
        app1.save_list_session_to(&db);

        // Simulate TUI restart: new App, same rows, restore session
        let mut app2 = app_with_rows();
        app2.repo_path = Some(repo_path.into());
        assert_eq!(app2.list_state.selected, 0, "new app starts at 0");
        app2.restore_list_session_from(&db);
        assert_eq!(app2.list_state.selected, 1, "should restore to feat-b");
    }

    #[test]
    fn session_round_trip_with_stale_worktree() {
        let db = crate::state::Database::open_in_memory().unwrap();
        let repo_path = "/repos/stale";

        // First session: user selects "feat-b" (index 1)
        let mut app1 = app_with_rows();
        app1.repo_path = Some(repo_path.into());
        app1.list_state.selected = 1;
        app1.save_list_session_to(&db);

        // Restart with different rows — "feat-b" was removed
        let mut app2 = App::new();
        app2.list_state = screens::list::ListState::new(vec![
            screens::list::WorktreeRow {
                name: "feat-a".into(),
                branch: "feat/a".into(),
                path: "/tmp/wt/feat-a".into(),
                status: "clean".into(),
                ahead_behind: "+0/-0".into(),
                managed: true,
                processes: String::new(),
            },
            screens::list::WorktreeRow {
                name: "feat-c".into(),
                branch: "feat/c".into(),
                path: "/tmp/wt/feat-c".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: true,
                processes: String::new(),
            },
        ]);
        app2.repo_path = Some(repo_path.into());
        app2.restore_list_session_from(&db);

        // "feat-b" is gone. scroll_position (1) is still valid,
        // so it falls back to index 1
        assert_eq!(app2.list_state.selected, 1,
            "should fall back to scroll position when worktree name not found");
    }

    #[test]
    fn session_per_repo_isolation() {
        let db = crate::state::Database::open_in_memory().unwrap();

        // Save session for repo A
        let mut app_a = app_with_rows();
        app_a.repo_path = Some("/repos/alpha".into());
        app_a.list_state.selected = 2; // "main"
        app_a.save_list_session_to(&db);

        // Save session for repo B
        let mut app_b = app_with_rows();
        app_b.repo_path = Some("/repos/beta".into());
        app_b.list_state.selected = 0; // "feat-a"
        app_b.save_list_session_to(&db);

        // Restore repo A — should get index 2
        let mut restore_a = app_with_rows();
        restore_a.repo_path = Some("/repos/alpha".into());
        restore_a.restore_list_session_from(&db);
        assert_eq!(restore_a.list_state.selected, 2);

        // Restore repo B — should get index 0
        let mut restore_b = app_with_rows();
        restore_b.repo_path = Some("/repos/beta".into());
        restore_b.restore_list_session_from(&db);
        assert_eq!(restore_b.list_state.selected, 0);
    }

    #[test]
    fn app_starts_in_running_state() {
        let app = App::new();
        assert!(app.is_running(), "newly created app should be running");
    }

    #[test]
    fn app_starts_on_list_screen() {
        let app = App::new();
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "app should start on the List screen"
        );
    }

    #[test]
    fn screen_enum_has_seven_variants() {
        let screens = [
            Screen::List,
            Screen::Detail,
            Screen::Create,
            Screen::Help,
            Screen::SyncPicker,
            Screen::DeleteConfirm,
            Screen::HookLog,
        ];
        for (i, a) in screens.iter().enumerate() {
            for (j, b) in screens.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn hook_log_screen_can_be_pushed_and_popped() {
        let mut app = App::new();
        app.hook_log_state = Some(screens::hook_log::HookLogState::new("post_create"));
        app.push_screen(Screen::HookLog);
        assert_eq!(app.active_screen(), Screen::HookLog);
        assert_eq!(app.nav_stack_depth(), 2);

        // Esc pops back
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
    }

    #[test]
    fn question_mark_pushes_help_from_any_screen() {
        let mut app = App::new();
        assert_eq!(app.active_screen(), Screen::List);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Help,
            "? should push Help screen"
        );
        assert_eq!(app.nav_stack_depth(), 2, "stack should have List + Help");
    }

    #[test]
    fn question_mark_toggles_help_closed_when_already_on_help() {
        let mut app = App::new();
        // Open help
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 2);

        // Press ? again — should close help (toggle)
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "? while on Help should pop back to previous screen"
        );
        assert_eq!(app.nav_stack_depth(), 1);
    }

    #[test]
    fn question_mark_toggles_help_from_detail_screen() {
        let mut app = app_with_rows();
        // Navigate to detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);

        // Open help from detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 3);

        // Close help — should return to detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Detail,
            "? toggle should return to Detail, not List"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn esc_pops_back_to_previous_screen() {
        let mut app = App::new();
        // Push Help from List
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 2);

        // Esc should pop back to List
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Esc should pop back to List"
        );
        assert_eq!(app.nav_stack_depth(), 1);
        assert!(
            app.is_running(),
            "app should still be running after popping"
        );
    }

    #[test]
    fn esc_on_root_screen_quits_app() {
        let mut app = App::new();
        assert_eq!(app.active_screen(), Screen::List);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(
            !app.is_running(),
            "Esc on root screen (List) should quit the app"
        );
    }

    #[test]
    fn q_on_root_screen_quits_app() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.is_running(), "q on root screen should quit the app");
    }

    #[test]
    fn quit_leaves_switch_path_none() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.is_running());
        assert!(app.switch_path.is_none(), "quit should not set switch_path");
    }

    #[test]
    fn q_on_non_root_screen_pops_back() {
        let mut app = App::new();
        // Push Help
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);

        // q should pop back, not quit
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "q on non-root should pop back to List"
        );
        assert!(app.is_running(), "q on non-root should not quit the app");
    }

    #[test]
    fn app_exits_on_ctrl_c() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.is_running(), "app should stop after Ctrl+C");
    }

    #[test]
    fn d_on_list_with_rows_pushes_detail() {
        let mut app = app_with_rows();
        assert_eq!(app.active_screen(), Screen::List);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Detail,
            "d on List with rows should push Detail screen"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn n_on_list_pushes_create() {
        let mut app = App::new();
        assert_eq!(app.active_screen(), Screen::List);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Create,
            "n on List should push Create screen"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn enter_on_non_list_screen_does_nothing() {
        let mut app = App::new();
        // Push Help
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);

        // Enter on Help should not push anything
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Help,
            "Enter on Help should do nothing"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn deep_stack_navigation_push_pop_sequence() {
        let mut app = app_with_rows();
        // List → Detail → Help → pop → pop → List
        app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);
        assert_eq!(app.nav_stack_depth(), 2);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 3);

        // Pop Help → Detail
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);
        assert_eq!(app.nav_stack_depth(), 2);

        // Pop Detail → List
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert_eq!(app.nav_stack_depth(), 1);
        assert!(app.is_running());

        // Pop List → quit
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.is_running());
    }

    #[test]
    fn question_mark_opens_help_from_detail_screen() {
        let mut app = app_with_rows();
        // Navigate to Detail first
        app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);

        // ? should still open Help from Detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Help);
        assert_eq!(app.nav_stack_depth(), 3);
    }

    fn app_with_rows() -> App {
        use screens::list::WorktreeRow;
        let mut app = App::new();
        app.list_state = screens::list::ListState::new(vec![
            WorktreeRow {
                name: "feat-a".into(),
                branch: "feat/a".into(),
                path: "/tmp/wt/feat-a".into(),
                status: "clean".into(),
                ahead_behind: "+0/-0".into(),
                managed: true,
                processes: String::new(),
            },
            WorktreeRow {
                name: "feat-b".into(),
                branch: "feat/b".into(),
                path: "/tmp/wt/feat-b".into(),
                status: "~2".into(),
                ahead_behind: "+1/-0".into(),
                managed: true,
                processes: String::new(),
            },
            WorktreeRow {
                name: "main".into(),
                branch: "main".into(),
                path: "/tmp/wt/main".into(),
                status: "clean".into(),
                ahead_behind: "-".into(),
                managed: false,
                processes: String::new(),
            },
        ]);
        app
    }

    #[test]
    fn j_key_moves_selection_down() {
        let mut app = app_with_rows();
        assert_eq!(app.list_state.selected, 0);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn k_key_moves_selection_up() {
        let mut app = app_with_rows();
        app.list_state.selected = 2;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn arrow_down_moves_selection_down() {
        let mut app = app_with_rows();
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 1);
    }

    #[test]
    fn arrow_up_moves_selection_up() {
        let mut app = app_with_rows();
        app.list_state.selected = 1;
        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.list_state.selected, 0);
    }

    #[test]
    fn s_on_list_pushes_sync_picker() {
        let mut app = app_with_rows();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(app.is_running());
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn s_on_list_sets_sync_picker_state_with_selected_worktree() {
        let mut app = app_with_rows();
        // Select second row
        app.list_state.selected = 1;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        let state = app
            .sync_picker_state
            .as_ref()
            .expect("sync_picker_state should be set");
        assert_eq!(state.worktree_name, "feat-b");
    }

    #[test]
    fn d_on_list_pushes_delete_confirm() {
        let mut app = app_with_rows();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert!(app.is_running());
        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn d_on_list_sets_delete_confirm_state_with_selected_worktree() {
        let mut app = app_with_rows();
        app.list_state.selected = 1; // select feat-b
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        let state = app
            .delete_confirm_state
            .as_ref()
            .expect("delete_confirm_state should be set");
        assert_eq!(state.worktree_name, "feat-b");
        assert_eq!(state.worktree_path, "/tmp/wt/feat-b");
        assert_eq!(state.branch, "feat/b");
    }

    #[test]
    fn d_on_empty_list_does_not_push_delete_confirm() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.delete_confirm_state.is_none());
    }

    #[test]
    fn enter_on_empty_list_does_not_push_detail() {
        let mut app = App::new();
        // Empty list — no rows
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Enter on empty list should stay on List"
        );
        assert!(app.detail_state.is_none());
    }

    #[test]
    fn app_ignores_unbound_keys() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.is_running(), "unbound keys should not stop the app");
    }

    #[test]
    #[serial]
    fn restore_panic_hook_restores_prior_hook() {
        use std::panic::{self, catch_unwind};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        // Install marker as the pre-existing hook BEFORE TUI touches anything
        let marker_ran = Arc::new(AtomicBool::new(false));
        let marker = marker_ran.clone();
        panic::set_hook(Box::new(move |_| {
            marker.store(true, Ordering::SeqCst);
        }));

        install_panic_hook(); // wraps marker in TUI hook
        restore_panic_hook(); // must restore marker, not default

        let _ = catch_unwind(|| panic!("test panic"));

        // Clean up so we don't affect other tests
        let _ = panic::take_hook();

        assert!(
            marker_ran.load(Ordering::SeqCst),
            "prior hook should run after restore, proving TUI hook was removed and original restored"
        );
    }

    #[test]
    fn list_screen_renders_empty_state_by_default() {
        let app = App::new();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("No worktrees"),
            "empty list should show 'No worktrees' message, got: {:?}",
            content.trim()
        );
    }

    #[test]
    fn create_screen_renders_placeholder() {
        let mut app = App::new();
        app.push_screen(Screen::Create);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("trench TUI"),
            "Create screen should show placeholder, got: {:?}",
            content.trim()
        );
    }

    fn app_with_create_state() -> App {
        let mut app = App::new();
        app.create_state = Some(screens::create::CreateState::new(
            vec!["main".into(), "develop".into()],
            "my-project".into(),
            "{{ repo }}/{{ branch | sanitize }}".into(),
        ));
        app.push_screen(Screen::Create);
        app
    }

    #[test]
    fn create_screen_renders_form_when_state_present() {
        let app = app_with_create_state();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("Create Worktree"),
            "Create screen with state should show form title, got: {:?}",
            content.trim()
        );
    }

    #[test]
    fn esc_on_create_pops_to_list_and_clears_state() {
        let mut app = app_with_create_state();
        assert_eq!(app.active_screen(), Screen::Create);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(
            app.create_state.is_none(),
            "create_state should be cleared on Esc"
        );
    }

    #[test]
    fn q_on_create_pops_when_not_in_branch_field() {
        let mut app = app_with_create_state();
        // Move focus to Base field so q acts as cancel
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Base
        );
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.create_state.is_none());
    }

    #[test]
    fn typing_on_create_updates_branch_input() {
        let mut app = app_with_create_state();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        let state = app.create_state.as_ref().unwrap();
        assert_eq!(state.branch_input, "foo");
    }

    #[test]
    fn typing_on_create_updates_path_preview() {
        let mut app = app_with_create_state();
        for c in "feature/auth".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let state = app.create_state.as_ref().unwrap();
        assert_eq!(state.path_preview, "my-project/feature-auth");
    }

    #[test]
    fn tab_on_create_moves_to_next_field() {
        let mut app = app_with_create_state();
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Branch
        );
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Base
        );
    }

    #[test]
    fn backtab_on_create_moves_to_previous_field() {
        let mut app = app_with_create_state();
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Hooks
        );
    }

    #[test]
    fn left_right_on_base_field_cycles_base_branches() {
        let mut app = app_with_create_state();
        // Move to Base field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Base
        );
        assert_eq!(app.create_state.as_ref().unwrap().selected_base, 0);
        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.create_state.as_ref().unwrap().selected_base, 1);
        app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.create_state.as_ref().unwrap().selected_base, 0);
    }

    #[test]
    fn space_on_hooks_field_toggles_hooks() {
        let mut app = app_with_create_state();
        // Move to Hooks field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Hooks
        );
        assert!(app.create_state.as_ref().unwrap().hooks_enabled);
        app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(!app.create_state.as_ref().unwrap().hooks_enabled);
    }

    #[test]
    fn backspace_on_create_removes_char() {
        let mut app = app_with_create_state();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.create_state.as_ref().unwrap().branch_input, "a");
    }

    #[test]
    fn app_has_create_state_initially_none() {
        let app = App::new();
        assert!(app.create_state.is_none());
    }

    #[test]
    fn n_on_list_sets_create_state() {
        let mut app = App::new();
        // Without a real git repo, init_create_form will use fallback defaults
        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Create);
        assert!(
            app.create_state.is_some(),
            "create_state should be initialized"
        );
    }

    #[test]
    fn enter_on_hooks_field_with_empty_branch_shows_validation_error() {
        let mut app = app_with_create_state();
        // Move to Hooks field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Hooks
        );
        // Try to create with empty branch
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // Should still be on Create screen with error
        assert_eq!(app.active_screen(), Screen::Create);
        assert!(
            app.create_state.as_ref().unwrap().error.is_some(),
            "should show validation error for empty branch"
        );
        assert!(
            app.create_state.as_ref().unwrap().result.is_none(),
            "should not have result yet"
        );
    }

    #[test]
    fn enter_on_hooks_field_with_branch_triggers_execute() {
        let mut app = app_with_create_state();
        // Type a branch name
        for c in "test-branch".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        // Move to Hooks field
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        // Try to create — will fail without real git repo but should set result
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Create);
        let state = app.create_state.as_ref().unwrap();
        assert!(
            state.is_result_mode(),
            "should be in result mode after execute attempt"
        );
    }

    #[test]
    fn enter_in_create_result_mode_pops_to_list() {
        let mut app = app_with_create_state();
        // Set result directly
        app.create_state.as_mut().unwrap().result = Some(screens::create::CreateResultMessage {
            success: true,
            message: "Created 'test'".into(),
        });
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Enter in result mode should pop to list"
        );
        assert!(app.create_state.is_none(), "create_state should be cleared");
    }

    #[test]
    fn space_in_create_result_mode_pops_to_list() {
        let mut app = app_with_create_state();
        app.create_state.as_mut().unwrap().result = Some(screens::create::CreateResultMessage {
            success: false,
            message: "Create failed".into(),
        });
        app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.create_state.is_none());
    }

    #[test]
    fn create_result_mode_renders_result_message() {
        let mut app = app_with_create_state();
        app.create_state.as_mut().unwrap().result = Some(screens::create::CreateResultMessage {
            success: true,
            message: "Created 'feat-x' at /tmp/wt/feat-x".into(),
        });
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(content.contains("Created"), "should show result message");
        assert!(content.contains("dismiss"), "should show dismiss footer");
    }

    #[test]
    fn help_screen_renders_help_overlay_not_placeholder() {
        let mut app = App::new();
        app.push_screen(Screen::Help);
        let backend = ratatui::backend::TestBackend::new(80, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            content.contains("Help"),
            "Help screen should render help overlay with title"
        );
        assert!(
            content.contains("Global"),
            "Help overlay should show Global group header"
        );
        assert!(
            content.contains("List"),
            "Help overlay should show List group header"
        );
    }

    fn sample_detail_state() -> screens::detail::DetailState {
        screens::detail::DetailState {
            name: "feat-a".into(),
            branch: "feat/a".into(),
            path: "/tmp/wt/feat-a".into(),
            base_branch: "main".into(),
            ahead_behind: "+0/-0".into(),
            created: "2026-03-10".into(),
            last_accessed: "2026-03-11".into(),
            hook_status: "success".into(),
            hook_timestamp: "2026-03-10".into(),
            changed_files: vec![("file.rs".into(), "modified".into())],
            commits: vec![("abc1234".into(), "test commit".into())],
        }
    }

    #[test]
    fn detail_screen_renders_detail_state_not_placeholder() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(
            content.contains("feat/a"),
            "detail screen should show branch from detail_state, got: {:?}",
            content.trim()
        );
        assert!(
            !content.contains("trench TUI"),
            "detail screen should NOT show placeholder"
        );
    }

    #[test]
    fn detail_screen_shows_changed_files_and_commits() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(content.contains("file.rs"), "should show changed file");
        assert!(content.contains("abc1234"), "should show commit hash");
    }

    #[test]
    fn s_on_empty_list_does_not_push_sync_picker() {
        let mut app = App::new();
        // Empty list — no rows
        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "s on empty list should stay on List"
        );
        assert!(
            app.sync_picker_state.is_none(),
            "sync_picker_state should remain None"
        );
    }

    #[test]
    fn s_on_detail_pushes_sync_picker() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        let picker = app
            .sync_picker_state
            .as_ref()
            .expect("sync_picker_state should be set");
        assert_eq!(picker.worktree_name, "feat-a");
    }

    #[test]
    fn o_on_detail_sets_editor_request() {
        let mut app = App::new();
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(app.is_running(), "o on detail should not crash or quit");
        assert_eq!(app.active_screen(), Screen::Detail);
        assert_eq!(
            app.editor_request,
            Some("/tmp/wt/feat-a".to_string()),
            "o should set editor_request to worktree path"
        );
    }

    #[test]
    fn arrow_down_on_sync_picker_selects_merge() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.sync_picker_state.as_ref().unwrap().selected, 0);

        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            app.sync_picker_state.as_ref().unwrap().selected,
            1,
            "down should select Merge"
        );
    }

    #[test]
    fn arrow_up_on_sync_picker_selects_rebase() {
        let mut app = App::new();
        let mut state = screens::sync_picker::SyncPickerState::new("feat-auth");
        state.selected = 1;
        app.sync_picker_state = Some(state);
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(
            app.sync_picker_state.as_ref().unwrap().selected,
            0,
            "up should select Rebase"
        );
    }

    #[test]
    fn j_k_keys_work_on_sync_picker() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(
            app.sync_picker_state.as_ref().unwrap().selected,
            1,
            "j should move down"
        );

        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(
            app.sync_picker_state.as_ref().unwrap().selected,
            0,
            "k should move up"
        );
    }

    #[test]
    fn enter_on_sync_picker_triggers_sync_and_sets_result() {
        // Without a real git repo, execute_sync will fail — but it should
        // set a result message (failure) and stay on the SyncPicker screen.
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        // Should still be on SyncPicker (showing result)
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        let picker = app.sync_picker_state.as_ref().unwrap();
        assert!(
            picker.is_result_mode(),
            "should be in result mode after Enter"
        );
        assert!(picker.result.is_some());
    }

    #[test]
    fn enter_in_result_mode_pops_back_to_list() {
        let mut app = App::new();
        let mut state = screens::sync_picker::SyncPickerState::new("feat-auth");
        state.result = Some(screens::sync_picker::SyncResultMessage {
            success: true,
            message: "Synced successfully".into(),
        });
        app.sync_picker_state = Some(state);
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Enter in result mode should pop to list"
        );
        assert!(
            app.sync_picker_state.is_none(),
            "sync_picker_state should be cleared"
        );
    }

    #[test]
    fn enter_in_result_mode_pops_to_list_from_detail_path() {
        let mut app = app_with_rows();
        // Simulate Detail → SyncPicker flow: nav stack = [List, Detail, SyncPicker]
        app.detail_state = Some(sample_detail_state());
        app.push_screen(Screen::Detail);
        let mut state = screens::sync_picker::SyncPickerState::new("feat-a");
        state.result = Some(screens::sync_picker::SyncResultMessage {
            success: true,
            message: "Synced successfully".into(),
        });
        app.sync_picker_state = Some(state);
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 3);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "should pop all the way to List, not Detail"
        );
        assert!(
            app.sync_picker_state.is_none(),
            "sync_picker_state should be cleared"
        );
    }

    #[test]
    fn sync_picker_screen_renders_options_through_app() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        let backend = ratatui::backend::TestBackend::new(80, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(content.contains("Rebase"), "should render Rebase option");
        assert!(content.contains("Merge"), "should render Merge option");
        assert!(content.contains("feat-auth"), "should show worktree name");
    }

    #[test]
    fn screen_enum_has_sync_picker_variant() {
        let screen = Screen::SyncPicker;
        assert_ne!(screen, Screen::List);
        assert_ne!(screen, Screen::Detail);
    }

    #[test]
    fn app_has_sync_picker_state_initially_none() {
        let app = App::new();
        assert!(app.sync_picker_state.is_none());
    }

    #[test]
    fn app_has_delete_confirm_state_initially_none() {
        let app = App::new();
        assert!(app.delete_confirm_state.is_none());
    }

    #[test]
    fn push_delete_confirm_screen_works() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);
        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn enter_on_delete_confirm_triggers_delete_and_sets_result() {
        // Without a real git repo, execute_delete will fail — but it should
        // set a result message (failure) and stay on the DeleteConfirm screen.
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        let state = app.delete_confirm_state.as_ref().unwrap();
        assert!(
            state.is_result_mode(),
            "should be in result mode after Enter"
        );
        assert!(state.result.is_some());
    }

    #[test]
    fn y_on_delete_confirm_triggers_delete_and_sets_result() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert_eq!(app.active_screen(), Screen::DeleteConfirm);
        let state = app.delete_confirm_state.as_ref().unwrap();
        assert!(state.is_result_mode(), "y should also trigger delete");
    }

    #[test]
    fn n_on_delete_confirm_cancels_dialog() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

        assert_eq!(
            app.active_screen(),
            Screen::List,
            "n should pop back to list"
        );
        assert!(
            app.delete_confirm_state.is_none(),
            "state should be cleared on cancel"
        );
    }

    #[test]
    fn enter_in_delete_result_mode_pops_to_list() {
        let mut app = App::new();
        let mut state = screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        );
        state.result = Some(screens::delete_confirm::DeleteResultMessage {
            success: true,
            message: "Removed successfully".into(),
        });
        app.delete_confirm_state = Some(state);
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Enter in result mode should pop to list"
        );
        assert!(
            app.delete_confirm_state.is_none(),
            "state should be cleared"
        );
    }

    #[test]
    fn space_in_delete_result_mode_pops_to_list() {
        let mut app = App::new();
        let mut state = screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        );
        state.result = Some(screens::delete_confirm::DeleteResultMessage {
            success: true,
            message: "Done".into(),
        });
        app.delete_confirm_state = Some(state);
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Space in result mode should pop to list"
        );
        assert!(app.delete_confirm_state.is_none());
    }

    #[test]
    fn esc_on_delete_confirm_clears_state() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Esc should pop back to List"
        );
        assert!(
            app.delete_confirm_state.is_none(),
            "Esc should clear delete_confirm_state"
        );
    }

    #[test]
    fn q_on_delete_confirm_clears_state() {
        let mut app = App::new();
        app.delete_confirm_state = Some(screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        ));
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(
            app.delete_confirm_state.is_none(),
            "q should clear delete_confirm_state"
        );
    }

    #[test]
    fn esc_on_delete_result_mode_clears_state() {
        let mut app = App::new();
        let mut state = screens::delete_confirm::DeleteConfirmState::new(
            "feat-auth",
            "/tmp/wt/feat-auth",
            "feature/auth",
        );
        state.result = Some(screens::delete_confirm::DeleteResultMessage {
            success: true,
            message: "Done".into(),
        });
        app.delete_confirm_state = Some(state);
        app.push_screen(Screen::DeleteConfirm);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "Esc in result mode should pop to List"
        );
        assert!(
            app.delete_confirm_state.is_none(),
            "Esc in result mode should clear state"
        );
    }

    #[test]
    fn esc_on_sync_picker_clears_state() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(
            app.sync_picker_state.is_none(),
            "Esc should clear sync_picker_state"
        );
    }

    #[test]
    fn push_sync_picker_screen_works() {
        let mut app = App::new();
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-auth"));
        app.push_screen(Screen::SyncPicker);
        assert_eq!(app.active_screen(), Screen::SyncPicker);
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn help_over_sync_picker_renders_sync_picker_underneath() {
        let mut app = app_with_rows();
        // Push SyncPicker, then Help
        app.sync_picker_state = Some(screens::sync_picker::SyncPickerState::new("feat-a"));
        app.push_screen(Screen::SyncPicker);
        app.push_screen(Screen::Help);
        assert_eq!(app.active_screen(), Screen::Help);

        let backend = ratatui::backend::TestBackend::new(80, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.ui(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        // Help overlay should be present
        assert!(content.contains("Help"), "should render Help overlay");
        // SyncPicker should be underneath (not list)
        assert!(
            content.contains("Sync strategy for"),
            "should render SyncPicker underneath Help overlay, got: {:?}",
            content.trim()
        );
    }

    #[test]
    fn q_inserts_into_branch_field_on_create_screen() {
        let mut app = app_with_create_state();
        assert_eq!(app.active_screen(), Screen::Create);
        // Branch field is focused by default
        assert_eq!(
            app.create_state.as_ref().unwrap().focused_field,
            screens::create::CreateField::Branch
        );

        // Press 'q' — should insert into branch_input, NOT pop screen
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert_eq!(
            app.active_screen(),
            Screen::Create,
            "q should NOT pop the Create screen when Branch field is focused"
        );
        assert_eq!(
            app.create_state.as_ref().unwrap().branch_input,
            "q",
            "q should be inserted into branch_input"
        );
    }

    #[test]
    fn backspace_to_empty_revalidates_and_shows_error() {
        let mut app = app_with_create_state();
        // Type a char then backspace — should revalidate and show error for empty
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(
            app.create_state.as_ref().unwrap().error.is_none(),
            "valid input should have no error"
        );
        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert!(
            app.create_state.as_ref().unwrap().error.is_some(),
            "empty input after backspace should revalidate and show error"
        );
    }

    #[test]
    fn process_hook_messages_updates_hook_log_state() {
        use screens::hook_log::{HookLogState, HookOutputMessage};

        let mut app = App::new();
        let (tx, rx) = std::sync::mpsc::channel();
        app.hook_log_state = Some(HookLogState::new("post_create"));
        app.hook_rx = Some(rx);
        app.push_screen(Screen::HookLog);

        // Send messages through the channel
        tx.send(HookOutputMessage::StepStarted { step: "run".into() })
            .unwrap();
        tx.send(HookOutputMessage::OutputLine {
            step: "run".into(),
            stream: "stdout".into(),
            line: "hello".into(),
        })
        .unwrap();
        tx.send(HookOutputMessage::StepCompleted {
            step: "run".into(),
            success: true,
            duration: std::time::Duration::from_millis(100),
        })
        .unwrap();
        tx.send(HookOutputMessage::HookCompleted {
            success: true,
            duration: std::time::Duration::from_secs(1),
            error: None,
        })
        .unwrap();

        // Process messages
        app.process_hook_messages();

        let state = app.hook_log_state.as_ref().unwrap();
        assert_eq!(state.sections.len(), 1);
        assert_eq!(state.sections[0].step, "run");
        assert_eq!(state.sections[0].lines.len(), 1);
        assert_eq!(state.sections[0].lines[0].text, "hello");
        assert!(state.completed);
        assert!(state.success);
    }

    #[test]
    fn process_hook_messages_no_op_without_receiver() {
        let mut app = App::new();
        // No hook_rx set — should not panic
        app.process_hook_messages();
    }

    #[test]
    fn start_hook_log_sets_up_state_and_pushes_screen() {
        let mut app = App::new();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.start_hook_log("post_create", rx);
        assert_eq!(app.active_screen(), Screen::HookLog);
        assert!(app.hook_log_state.is_some());
        assert_eq!(app.hook_log_state.as_ref().unwrap().title, "post_create");
        assert!(app.hook_rx.is_some());
    }

    #[test]
    fn esc_on_hook_log_returns_to_list_and_clears_state() {
        let mut app = App::new();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.start_hook_log("post_create", rx);
        assert_eq!(app.active_screen(), Screen::HookLog);

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.hook_log_state.is_none());
        // hook_rx stays alive for post-dismiss draining
        assert!(app.hook_rx.is_some());
    }

    #[test]
    fn q_on_hook_log_returns_to_list() {
        let mut app = App::new();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.start_hook_log("post_create", rx);
        assert_eq!(app.active_screen(), Screen::HookLog);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.hook_log_state.is_none());
    }

    #[test]
    fn dismiss_hook_log_returns_to_list_not_source() {
        let mut app = App::new();
        // Simulate: List → Create → HookLog (as happens during create-with-hooks)
        app.create_state = Some(screens::create::CreateState::new(
            vec![],
            String::new(),
            String::new(),
        ));
        app.push_screen(Screen::Create);
        let (_tx, rx) = std::sync::mpsc::channel();
        app.start_hook_log("create hooks", rx);
        assert_eq!(app.active_screen(), Screen::HookLog);
        assert_eq!(app.nav_stack_depth(), 3); // List → Create → HookLog

        // Esc should dismiss HookLog AND the source dialog, landing on List
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::List,
            "dismiss from HookLog should return to List, not Create"
        );
        assert!(
            app.hook_log_state.is_none(),
            "hook_log_state should be cleared"
        );
        assert!(
            app.create_state.is_none(),
            "source dialog state should be cleared"
        );
    }

    #[test]
    fn process_hook_messages_drains_after_dismiss() {
        use screens::hook_log::HookOutputMessage;

        let mut app = App::new();
        let (tx, rx) = std::sync::mpsc::channel();
        app.start_hook_log("test hooks", rx);
        assert_eq!(app.active_screen(), Screen::HookLog);

        // Dismiss the hook log (user pressed Esc)
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::List);
        assert!(app.hook_log_state.is_none(), "state should be cleared");
        // hook_rx should still be alive for draining
        assert!(app.hook_rx.is_some(), "hook_rx should survive dismiss");

        // Background thread sends completion after dismiss
        tx.send(HookOutputMessage::HookCompleted {
            success: true,
            duration: std::time::Duration::from_secs(1),
            error: None,
        })
        .unwrap();

        // process_hook_messages should drain and clean up
        app.process_hook_messages();
        assert!(
            app.hook_rx.is_none(),
            "hook_rx should be cleaned up after HookCompleted"
        );
    }

    #[test]
    fn hook_log_arrow_down_scrolls_state() {
        let mut app = App::new();
        let mut state = screens::hook_log::HookLogState::new("test");
        state.process_message(screens::hook_log::HookOutputMessage::StepStarted {
            step: "run".into(),
        });
        for i in 0..30 {
            state.process_message(screens::hook_log::HookOutputMessage::OutputLine {
                step: "run".into(),
                stream: "stdout".into(),
                line: format!("line {i}"),
            });
        }
        state.scroll_offset = 0;
        state.completed = true;
        app.hook_log_state = Some(state);
        app.push_screen(Screen::HookLog);

        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(
            app.hook_log_state.as_ref().unwrap().scroll_offset > 0,
            "Down arrow should scroll the hook log"
        );
    }

    #[test]
    fn hook_log_arrow_up_scrolls_state() {
        let mut app = App::new();
        let mut state = screens::hook_log::HookLogState::new("test");
        state.scroll_offset = 5;
        state.completed = true;
        app.hook_log_state = Some(state);
        app.push_screen(Screen::HookLog);

        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(
            app.hook_log_state.as_ref().unwrap().scroll_offset, 4,
            "Up arrow should decrement scroll offset"
        );
    }

    #[test]
    fn hook_log_page_down_scrolls_state() {
        let mut app = App::new();
        let mut state = screens::hook_log::HookLogState::new("test");
        state.process_message(screens::hook_log::HookOutputMessage::StepStarted {
            step: "run".into(),
        });
        for i in 0..50 {
            state.process_message(screens::hook_log::HookOutputMessage::OutputLine {
                step: "run".into(),
                stream: "stdout".into(),
                line: format!("line {i}"),
            });
        }
        state.scroll_offset = 0;
        state.completed = true;
        app.hook_log_state = Some(state);
        app.push_screen(Screen::HookLog);

        app.handle_key_event(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(
            app.hook_log_state.as_ref().unwrap().scroll_offset > 0,
            "PageDown should scroll the hook log"
        );
    }

    #[test]
    fn l_key_on_list_does_not_crash() {
        let mut app = app_with_rows();
        assert_eq!(app.active_screen(), Screen::List);

        // `l` triggers load_hook_log_replay. Depending on whether a real DB
        // and repo are accessible, it either stays on List (DB unavailable)
        // or pushes HookLog with "no history" message.
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        let screen = app.active_screen();
        assert!(
            screen == Screen::List || screen == Screen::HookLog,
            "should be on List or HookLog, got: {screen:?}"
        );
    }

    #[test]
    fn l_key_on_detail_does_not_crash() {
        let mut app = app_with_rows();
        app.detail_state = Some(screens::detail::DetailState {
            name: "feat-a".into(),
            branch: "feat/a".into(),
            path: "/tmp/wt/feat-a".into(),
            base_branch: "main".into(),
            ahead_behind: "+0/-0".into(),
            created: "2026-03-01".into(),
            last_accessed: "-".into(),
            hook_status: "-".into(),
            hook_timestamp: "-".into(),
            changed_files: vec![],
            commits: vec![],
        });
        app.push_screen(Screen::Detail);
        assert_eq!(app.active_screen(), Screen::Detail);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        let screen = app.active_screen();
        assert!(
            screen == Screen::Detail || screen == Screen::HookLog,
            "should be on Detail or HookLog, got: {screen:?}"
        );
    }

    #[test]
    fn hook_log_j_k_scroll_state() {
        let mut app = App::new();
        let mut state = screens::hook_log::HookLogState::new("test");
        state.scroll_offset = 5;
        state.completed = true;
        app.hook_log_state = Some(state);
        app.push_screen(Screen::HookLog);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(
            app.hook_log_state.as_ref().unwrap().scroll_offset, 4,
            "k should scroll up"
        );

        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        // j scrolls down — scroll_offset might go back up or stay depending on total_lines
        // Just verify it didn't crash and the handler ran
    }

    #[test]
    fn replay_dismiss_returns_to_detail_not_list() {
        let mut app = app_with_rows();
        // Navigate to Detail
        app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.active_screen(), Screen::Detail);

        // Simulate replay hook log pushed from Detail
        let mut state = screens::hook_log::HookLogState::new("post_create");
        state.replay = true;
        app.hook_log_state = Some(state);
        app.push_screen(Screen::HookLog);
        assert_eq!(app.active_screen(), Screen::HookLog);
        assert_eq!(app.nav_stack_depth(), 3); // List → Detail → HookLog

        // Esc should pop back to Detail (not unwind to List)
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.active_screen(),
            Screen::Detail,
            "replay dismiss should return to Detail, not List"
        );
        assert_eq!(app.nav_stack_depth(), 2);
    }

    #[test]
    fn app_starts_with_no_watcher() {
        let app = App::new();
        assert!(app.watcher.is_none(), "watcher should be None initially");
    }

    #[test]
    fn check_watcher_noop_without_watcher() {
        let mut app = App::new();
        // Should not panic or do anything
        app.check_watcher();
    }

    #[test]
    fn check_watcher_triggers_refresh_after_debounce() {
        use std::time::Duration;

        let dir = tempfile::TempDir::new().unwrap();
        let dw = watcher::DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(50),
        )
        .unwrap();

        let mut app = app_with_rows();
        app.watcher = Some(dw);

        // Initial state: 3 rows
        assert_eq!(app.list_state.rows.len(), 3);

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher();

        // Create a file to trigger watcher event
        std::fs::write(dir.path().join("trigger.txt"), "change").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher(); // picks up event

        // Wait for debounce to expire
        std::thread::sleep(Duration::from_millis(100));

        // check_watcher should request refresh (sets refresh_pending)
        // Since we can't call refresh_list (no real repo), we test the flag
        assert!(
            app.check_watcher_returns_refresh(),
            "should signal refresh after debounce"
        );
    }

    #[test]
    fn check_watcher_skips_refresh_on_non_list_screen() {
        use std::time::Duration;

        let dir = tempfile::TempDir::new().unwrap();
        let dw = watcher::DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(50),
        )
        .unwrap();

        let mut app = app_with_rows();
        app.watcher = Some(dw);
        app.push_screen(Screen::Help); // not on List screen

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher();

        // Trigger event
        std::fs::write(dir.path().join("trigger.txt"), "change").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher();

        std::thread::sleep(Duration::from_millis(100));

        // Should NOT signal refresh while on non-List screen
        assert!(
            !app.check_watcher_returns_refresh(),
            "should not refresh on non-List screen"
        );
    }

    #[test]
    fn rebuild_watcher_updates_watched_paths() {
        use std::time::Duration;

        let dir1 = tempfile::TempDir::new().unwrap();
        let dir2 = tempfile::TempDir::new().unwrap();

        let mut app = App::new();
        app.auto_refresh = true;

        // Simulate list_state with dir1 only
        app.list_state = screens::list::ListState::new(vec![screens::list::WorktreeRow {
            name: "wt1".to_string(),
            branch: "main".to_string(),
            path: dir1.path().to_string_lossy().to_string(),
            status: String::new(),
            ahead_behind: String::new(),
            processes: String::new(),
            managed: true,
        }]);
        app.rebuild_watcher();
        assert!(app.watcher.is_some(), "watcher should be initialized");

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(200));
        if let Some(ref mut w) = app.watcher {
            w.should_refresh();
        }

        // Changes in dir2 should NOT be detected (not watched)
        std::fs::write(dir2.path().join("test.txt"), "hello").unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !app.check_watcher_returns_refresh(),
            "dir2 should not be watched yet"
        );

        // Now update list_state to include dir2
        app.list_state = screens::list::ListState::new(vec![
            screens::list::WorktreeRow {
                name: "wt1".to_string(),
                branch: "main".to_string(),
                path: dir1.path().to_string_lossy().to_string(),
                status: String::new(),
                ahead_behind: String::new(),
                processes: String::new(),
                managed: true,
            },
            screens::list::WorktreeRow {
                name: "wt2".to_string(),
                branch: "feature".to_string(),
                path: dir2.path().to_string_lossy().to_string(),
                status: String::new(),
                ahead_behind: String::new(),
                processes: String::new(),
                managed: true,
            },
        ]);
        app.rebuild_watcher();

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(300));
        if let Some(ref mut w) = app.watcher {
            w.should_refresh();
        }
        std::thread::sleep(Duration::from_millis(600));
        if let Some(ref mut w) = app.watcher {
            w.should_refresh();
        }

        // Changes in dir2 should NOW be detected
        std::fs::write(dir2.path().join("test2.txt"), "world").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        if let Some(ref mut w) = app.watcher {
            w.should_refresh(); // picks up event
        }
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            app.check_watcher_returns_refresh(),
            "dir2 should be watched after rebuild"
        );
    }

    #[test]
    fn check_watcher_preserves_pending_refresh_on_non_list_screen() {
        use std::time::Duration;

        let dir = tempfile::TempDir::new().unwrap();
        let dw = watcher::DebouncedWatcher::with_debounce(
            &[dir.path()],
            Duration::from_millis(50),
        )
        .unwrap();

        let mut app = app_with_rows();
        app.watcher = Some(dw);
        app.push_screen(Screen::Help); // not on List screen

        // Drain startup noise
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher();

        // Trigger event while on Help screen
        std::fs::write(dir.path().join("trigger.txt"), "change").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher(); // drains event

        // Wait for debounce to expire while still on Help screen
        std::thread::sleep(Duration::from_millis(100));
        app.check_watcher(); // debounce expires — should NOT lose the pending refresh

        // Now return to List screen
        app.nav_stack.pop(); // back to List
        assert_eq!(app.active_screen(), Screen::List);

        // The pending refresh should still be available
        assert!(
            app.check_watcher_returns_refresh(),
            "pending refresh should be preserved when returning to List screen"
        );
    }
}
