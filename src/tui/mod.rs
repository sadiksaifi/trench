pub mod screens;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Alignment, widgets::Paragraph, Frame};

pub struct App {
    running: bool,
}

impl App {
    pub fn new() -> Self {
        Self { running: true }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn ui(&self, frame: &mut Frame) {
        let placeholder = Paragraph::new("trench TUI — press q to quit")
            .alignment(Alignment::Center);
        frame.render_widget(placeholder, frame.area());
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => self.running = false,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.running = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn app_starts_in_running_state() {
        let app = App::new();
        assert!(app.is_running(), "newly created app should be running");
    }

    #[test]
    fn app_exits_on_q_key() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.is_running(), "app should stop after pressing 'q'");
    }

    #[test]
    fn app_exits_on_ctrl_c() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.is_running(), "app should stop after Ctrl+C");
    }

    #[test]
    fn app_ignores_other_keys() {
        let mut app = App::new();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.is_running(), "non-quit keys should not stop the app");
    }

    #[test]
    fn placeholder_ui_renders_trench_tui() {
        let app = App::new();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| app.ui(frame))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let content: String = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            content.contains("trench TUI"),
            "placeholder screen should contain 'trench TUI', got: {:?}",
            content.trim()
        );
    }
}
