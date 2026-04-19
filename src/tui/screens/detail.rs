use std::path::Path;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::git;
use crate::state::Database;
use crate::tui::screens::list::WorktreeRow;

/// View model for the detail screen showing a single worktree's information.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailState {
    pub name: String,
    pub branch: String,
    pub path: String,
    pub base_branch: String,
    pub ahead_behind: String,
    pub created: String,
    pub last_accessed: String,
    pub hook_status: String,
    pub hook_timestamp: String,
    pub changed_files: Vec<(String, String)>,
    pub commits: Vec<(String, String)>,
}

/// Build a DetailState for the selected worktree by querying DB and git.
///
/// Best-effort: fields that fail to load are replaced with fallback values
/// so the detail screen always renders something.
pub fn load_detail(name: &str, cwd: &Path, db: &Database, date_format: &str) -> DetailState {
    let repo_info = git::discover_repo(cwd).ok();
    let repo_path = repo_info.as_ref().map(|r| r.path.clone());

    // Look up DB worktree
    let db_repo = repo_path
        .as_ref()
        .and_then(|p| p.to_str())
        .and_then(|p| db.get_repo_by_path(p).ok().flatten());

    let db_wt = db_repo
        .as_ref()
        .and_then(|r| db.find_worktree_by_identifier(r.id, name).ok().flatten());

    let wt_path = db_wt.as_ref().map(|w| w.path.clone());
    let branch = db_wt
        .as_ref()
        .map(|w| w.branch.clone())
        .unwrap_or_else(|| name.to_string());
    let base_branch = db_wt
        .as_ref()
        .and_then(|w| w.base_branch.clone())
        .or_else(|| repo_info.as_ref().map(|r| r.default_branch.clone()))
        .unwrap_or_else(|| "-".to_string());

    let ahead_behind = repo_path
        .as_ref()
        .and_then(|rp| {
            git::ahead_behind(rp, &branch, Some(&base_branch))
                .ok()
                .flatten()
        })
        .map(|(a, b)| format!("+{a}/-{b}"))
        .unwrap_or_else(|| "-".to_string());

    let created = db_wt
        .as_ref()
        .map(|w| format_timestamp(w.created_at, date_format))
        .unwrap_or_else(|| "-".to_string());

    let last_accessed = db_wt
        .as_ref()
        .and_then(|w| w.last_accessed)
        .map(|ts| format_timestamp(ts, date_format))
        .unwrap_or_else(|| "never".to_string());

    // Hook status from most recent event
    let (hook_status, hook_timestamp) = db_wt
        .as_ref()
        .and_then(|w| {
            db.list_events(w.id, 1).ok().and_then(|events| {
                events.into_iter().next().map(|e| {
                    (
                        e.event_type.clone(),
                        format_timestamp(e.created_at, date_format),
                    )
                })
            })
        })
        .unwrap_or_else(|| ("none".to_string(), "-".to_string()));

    // Git data
    let changed_files = if let Some(ref wt_path) = wt_path {
        git::changed_files(Path::new(wt_path))
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.path, f.status.to_string()))
            .collect()
    } else {
        vec![]
    };

    let commits = if let Some(ref wt_path) = wt_path {
        git::recent_commits(Path::new(wt_path), 10)
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.hash, c.message))
            .collect()
    } else {
        vec![]
    };

    DetailState {
        name: name.to_string(),
        branch,
        path: wt_path.unwrap_or_else(|| "-".to_string()),
        base_branch,
        ahead_behind,
        created,
        last_accessed,
        hook_status,
        hook_timestamp,
        changed_files,
        commits,
    }
}

/// Build a best-effort detail view directly from a selected list row.
pub fn fallback_from_row(row: &WorktreeRow) -> DetailState {
    DetailState {
        name: row.name.clone(),
        branch: row.branch.clone(),
        path: row.path.clone(),
        base_branch: "-".to_string(),
        ahead_behind: if row.ahead_behind.is_empty() {
            "-".to_string()
        } else {
            row.ahead_behind.clone()
        },
        created: "-".to_string(),
        last_accessed: "never".to_string(),
        hook_status: "none".to_string(),
        hook_timestamp: "-".to_string(),
        changed_files: vec![],
        commits: vec![],
    }
}

fn format_timestamp(ts: i64, format: &str) -> String {
    if ts < 0 {
        return "-".to_string();
    }
    let secs = ts;
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    let (year, month, day) = days_to_date(days);
    let mut rendered = format.to_string();
    for (token, value) in [
        ("%Y", format!("{year:04}")),
        ("%m", format!("{month:02}")),
        ("%d", format!("{day:02}")),
        ("%H", format!("{hours:02}")),
        ("%M", format!("{minutes:02}")),
    ] {
        rendered = rendered.replace(token, &value);
    }
    rendered
}

fn days_to_date(days_since_epoch: i64) -> (i64, i64, i64) {
    // Algorithm from https://howardhinnant.github.io/date_algorithms.html
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub fn render(
    state: &DetailState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let bold = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let chunks = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);

    let metadata_lines = vec![
        Line::from(vec![
            Span::styled("Branch: ", bold),
            Span::raw(&state.branch),
            Span::raw("  "),
            Span::styled("Name: ", bold),
            Span::raw(&state.name),
        ]),
        Line::from(vec![Span::styled("Path:   ", bold), Span::raw(&state.path)]),
        Line::from(vec![
            Span::styled("Base:   ", bold),
            Span::raw(&state.base_branch),
            Span::raw("  "),
            Span::styled("Ahead/Behind: ", bold),
            Span::raw(&state.ahead_behind),
        ]),
        Line::from(vec![
            Span::styled("Created: ", bold),
            Span::raw(&state.created),
            Span::raw("  "),
            Span::styled("Last Accessed: ", bold),
            Span::raw(&state.last_accessed),
        ]),
        Line::from(vec![
            Span::styled("Hook:    ", bold),
            Span::raw(&state.hook_status),
            Span::raw("  "),
            Span::styled("At: ", bold),
            Span::raw(&state.hook_timestamp),
        ]),
    ];
    frame.render_widget(Paragraph::new(metadata_lines), chunks[0]);

    let body_chunks =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(chunks[2]);

    let mut file_lines: Vec<Line> = vec![Line::from(Span::styled("Changed Files", bold))];
    if state.changed_files.is_empty() {
        file_lines.push(Line::from("  No changes"));
    } else {
        for (path, status) in &state.changed_files {
            file_lines.push(Line::from(format!("  {status:>10}  {path}")));
        }
    }
    frame.render_widget(Paragraph::new(file_lines), body_chunks[0]);

    let mut commit_lines: Vec<Line> = vec![Line::from(Span::styled("Recent Commits", bold))];
    if state.commits.is_empty() {
        commit_lines.push(Line::from("  No commits"));
    } else {
        for (hash, message) in &state.commits {
            commit_lines.push(Line::from(format!("  {hash}  {message}")));
        }
    }
    frame.render_widget(Paragraph::new(commit_lines), body_chunks[1]);

    frame.render_widget(
        Paragraph::new(Line::from(" s sync  o open  l log  Esc back ")).style(
            Style::default()
                .fg(theme.selection_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        chunks[3],
    );
}

pub fn render_with_options(
    state: &DetailState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
    options: &crate::tui::chrome::UiOptions,
) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(7),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Worktree Detail",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            crate::tui::chrome::pill(theme, &state.branch, crate::tui::chrome::Tone::Accent),
            Span::raw("  "),
            crate::tui::chrome::pill(theme, &state.base_branch, crate::tui::chrome::Tone::Muted),
        ]))
        .style(Style::default().fg(theme.fg).bg(theme.bg)),
        chunks[0],
    );

    render_summary_card(state, frame, chunks[1], theme, options);

    let body_chunks = if chunks[2].width >= 120 {
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[2])
    } else {
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(chunks[2])
    };
    render_file_card(state, frame, body_chunks[0], theme);
    render_commit_card(state, frame, body_chunks[1], theme);
    crate::tui::chrome::render_keybar(
        frame,
        chunks[3],
        theme,
        &[("s", "sync"), ("o", "open"), ("l", "log"), ("Esc", "back")],
    );
}

fn render_summary_card(
    state: &DetailState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
    options: &crate::tui::chrome::UiOptions,
) {
    let block = crate::tui::chrome::panel(" Summary ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        metric_line("Name", &state.name, theme),
        metric_line("Branch", &state.branch, theme),
        metric_line("Path", &state.path, theme),
        metric_line("Base", &state.base_branch, theme),
    ];
    if options.show_ahead_behind {
        lines.push(metric_line("Ahead/Behind", &state.ahead_behind, theme));
    }
    lines.push(metric_line("Created", &state.created, theme));
    lines.push(metric_line("Last Accessed", &state.last_accessed, theme));
    lines.push(metric_line("Hook", &state.hook_status, theme));
    lines.push(metric_line("Hook At", &state.hook_timestamp, theme));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.fg).bg(theme.bg_panel)),
        inner,
    );
}

fn render_file_card(
    state: &DetailState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let block = crate::tui::chrome::panel(" Changed Files ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![Line::from(vec![crate::tui::chrome::pill(
        theme,
        &format!("{} files", state.changed_files.len()),
        crate::tui::chrome::Tone::Muted,
    )])];
    if state.changed_files.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("No changes"));
    } else {
        for (path, status) in &state.changed_files {
            lines.push(Line::from(format!("{status:>10}  {path}")));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.fg).bg(theme.bg_panel)),
        inner,
    );
}

fn render_commit_card(
    state: &DetailState,
    frame: &mut Frame,
    area: Rect,
    theme: &crate::tui::theme::Theme,
) {
    let block = crate::tui::chrome::panel(" Recent Commits ", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![Line::from(vec![crate::tui::chrome::pill(
        theme,
        &format!("{} commits", state.commits.len()),
        crate::tui::chrome::Tone::Muted,
    )])];
    if state.commits.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("No commits"));
    } else {
        for (hash, message) in &state.commits {
            lines.push(Line::from(format!("{hash}  {message}")));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.fg).bg(theme.bg_panel)),
        inner,
    );
}

fn metric_line(label: &str, value: &str, theme: &crate::tui::theme::Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<14}"),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn render_to_buffer(state: &DetailState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name("catppuccin");
        terminal
            .draw(|frame| render(state, frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        buf.content().iter().map(|cell| cell.symbol()).collect()
    }

    fn sample_detail() -> DetailState {
        DetailState {
            name: "feature-auth".into(),
            branch: "feature/auth".into(),
            path: "/home/user/.worktrees/myproject/feature-auth".into(),
            base_branch: "main".into(),
            ahead_behind: "+1/-0".into(),
            created: "2026-03-10 14:30".into(),
            last_accessed: "2026-03-11 09:15".into(),
            hook_status: "success".into(),
            hook_timestamp: "2026-03-10 14:31".into(),
            changed_files: vec![
                ("src/auth.rs".into(), "modified".into()),
                ("tests/auth_test.rs".into(), "new".into()),
            ],
            commits: vec![
                ("abc1234".into(), "feat: add auth module".into()),
                ("def5678".into(), "test: add auth tests".into()),
            ],
        }
    }

    #[test]
    fn detail_state_holds_all_metadata_fields() {
        let state = sample_detail();
        assert_eq!(state.name, "feature-auth");
        assert_eq!(state.branch, "feature/auth");
        assert_eq!(state.path, "/home/user/.worktrees/myproject/feature-auth");
        assert_eq!(state.base_branch, "main");
        assert_eq!(state.ahead_behind, "+1/-0");
        assert_eq!(state.created, "2026-03-10 14:30");
        assert_eq!(state.last_accessed, "2026-03-11 09:15");
    }

    #[test]
    fn detail_state_holds_hook_status() {
        let state = sample_detail();
        assert_eq!(state.hook_status, "success");
        assert_eq!(state.hook_timestamp, "2026-03-10 14:31");
    }

    #[test]
    fn detail_state_holds_changed_files() {
        let state = sample_detail();
        assert_eq!(state.changed_files.len(), 2);
        assert_eq!(
            state.changed_files[0],
            ("src/auth.rs".into(), "modified".into())
        );
    }

    #[test]
    fn detail_state_holds_commits() {
        let state = sample_detail();
        assert_eq!(state.commits.len(), 2);
        assert_eq!(
            state.commits[0],
            ("abc1234".into(), "feat: add auth module".into())
        );
    }

    #[test]
    fn detail_state_supports_empty_lists() {
        let state = DetailState {
            name: "empty".into(),
            branch: "empty-branch".into(),
            path: "/tmp/empty".into(),
            base_branch: "-".into(),
            ahead_behind: "-".into(),
            created: "-".into(),
            last_accessed: "never".into(),
            hook_status: "none".into(),
            hook_timestamp: "-".into(),
            changed_files: vec![],
            commits: vec![],
        };
        assert!(state.changed_files.is_empty());
        assert!(state.commits.is_empty());
    }

    #[test]
    fn renders_metadata_section_with_branch_and_path() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(
            text.contains("feature/auth"),
            "should show branch, got: {text}"
        );
        assert!(text.contains("feature-auth"), "should show worktree name");
    }

    #[test]
    fn renders_metadata_with_base_branch_and_ahead_behind() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("main"), "should show base branch");
        assert!(text.contains("+1/-0"), "should show ahead/behind");
    }

    #[test]
    fn renders_metadata_with_created_and_last_accessed() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(
            text.contains("2026-03-10 14:30"),
            "should show created date"
        );
        assert!(
            text.contains("2026-03-11 09:15"),
            "should show last accessed"
        );
    }

    #[test]
    fn renders_changed_files_section() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("Changed Files"), "should show section header");
        assert!(
            text.contains("src/auth.rs"),
            "should show first changed file"
        );
        assert!(text.contains("modified"), "should show file status");
        assert!(
            text.contains("tests/auth_test.rs"),
            "should show second file"
        );
        assert!(text.contains("new"), "should show second file status");
    }

    #[test]
    fn renders_no_changes_when_files_empty() {
        let mut state = sample_detail();
        state.changed_files = vec![];
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(
            text.contains("No changes"),
            "should show empty state message"
        );
    }

    #[test]
    fn renders_recent_commits_section() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(
            text.contains("Recent Commits"),
            "should show commits header"
        );
        assert!(text.contains("abc1234"), "should show first commit hash");
        assert!(
            text.contains("feat: add auth module"),
            "should show first commit message"
        );
        assert!(text.contains("def5678"), "should show second commit hash");
    }

    #[test]
    fn renders_no_commits_when_empty() {
        let mut state = sample_detail();
        state.commits = vec![];
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(
            text.contains("No commits"),
            "should show empty commits message"
        );
    }

    #[test]
    fn renders_hook_status_in_metadata() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("Hook"), "should show hook label");
        assert!(text.contains("success"), "should show hook status value");
        assert!(
            text.contains("2026-03-10 14:31"),
            "should show hook timestamp"
        );
    }

    #[test]
    fn renders_hook_status_none() {
        let mut state = sample_detail();
        state.hook_status = "none".into();
        state.hook_timestamp = "-".into();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("none"), "should show 'none' for no hooks");
    }

    #[test]
    fn renders_detail_footer_with_keybindings() {
        let state = sample_detail();
        let buf = render_to_buffer(&state, 100, 30);
        let text = buffer_text(&buf);
        assert!(text.contains("s sync"), "footer should show s sync");
        assert!(text.contains("o open"), "footer should show o open");
        assert!(text.contains("Esc back"), "footer should show Esc back");
    }

    #[test]
    fn footer_is_on_last_line() {
        let state = sample_detail();
        let height: u16 = 20;
        let buf = render_to_buffer(&state, 100, height);
        // Extract last line text
        let last_row = height - 1;
        let mut last_line = String::new();
        for col in 0..100 {
            last_line.push_str(buf.cell((col, last_row)).unwrap().symbol());
        }
        assert!(
            last_line.contains("s sync"),
            "last line should contain keybindings, got: {last_line}"
        );
    }

    #[test]
    fn format_timestamp_returns_dash_for_negative_input() {
        let result = super::format_timestamp(-3600, "%Y-%m-%d %H:%M");
        assert_eq!(result, "-", "negative timestamps should return dash");
    }

    #[test]
    fn format_timestamp_converts_epoch_to_readable() {
        // 2026-03-11 00:00 UTC = 1773187200
        let ts = 1773187200_i64;
        let result = super::format_timestamp(ts, "%Y-%m-%d %H:%M");
        assert!(
            result.starts_with("2026-03-11"),
            "should format as 2026-03-11, got: {result}"
        );
    }

    #[test]
    fn load_detail_returns_fallbacks_for_unknown_worktree() {
        let db = Database::open_in_memory().unwrap();
        // load_detail with a cwd that isn't a git repo — should return safe fallbacks
        let tmp = tempfile::tempdir().unwrap();
        let state = load_detail("nonexistent", tmp.path(), &db, "%Y-%m-%d %H:%M");
        assert_eq!(state.name, "nonexistent");
        assert_eq!(state.path, "-", "missing path should show dash fallback");
        assert_eq!(state.hook_status, "none");
        assert_eq!(state.hook_timestamp, "-");
        assert!(state.changed_files.is_empty());
        assert!(state.commits.is_empty());
    }
}
