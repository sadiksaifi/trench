use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiOptions {
    pub theme_name: String,
    pub date_format: String,
    pub show_ahead_behind: bool,
    pub show_dirty_count: bool,
}

impl Default for UiOptions {
    fn default() -> Self {
        Self {
            theme_name: "ops".to_string(),
            date_format: "%Y-%m-%d %H:%M".to_string(),
            show_ahead_behind: true,
            show_dirty_count: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStatus<'a> {
    pub repo_name: Option<&'a str>,
    pub screen_label: &'a str,
    pub theme_name: &'a str,
    pub auto_refresh: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Success,
    Warning,
    Error,
    Muted,
}

pub fn render_app_frame(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    status: &AppStatus<'_>,
) -> Rect {
    frame.render_widget(
        Block::default().style(Style::default().fg(theme.fg).bg(theme.bg)),
        area,
    );

    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    let repo = status.repo_name.unwrap_or("no repo");
    let refresh = if status.auto_refresh {
        "watch on"
    } else {
        "watch off"
    };

    let line = Line::from(vec![
        Span::styled(
            " trench ",
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        pill(theme, repo, Tone::Muted),
        Span::raw("  "),
        pill(theme, status.screen_label, Tone::Accent),
        Span::raw("  "),
        pill(
            theme,
            refresh,
            if status.auto_refresh {
                Tone::Success
            } else {
                Tone::Warning
            },
        ),
        Span::raw("  "),
        pill(theme, status.theme_name, Tone::Muted),
    ]);

    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(theme.fg).bg(theme.bg_elevated)),
        chunks[0],
    );

    chunks[1]
}

pub fn panel<'a>(title: impl Into<Line<'a>>, theme: &Theme) -> Block<'a> {
    Block::default()
        .title(title)
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_panel))
}

pub fn render_keybar(frame: &mut Frame, area: Rect, theme: &Theme, items: &[(&str, &str)]) {
    frame.render_widget(
        Paragraph::new(keybar_line(theme, items)).style(
            Style::default()
                .fg(theme.fg)
                .bg(theme.bg_elevated)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

pub fn render_empty_state(frame: &mut Frame, area: Rect, theme: &Theme, title: &str, body: &str) {
    let block = panel(Line::from(title.to_string()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                title.to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                body.to_string(),
                Style::default().fg(theme.fg_muted),
            )),
        ])
        .alignment(Alignment::Center),
        inner,
    );
}

pub fn render_modal(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    width: u16,
    height: u16,
    title: &str,
) -> Rect {
    let dialog_area = centered_rect(width, height, area);
    frame.render_widget(Clear, dialog_area);
    let block = panel(Line::from(title.to_string()), theme);
    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);
    inner
}

pub fn keybar_line(theme: &Theme, items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (idx, (key, desc)) in items.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", Style::default().bg(theme.bg_elevated)));
        }
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.accent_soft)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {desc}"),
            Style::default().fg(theme.fg_muted),
        ));
    }
    Line::from(spans)
}

pub fn pill(theme: &Theme, label: &str, tone: Tone) -> Span<'static> {
    let (fg, bg) = match tone {
        Tone::Accent => (theme.selection_fg, theme.accent_soft),
        Tone::Success => (theme.success, theme.bg_panel),
        Tone::Warning => (theme.warning, theme.bg_panel),
        Tone::Error => (theme.error, theme.bg_panel),
        Tone::Muted => (theme.fg_muted, theme.bg_panel),
    };
    Span::styled(
        format!(" {label} "),
        Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}
