use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};
use tui_spinner::FluxFrames;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::operation::{OperationKind, OperationStage};
use crate::tui::{
    app::{unavailable_reason, AppState, Viewport, WorktreeIdentity, WorktreeStatus},
    create_flow::{CreateDialog, CreateMode},
    keymap::{self, Binding, Context},
    line_input::LineInput,
    operation_modal::{ModalStatus, OperationModal},
    remove_flow::{RemoveDialog, RemoveMode},
    sync_flow::{SyncDialog, SyncMode},
    theme::Theme,
};

pub struct ViewModel<'a> {
    state: &'a AppState,
    area: Rect,
}

impl<'a> ViewModel<'a> {
    pub fn new(state: &'a AppState, area: Rect) -> Self {
        Self { state, area }
    }

    pub fn is_tiny(&self) -> bool {
        self.area.width < Viewport::MIN_WIDTH || self.area.height < Viewport::MIN_HEIGHT
    }

    pub fn is_wide(&self) -> bool {
        self.area.width >= Viewport::WIDE_WIDTH
    }

    pub fn inspector_visible(&self) -> bool {
        self.state
            .inspector_override
            .unwrap_or_else(|| self.is_wide())
    }

    pub fn selected(&self) -> Option<&'a WorktreeIdentity> {
        self.state.selected_visible()
    }

    pub fn visible(&self) -> Vec<&'a WorktreeIdentity> {
        self.state.visible_identities()
    }

    pub fn status_for(&self, identity: &WorktreeIdentity) -> Option<&'a WorktreeStatus> {
        self.state.statuses.get(&identity.id)
    }
}

pub fn render(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(
        Block::default().style(theme.with_bg(Style::default().fg(theme.fg), theme.bg)),
        area,
    );
    let model = ViewModel::new(state, area);
    if model.is_tiny() {
        render_resize(&model, frame, theme);
        return;
    }
    render_cockpit(&model, frame, theme);
    let has_dialog = state.operation_modal.is_some()
        || state.create_dialog.is_some()
        || state.sync_dialog.is_some()
        || state.remove_dialog.is_some();
    if has_dialog {
        dim_background(frame, area, theme);
    }
    if let Some(modal) = state.operation_modal.as_ref() {
        render_operation_modal(modal, frame, area, theme);
    } else if let Some(dialog) = state.create_dialog.as_ref() {
        render_create_dialog(dialog, state.refresh.spinner_tick, frame, area, theme);
    } else if let Some(dialog) = state.sync_dialog.as_ref() {
        render_sync_dialog(dialog, state.refresh.spinner_tick, frame, area, theme);
    } else if let Some(dialog) = state.remove_dialog.as_ref() {
        render_remove_dialog(dialog, frame, area, theme);
    }
    if state.help_open {
        dim_background(frame, area, theme);
        if has_dialog {
            render_overlay_help(state, frame, area, theme);
        } else {
            render_help(&model, frame, theme);
        }
        let footer = Rect {
            x: area.x,
            y: area.bottom().saturating_sub(1),
            width: area.width,
            height: 1,
        };
        render_dialog_keybar(
            frame,
            footer,
            theme,
            &[KeyHint::secondary("?", "close help")],
        );
    }
}

fn dim_background(frame: &mut Frame, area: Rect, theme: &Theme) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
                cell.set_fg(theme.disabled_fg)
                    .set_style(Style::default().add_modifier(Modifier::DIM));
            }
        }
    }
}

fn render_remove_dialog(dialog: &RemoveDialog, frame: &mut Frame, area: Rect, theme: &Theme) {
    let footer = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let content_area = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    let modal = centered_rect(
        content_area.width.saturating_sub(8).min(76),
        13,
        content_area,
    );
    frame.render_widget(Clear, modal);
    let title = match dialog.mode() {
        RemoveMode::Review | RemoveMode::Ready => " Remove worktree ",
        RemoveMode::ConfirmDirtyWorktree => " Confirm dirty worktree removal ",
        RemoveMode::ConfirmUnmergedBranch => " Confirm unmerged branch deletion ",
    };
    let block = active_panel(Some(title.to_string()), theme);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);
    let target = dialog.target();
    let mut lines = vec![
        metric_line("Worktree", &target.worktree, theme),
        metric_line(
            "Branch",
            target.branch.as_deref().unwrap_or("detached"),
            theme,
        ),
        Line::from(""),
    ];
    match dialog.mode() {
        RemoveMode::Review | RemoveMode::Ready => {
            lines.push(Line::from("The worktree directory will be removed."));
            if dialog.can_delete_branch() {
                let marker = if dialog.delete_branch() { "[x]" } else { "[ ]" };
                lines.push(focused_control_line(
                    &format!(
                        "{marker} Also delete local branch {}",
                        target.branch.as_deref().unwrap_or_default()
                    ),
                    inner.width,
                    theme,
                ));
            }
        }
        RemoveMode::ConfirmDirtyWorktree => {
            lines.push(Line::from("This worktree has uncommitted changes."));
            lines.push(Line::from("Press Enter to remove those changes."));
        }
        RemoveMode::ConfirmUnmergedBranch => {
            lines.push(Line::from("This local branch is not merged."));
            lines.push(Line::from("Press Enter to force branch deletion."));
        }
    }
    if let Some(error) = dialog.validation_error() {
        lines.push(Line::from(""));
        lines.push(error_line(error, theme));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
        inner,
    );
    let items = match dialog.mode() {
        RemoveMode::Review | RemoveMode::Ready if dialog.can_delete_branch() => vec![
            KeyHint::secondary("Space", "branch"),
            KeyHint::danger("Enter", "remove"),
            KeyHint::secondary("Esc", "close"),
            KeyHint::secondary("?", "help"),
        ],
        RemoveMode::Review | RemoveMode::Ready => vec![
            KeyHint::danger("Enter", "remove"),
            KeyHint::secondary("Esc", "close"),
            KeyHint::secondary("?", "help"),
        ],
        RemoveMode::ConfirmDirtyWorktree | RemoveMode::ConfirmUnmergedBranch => vec![
            KeyHint::danger("Enter", "confirm"),
            KeyHint::secondary("Esc", "back"),
            KeyHint::secondary("?", "help"),
        ],
    };
    render_dialog_keybar(frame, footer, theme, &items);
}

fn render_sync_dialog(
    dialog: &SyncDialog,
    spinner_tick: u64,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
) {
    let footer = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let content_area = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    match dialog.mode() {
        SyncMode::Form => {
            let modal = centered_rect(
                content_area.width.saturating_sub(8).min(76),
                12,
                content_area,
            );
            frame.render_widget(Clear, modal);
            let block = active_panel(Some(" Sync worktree ".to_string()), theme);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let mut lines = vec![
                metric_line("Worktree", dialog.worktree(), theme),
                metric_line("Branch", dialog.branch().unwrap_or("detached"), theme),
                Line::from(""),
                selector_line("Base", dialog.base().unwrap_or("Select a base"), theme),
                strategy_line(
                    matches!(
                        dialog.strategy(),
                        crate::cli::commands::sync::stateless::SyncStrategy::Rebase
                    ),
                    theme,
                ),
            ];
            if let Some(error) = dialog.validation_error() {
                lines.push(Line::from(""));
                lines.push(error_line(error, theme));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
                inner,
            );
            let submit = if dialog.submission().is_some() {
                KeyHint::primary("Enter", "sync")
            } else {
                KeyHint::disabled("Enter", "sync (select a base)")
            };
            render_dialog_keybar(
                frame,
                footer,
                theme,
                &[
                    KeyHint::secondary("Tab", "base"),
                    KeyHint::secondary("←/→", "mode"),
                    submit,
                    KeyHint::secondary("Esc", "close"),
                    KeyHint::secondary("?", "help"),
                ],
            );
        }
        SyncMode::BasePicker => {
            let modal = centered_rect(
                content_area.width.saturating_sub(6).min(86),
                content_area.height.saturating_sub(4).min(20),
                content_area,
            );
            frame.render_widget(Clear, modal);
            let block = active_panel(Some(" Sync worktree · Select base ".to_string()), theme);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [search_input, options] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            render_text_input(
                frame,
                search_input,
                "Search",
                dialog.base_input(),
                "Type to filter bases",
                theme,
            );
            let mut lines = Vec::new();
            if dialog.origin_spinner_visible() {
                lines.push(Line::from(format!(
                    "Updating origin {}",
                    flux_frame(spinner_tick)
                )));
            }
            if let Some(warning) = dialog.warning() {
                lines.push(Line::from(warning));
            }
            lines.push(Line::from(""));
            lines.extend(
                dialog
                    .base_candidates()
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| {
                        selectable_line(
                            &candidate.name,
                            index == dialog.base_selection(),
                            inner.width,
                            theme,
                        )
                    }),
            );
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
                options,
            );
            render_dialog_keybar(
                frame,
                footer,
                theme,
                &[
                    if dialog.base_candidates().is_empty() {
                        KeyHint::disabled("Enter", "select (no matches)")
                    } else {
                        KeyHint::primary("Enter", "select")
                    },
                    KeyHint::secondary("Esc", "back"),
                    KeyHint::secondary("?", "help"),
                ],
            );
        }
    }
}

fn render_resize(model: &ViewModel<'_>, frame: &mut Frame, theme: &Theme) {
    let [body, keybar] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(model.area);
    let message = vec![
        Line::from(Span::styled(
            "Resize terminal",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metric_line(
            "Current",
            &format!("{}×{}", model.area.width, model.area.height),
            theme,
        ),
        metric_line(
            "Minimum",
            &format!("{}×{}", Viewport::MIN_WIDTH, Viewport::MIN_HEIGHT),
            theme,
        ),
    ];
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg)),
        body,
    );
    render_dialog_keybar(frame, keybar, theme, &[KeyHint::secondary("q", "quit")]);
}

fn render_cockpit(model: &ViewModel<'_>, frame: &mut Frame, theme: &Theme) {
    let warning_height = u16::from(model.state.refresh.warning.is_some());
    let notification_height = u16::from(model.state.notification.is_some());
    let [body, warning, notification, keybar] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(warning_height),
        Constraint::Length(notification_height),
        Constraint::Length(1),
    ])
    .areas(model.area);
    let body = if model.state.search.is_some() {
        let [search, body] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(body);
        render_search(model, frame, search, theme);
        body
    } else {
        body
    };
    if model.inspector_visible() && model.is_wide() {
        let [list, inspector] =
            Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                .areas(body);
        render_list(model, frame, list, theme);
        render_inspector(model, frame, inspector, theme);
    } else if model.inspector_visible() {
        render_inspector(model, frame, body, theme);
    } else {
        render_list(model, frame, body, theme);
    }
    if let Some(message) = model.state.refresh.warning.as_deref() {
        frame.render_widget(
            Paragraph::new(message).style(
                theme.with_bg(
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                    theme.bg_elevated,
                ),
            ),
            warning,
        );
    }
    if let Some(notice) = model.state.notification.as_ref() {
        frame.render_widget(
            Paragraph::new(notice.text.clone()).style(
                theme.with_bg(
                    Style::default()
                        .fg(theme.selection_fg)
                        .add_modifier(Modifier::BOLD),
                    theme.accent_soft,
                ),
            ),
            notification,
        );
    }
    render_keybar(
        model.state,
        frame,
        keybar,
        theme,
        model.state.context(),
        !model.is_wide(),
    );
}

fn render_search(model: &ViewModel<'_>, frame: &mut Frame, area: Rect, theme: &Theme) {
    let query = model
        .state
        .search
        .as_ref()
        .expect("search surface requires active search");
    let result_count = model.visible().len();
    let result_label = if result_count == 1 {
        "1 result".to_string()
    } else {
        format!("{result_count} results")
    };
    let result_width = result_label.chars().count().saturating_add(2);
    let value_width = usize::from(area.width.saturating_sub(2))
        .saturating_sub(3)
        .saturating_sub(result_width);
    let mut content = vec![Span::styled(
        "> ",
        Style::default().fg(theme.accent),
    )];
    content.extend(input_value_spans(
        query.input(),
        "Type to filter worktrees",
        value_width,
        theme,
    ));
    content.push(Span::styled(
        format!("  {result_label}"),
        Style::default().fg(theme.fg_muted),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_active))
        .title(" Search · typing ")
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(theme.with_bg(Style::default(), theme.control_bg));
    frame.render_widget(
        Paragraph::new(Line::from(content))
        .block(block)
        .style(theme.with_bg(Style::default().fg(theme.fg), theme.control_bg)),
        area,
    );
}

fn render_create_dialog(
    dialog: &CreateDialog,
    spinner_tick: u64,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
) {
    let footer = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let content_area = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    match dialog.mode() {
        CreateMode::Form => {
            let modal = centered_rect(
                content_area.width.saturating_sub(8).min(76),
                13,
                content_area,
            );
            frame.render_widget(Clear, modal);
            let block = active_panel(Some(" Create worktree ".to_string()), theme);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [branch_input, details] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            render_text_input(
                frame,
                branch_input,
                "Branch",
                dialog.branch_input(),
                "Type a branch name",
                theme,
            );
            let mut lines = Vec::new();
            if let Some(preview) = dialog.preview() {
                if preview.base_visible {
                    lines.push(selector_line(
                        "Create from",
                        preview.base.as_deref().unwrap_or("Select a base"),
                        theme,
                    ));
                }
                lines.extend([
                    metric_line("Worktree", &preview.worktree, theme),
                    metric_line("Path", &preview.path.to_string_lossy(), theme),
                ]);
            }
            if let Some(error) = dialog.validation_error() {
                lines.push(Line::from(""));
                lines.push(error_line(error, theme));
            }
            let suggestions = dialog.branch_suggestions();
            if !suggestions.is_empty() {
                lines.push(Line::from(""));
                lines.extend(
                    suggestions
                        .iter()
                        .enumerate()
                        .take(3)
                        .map(|(index, suggestion)| {
                            selectable_line(
                                &suggestion.label,
                                index == dialog.branch_selection(),
                                details.width,
                                theme,
                            )
                        }),
                );
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
                details,
            );
            let has_base = dialog.preview().is_some_and(|preview| preview.base_visible);
            let submit = if dialog.submission().is_some() {
                KeyHint::primary("Enter", "create")
            } else {
                KeyHint::disabled("Enter", "create (branch required)")
            };
            let mut items = vec![submit];
            if has_base {
                items.push(KeyHint::secondary("Tab", "base"));
            }
            items.extend([
                KeyHint::secondary("Esc", "close"),
                KeyHint::secondary("?", "help"),
            ]);
            render_dialog_keybar(frame, footer, theme, &items);
        }
        CreateMode::BasePicker => {
            let modal = centered_rect(
                content_area.width.saturating_sub(6).min(86),
                content_area.height.saturating_sub(4).min(20),
                content_area,
            );
            frame.render_widget(Clear, modal);
            let block = active_panel(Some(" Create worktree · Select base ".to_string()), theme);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [search_input, options] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            render_text_input(
                frame,
                search_input,
                "Search",
                dialog.base_input(),
                "Type to filter bases",
                theme,
            );
            let mut lines = Vec::new();
            if dialog.origin_spinner_visible() {
                lines.push(Line::from(format!(
                    "Updating origin {}",
                    flux_frame(spinner_tick)
                )));
            }
            if let Some(warning) = dialog.warning() {
                lines.push(Line::from(warning));
            }
            lines.push(Line::from(""));
            lines.extend(
                dialog
                    .base_candidates()
                    .iter()
                    .enumerate()
                    .map(|(index, candidate)| {
                        selectable_line(
                            &candidate.name,
                            index == dialog.base_selection(),
                            inner.width,
                            theme,
                        )
                    }),
            );
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
                options,
            );
            render_dialog_keybar(
                frame,
                footer,
                theme,
                &[
                    if dialog.base_candidates().is_empty() {
                        KeyHint::disabled("Enter", "select (no matches)")
                    } else {
                        KeyHint::primary("Enter", "select")
                    },
                    KeyHint::secondary("Esc", "back"),
                    KeyHint::secondary("?", "help"),
                ],
            );
        }
    }
}

fn render_text_input(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    input: &LineInput,
    placeholder: &str,
    theme: &Theme,
) {
    let value_width = usize::from(area.width.saturating_sub(2)).saturating_sub(3);
    let mut content = vec![Span::styled(
        "> ",
        Style::default().fg(theme.accent),
    )];
    content.extend(input_value_spans(input, placeholder, value_width, theme));
    let content = Line::from(content);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_active))
        .title(format!(" {label} · typing "))
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(theme.with_bg(Style::default(), theme.control_bg));
    frame.render_widget(
        Paragraph::new(content)
            .block(block)
            .style(theme.with_bg(Style::default(), theme.control_bg)),
        area,
    );
}

fn input_value_spans(
    input: &LineInput,
    placeholder: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if input.value().is_empty() {
        return vec![
            Span::styled(
                tail_ellipsize(placeholder, width),
                Style::default().fg(theme.fg_muted),
            ),
            Span::styled("▌", Style::default().fg(theme.accent)),
        ];
    }
    let window = input.window(width);
    vec![
        Span::styled(window.before_cursor, Style::default().fg(theme.fg)),
        Span::styled("▌", Style::default().fg(theme.accent)),
        Span::styled(window.after_cursor, Style::default().fg(theme.fg)),
    ]
}

fn tail_ellipsize(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let budget = width - 1;
    let mut used = 0;
    let mut tail = Vec::new();
    for grapheme in value.graphemes(true).rev() {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if used + grapheme_width > budget {
            break;
        }
        tail.push(grapheme);
        used += grapheme_width;
    }
    tail.reverse();
    format!("…{}", tail.concat())
}

fn selector_line(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<14}"),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("[ {value}  ▾ ]"),
            theme.with_bg(
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                theme.control_bg,
            ),
        ),
    ])
}

fn strategy_line(rebase_selected: bool, theme: &Theme) -> Line<'static> {
    let selected = |label: &str| {
        Span::styled(
            format!("[● {label}]"),
            theme.with_bg(
                Style::default()
                    .fg(theme.selection_fg)
                    .add_modifier(Modifier::BOLD),
                theme.selection_bg,
            ),
        )
    };
    let idle = |label: &str| {
        Span::styled(
            format!("[○ {label}]"),
            theme.with_bg(Style::default().fg(theme.fg), theme.control_bg),
        )
    };
    let (rebase, merge) = if rebase_selected {
        (selected("Rebase"), idle("Merge"))
    } else {
        (idle("Rebase"), selected("Merge"))
    };
    Line::from(vec![
        Span::styled(
            format!("{:<14}", "Strategy"),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        rebase,
        Span::raw("  "),
        merge,
        Span::styled("  ←/→", Style::default().fg(theme.fg_muted)),
    ])
}

fn focused_control_line(label: &str, width: u16, theme: &Theme) -> Line<'static> {
    let content = format!("› {label}");
    let padding = usize::from(width).saturating_sub(content.chars().count());
    Line::from(Span::styled(
        format!("{content}{}", " ".repeat(padding)),
        theme.with_bg(
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            theme.control_bg,
        ),
    ))
}

fn selectable_line(label: &str, selected: bool, width: u16, theme: &Theme) -> Line<'static> {
    let marker = if selected { "›" } else { " " };
    let content = format!("{marker} {label}");
    let padding = usize::from(width).saturating_sub(content.chars().count());
    let style = if selected {
        theme.with_bg(
            Style::default()
                .fg(theme.selection_fg)
                .add_modifier(Modifier::BOLD),
            theme.selection_bg,
        )
    } else {
        Style::default().fg(theme.fg)
    };
    Line::from(Span::styled(
        format!("{content}{}", " ".repeat(padding)),
        style,
    ))
}

fn render_operation_modal(modal: &OperationModal, frame: &mut Frame, area: Rect, theme: &Theme) {
    let footer = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let content_area = Rect {
        height: area.height.saturating_sub(1),
        ..area
    };
    let dialog = centered_rect(
        content_area.width.saturating_sub(8).min(78),
        content_area.height.saturating_sub(4).min(18),
        content_area,
    );
    frame.render_widget(Clear, dialog);
    let title = match modal.operation() {
        OperationKind::Create => " Create worktree ",
        OperationKind::Sync => " Sync worktree ",
        OperationKind::Remove => " Remove worktree ",
    };
    let block = active_panel(Some(title.to_string()), theme);
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    let mut lines = vec![metric_line(
        "Elapsed",
        &format_elapsed(modal.elapsed()),
        theme,
    )];
    for stage in modal.stages() {
        let (marker, style) =
            if modal.current_stage() == Some(stage.stage) && modal.spinner_visible() {
                (
                    flux_frame(modal.spinner_tick()).to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else if stage.success == Some(true) {
                (
                    "✓".to_string(),
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD),
                )
            } else if stage.success == Some(false) {
                (
                    "×".to_string(),
                    Style::default()
                        .fg(theme.error)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("·".to_string(), Style::default().fg(theme.fg_muted))
            };
        lines.push(Line::from(Span::styled(
            format!(
                "{marker} {}",
                operation_stage_label(modal.operation(), stage.stage)
            ),
            style,
        )));
    }
    for warning in modal.warnings() {
        lines.push(Line::from(Span::styled(
            format!("Warning: {warning}"),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let hook_height = usize::from(inner.height.saturating_sub(lines.len() as u16 + 3));
    if !modal.visible_hook_lines(hook_height).is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("Hook output"));
        lines.extend(
            modal
                .visible_hook_lines(hook_height)
                .into_iter()
                .map(|line| Line::from(format!("  {line}"))),
        );
    }
    if let ModalStatus::Failed {
        stage,
        mutation_state,
        message,
        retained_quarantine,
    } = modal.status()
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "Error: Failed at {}: {message}",
                operation_stage_label(modal.operation(), *stage)
            ),
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(format!(
            "Mutation: {}",
            mutation_state_label(*mutation_state)
        )));
        if let Some(path) = retained_quarantine {
            lines.push(Line::from(format!(
                "Retained quarantine: {}",
                path.display()
            )));
            lines.push(Line::from("Recovery is required before retrying."));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
        inner,
    );
    let items = match modal.status() {
        ModalStatus::Failed {
            mutation_state:
                crate::operation::MutationState::NotStarted
                | crate::operation::MutationState::RolledBack,
            ..
        } => vec![
            KeyHint::secondary("↑/↓", "output"),
            KeyHint::primary("Enter", "back"),
            KeyHint::secondary("?", "help"),
        ],
        ModalStatus::Failed { .. } => vec![
            KeyHint::secondary("↑/↓", "output"),
            KeyHint::secondary("?", "help"),
        ],
        ModalStatus::Running if !modal.mutation_started() => vec![
            KeyHint::secondary("↑/↓", "output"),
            KeyHint::secondary("Esc", "cancel"),
            KeyHint::secondary("?", "help"),
        ],
        ModalStatus::Running | ModalStatus::Succeeded => vec![
            KeyHint::secondary("↑/↓", "output"),
            KeyHint::secondary("?", "help"),
        ],
    };
    render_dialog_keybar(frame, footer, theme, &items);
}

fn mutation_state_label(state: crate::operation::MutationState) -> &'static str {
    match state {
        crate::operation::MutationState::NotStarted => "not started",
        crate::operation::MutationState::Applied => "applied",
        crate::operation::MutationState::RolledBack => "rolled back",
        crate::operation::MutationState::PartiallyApplied => "partially applied",
    }
}

#[derive(Clone, Copy)]
enum KeyTone {
    Secondary,
    Primary,
    Danger,
    Disabled,
}

#[derive(Clone, Copy)]
struct KeyHint<'a> {
    key: &'a str,
    action: &'a str,
    tone: KeyTone,
}

impl<'a> KeyHint<'a> {
    const fn secondary(key: &'a str, action: &'a str) -> Self {
        Self {
            key,
            action,
            tone: KeyTone::Secondary,
        }
    }

    const fn primary(key: &'a str, action: &'a str) -> Self {
        Self {
            key,
            action,
            tone: KeyTone::Primary,
        }
    }

    const fn danger(key: &'a str, action: &'a str) -> Self {
        Self {
            key,
            action,
            tone: KeyTone::Danger,
        }
    }

    const fn disabled(key: &'a str, action: &'a str) -> Self {
        Self {
            key,
            action,
            tone: KeyTone::Disabled,
        }
    }
}

fn render_dialog_keybar(frame: &mut Frame, area: Rect, theme: &Theme, items: &[KeyHint<'_>]) {
    let mut visible = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                index,
                format!(
                    "[{}] {}",
                    item.key,
                    compact_key_action(item.action, area.width)
                ),
            )
        })
        .collect::<Vec<_>>();
    while keybar_width(&visible) > usize::from(area.width) {
        let Some(position) = visible.iter().position(|(index, _)| {
            let item = &items[*index];
            matches!(item.tone, KeyTone::Secondary) && !matches!(item.key, "Esc" | "?")
        }) else {
            break;
        };
        visible.remove(position);
    }
    let mut spans = Vec::new();
    for (position, (index, label)) in visible.into_iter().enumerate() {
        if position > 0 {
            spans.push(Span::raw("  "));
        }
        let item = &items[index];
        let style = match item.tone {
            KeyTone::Secondary => theme.with_bg(
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                theme.control_bg,
            ),
            KeyTone::Primary => theme.with_bg(
                Style::default()
                    .fg(theme.primary_fg)
                    .add_modifier(Modifier::BOLD),
                theme.primary_bg,
            ),
            KeyTone::Danger => theme.with_bg(
                Style::default()
                    .fg(theme.danger_fg)
                    .add_modifier(Modifier::BOLD),
                theme.danger_bg,
            ),
            KeyTone::Disabled => Style::default()
                .fg(theme.disabled_fg)
                .add_modifier(Modifier::DIM),
        };
        spans.push(Span::styled(label, style));
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
        area,
    );
}

fn compact_key_action(action: &str, width: u16) -> &str {
    if width > Viewport::MIN_WIDTH {
        return action;
    }
    match action {
        "create (branch required)" => "create unavailable",
        "sync (select a base)" => "sync unavailable",
        "select (no matches)" => "select unavailable",
        value => value,
    }
}

fn keybar_width(items: &[(usize, String)]) -> usize {
    items
        .iter()
        .map(|(_, label)| UnicodeWidthStr::width(label.as_str()))
        .sum::<usize>()
        + items.len().saturating_sub(1) * 2
}

fn operation_stage_label(operation: OperationKind, stage: OperationStage) -> &'static str {
    match stage {
        OperationStage::Fetch => "Fetch origin",
        OperationStage::Revalidate => "Revalidate",
        OperationStage::PreHook => match operation {
            OperationKind::Create => "Pre-create hook",
            OperationKind::Sync => "Pre-sync hook",
            OperationKind::Remove => "Pre-remove hook",
        },
        OperationStage::CreateWorktree => "Create worktree",
        OperationStage::Sync => "Sync worktree",
        OperationStage::RemoveWorktree => "Remove worktree",
        OperationStage::Prune => "Prune worktrees",
        OperationStage::DeleteBranch => "Delete branch",
        OperationStage::PostHook => match operation {
            OperationKind::Create => "Post-create hook",
            OperationKind::Sync => "Post-sync hook",
            OperationKind::Remove => "Post-remove hook",
        },
        OperationStage::Rollback => "Rollback",
    }
}

fn format_elapsed(duration: std::time::Duration) -> String {
    format!("{:.1}s", duration.as_secs_f64())
}

fn render_list(model: &ViewModel<'_>, frame: &mut Frame, area: Rect, theme: &Theme) {
    let visible = model.visible();
    let result_count = visible.len();
    let title = if model.state.refresh.updating_refs {
        format!(
            " Worktrees · {} · Updating refs {} ",
            result_count,
            flux_frame(model.state.refresh.spinner_tick)
        )
    } else {
        format!(" Worktrees · {result_count} ")
    };
    let header = Row::new(["Worktree", "Branch", "Git"])
        .style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .height(1);
    let rows = visible.iter().map(|identity| {
        let current = if identity.is_current { "* " } else { "" };
        Row::new([
            Cell::from(format!("{current}{}", identity.worktree)),
            Cell::from(
                identity
                    .branch
                    .clone()
                    .unwrap_or_else(|| "detached".to_string()),
            ),
            Cell::from(row_git(model, identity)),
        ])
        .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(31),
            Constraint::Percentage(34),
            Constraint::Percentage(35),
        ],
    )
    .header(header)
    .block(panel(Some(title), theme))
    .row_highlight_style(
        theme.with_bg(
            Style::default()
                .fg(theme.selection_fg)
                .add_modifier(Modifier::BOLD),
            theme.selection_bg,
        ),
    )
    .highlight_symbol("› ")
    .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel));
    let mut table_state = TableState::default();
    let selected = model
        .state
        .selected
        .as_ref()
        .and_then(|id| visible.iter().position(|row| &row.id == id));
    table_state.select(selected);
    frame.render_stateful_widget(table, area, &mut table_state);
    if visible.is_empty() {
        let empty = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(2),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(3),
        };
        frame.render_widget(
            Paragraph::new("No matching worktrees")
                .alignment(Alignment::Center)
                .style(theme.with_bg(Style::default().fg(theme.fg_muted), theme.bg_panel)),
            empty,
        );
    }
}

fn row_git(model: &ViewModel<'_>, identity: &WorktreeIdentity) -> String {
    let status = model.status_for(identity);
    if model.state.row_is_waiting(&identity.id) {
        return match status {
            Some(status) => format!(
                "{} {}",
                flux_frame(model.state.refresh.spinner_tick),
                compact_git(status)
            ),
            None => format!("{} Updating", flux_frame(model.state.refresh.spinner_tick)),
        };
    }
    status.map(compact_git).unwrap_or_else(|| "—".to_string())
}

fn flux_frame(tick: u64) -> char {
    let frames = FluxFrames::BRAILLE;
    let frame_count = u64::try_from(frames.len()).expect("Flux frames fit in u64");
    let index = usize::try_from(tick % frame_count).expect("Flux frame index fits in usize");
    frames[index]
}

fn render_inspector(model: &ViewModel<'_>, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = panel(None, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(identity) = model.selected() else {
        frame.render_widget(
            Paragraph::new("No worktree selected")
                .alignment(Alignment::Center)
                .style(theme.with_bg(Style::default().fg(theme.fg_muted), theme.bg_panel)),
            inner,
        );
        return;
    };
    let status = model.status_for(identity);
    let identity_label = if model.state.row_is_waiting(&identity.id) {
        format!(
            "{} {}",
            flux_frame(model.state.refresh.spinner_tick),
            identity.worktree
        )
    } else {
        identity.worktree.clone()
    };
    let mut lines = vec![Line::from(Span::styled(
        identity_label,
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ))];
    if identity.is_current {
        lines.push(Line::from(Span::styled(
            " current ",
            theme.with_bg(
                Style::default()
                    .fg(theme.selection_fg)
                    .add_modifier(Modifier::BOLD),
                theme.accent_soft,
            ),
        )));
    }
    if model.state.refresh.updating_refs {
        lines.push(Line::from(Span::styled(
            format!(
                "Updating refs {}",
                flux_frame(model.state.refresh.spinner_tick)
            ),
            Style::default().fg(theme.fg_muted),
        )));
    }
    lines.extend([
        Line::from(""),
        metric_line(
            "Branch",
            identity.branch.as_deref().unwrap_or("detached"),
            theme,
        ),
        metric_line("Path", &identity.path.to_string_lossy(), theme),
        metric_line(
            "HEAD",
            &identity
                .head
                .as_deref()
                .map(short_head)
                .unwrap_or_else(|| "—".to_string()),
            theme,
        ),
        metric_line("Staged", &count(status.map(|value| value.staged)), theme),
        metric_line(
            "Modified",
            &count(status.map(|value| value.modified)),
            theme,
        ),
        metric_line(
            "Untracked",
            &count(status.map(|value| value.untracked)),
            theme,
        ),
        compare_line(status, theme),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_panel)),
        inner,
    );
}

fn render_keybar(
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    context: Context,
    narrow: bool,
) {
    let bindings: Vec<_> = keymap::keybar_bindings(context, narrow)
        .into_iter()
        .filter(|binding| unavailable_reason(state, binding.action).is_none())
        .collect();
    let mut spans = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            binding.label,
            Style::default()
                .fg(theme.selection_fg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", binding.description),
            Style::default().fg(theme.fg_muted),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .style(theme.with_bg(Style::default().fg(theme.fg), theme.bg_elevated)),
        area,
    );
}

fn render_help(model: &ViewModel<'_>, frame: &mut Frame, theme: &Theme) {
    let context = model.state.context();
    let bindings = keymap::bindings(context);
    let two_columns = model.is_wide();
    let width = if two_columns {
        model.area.width.saturating_sub(8).min(96)
    } else {
        model.area.width.saturating_sub(4).min(54)
    };
    let rows = if two_columns {
        bindings.len().div_ceil(2)
    } else {
        bindings.len()
    };
    let height = (rows as u16 + 4).min(model.area.height.saturating_sub(2));
    let dialog = centered_rect(width, height, model.area);
    frame.render_widget(Clear, dialog);
    let title = match context {
        Context::Cockpit => " Help · Worktrees ",
        Context::Search => " Help · Search ",
        Context::Resize => " Help · Resize ",
    };
    let block = active_panel(Some(title.to_string()), theme);
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    if two_columns {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(inner);
        let split = bindings.len().div_ceil(2);
        render_help_column(model.state, frame, left, theme, &bindings[..split]);
        render_help_column(model.state, frame, right, theme, &bindings[split..]);
    } else {
        render_help_column(model.state, frame, inner, theme, bindings);
    }
}

fn render_overlay_help(state: &AppState, frame: &mut Frame, area: Rect, theme: &Theme) {
    let (title, items): (&str, &[(&str, &str)]) =
        if let Some(modal) = state.operation_modal.as_ref() {
            let items = match modal.status() {
                ModalStatus::Failed {
                    mutation_state:
                        crate::operation::MutationState::NotStarted
                        | crate::operation::MutationState::RolledBack,
                    ..
                } => &[
                    ("↑/↓", "scroll output"),
                    ("Enter", "return to form"),
                    ("?", "close help"),
                ][..],
                ModalStatus::Failed { .. } => &[("↑/↓", "scroll output"), ("?", "close help")][..],
                ModalStatus::Running if !modal.mutation_started() => &[
                    ("↑/↓", "scroll output"),
                    ("Esc", "cancel"),
                    ("?", "close help"),
                ][..],
                ModalStatus::Running | ModalStatus::Succeeded => {
                    &[("↑/↓", "scroll output"), ("?", "close help")][..]
                }
            };
            (" Help · Operation ", items)
        } else if state
            .sync_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.mode() == SyncMode::BasePicker)
        {
            (
                " Help · Sync worktree ",
                &[
                    ("type", "search bases"),
                    ("↑/↓", "select base"),
                    ("Enter", "select base"),
                    ("Esc", "back"),
                    ("?", "close help"),
                ],
            )
        } else if state.sync_dialog.is_some() {
            (
                " Help · Sync worktree ",
                &[
                    ("Tab", "select base"),
                    ("←/→", "select strategy"),
                    ("Enter", "sync worktree"),
                    ("Esc", "close"),
                    ("?", "close help"),
                ],
            )
        } else if let Some(dialog) = state.remove_dialog.as_ref() {
            let items = match dialog.mode() {
                RemoveMode::Review | RemoveMode::Ready if dialog.can_delete_branch() => &[
                    ("Space", "toggle local branch deletion"),
                    ("Enter", "remove worktree"),
                    ("Esc", "close"),
                    ("?", "close help"),
                ][..],
                RemoveMode::Review | RemoveMode::Ready => &[
                    ("Enter", "remove worktree"),
                    ("Esc", "close"),
                    ("?", "close help"),
                ][..],
                RemoveMode::ConfirmDirtyWorktree | RemoveMode::ConfirmUnmergedBranch => &[
                    ("Enter", "confirm removal"),
                    ("Esc", "back"),
                    ("?", "close help"),
                ][..],
            };
            (" Help · Remove worktree ", items)
        } else if state
            .create_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.mode() == CreateMode::BasePicker)
        {
            (
                " Help · Create worktree ",
                &[
                    ("type", "search bases"),
                    ("↑/↓", "select base"),
                    ("Enter", "select base"),
                    ("Esc", "back"),
                    ("?", "close help"),
                ],
            )
        } else {
            (
                " Help · Create worktree ",
                &[
                    ("type", "search branches"),
                    ("↑/↓", "select suggestion"),
                    ("Tab", "select base"),
                    ("Enter", "create or navigate"),
                    ("Esc", "close"),
                    ("?", "close help"),
                ],
            )
        };
    let dialog = centered_rect(
        52.min(area.width.saturating_sub(4)),
        items.len() as u16 + 4,
        area,
    );
    frame.render_widget(Clear, dialog);
    let block = active_panel(Some(title.to_string()), theme);
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    let lines = items.iter().map(|(key, description)| {
        Line::from(vec![
            Span::styled(
                format!("{key:<10}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*description, Style::default().fg(theme.fg)),
        ])
    });
    frame.render_widget(
        Paragraph::new(lines.collect::<Vec<_>>())
            .wrap(Wrap { trim: true })
            .style(theme.with_bg(Style::default(), theme.bg_elevated)),
        inner,
    );
}

fn render_help_column(
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    bindings: &[Binding],
) {
    let lines = bindings.iter().map(|binding| {
        let reason = unavailable_reason(state, binding.action);
        let description = reason.unwrap_or(binding.description);
        let unavailable = reason.is_some();
        let key_style = if unavailable {
            Style::default()
                .fg(theme.disabled_fg)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        };
        let description_style = if unavailable {
            Style::default()
                .fg(theme.disabled_fg)
                .add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(theme.fg)
        };
        Line::from(vec![
            Span::styled(format!("{:<8}", binding.label), key_style),
            Span::styled(description.to_string(), description_style),
        ])
    });
    frame.render_widget(
        Paragraph::new(lines.collect::<Vec<_>>())
            .wrap(Wrap { trim: true })
            .style(theme.with_bg(Style::default(), theme.bg_elevated)),
        area,
    );
}

fn panel(title: Option<String>, theme: &Theme) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(theme.with_bg(Style::default(), theme.bg_panel));
    match title {
        Some(title) => block.title(title),
        None => block,
    }
}

fn active_panel(title: Option<String>, theme: &Theme) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_active))
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(theme.with_bg(Style::default(), theme.bg_elevated));
    match title {
        Some(title) => block.title(title),
        None => block,
    }
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

fn compact_git(status: &WorktreeStatus) -> String {
    let changed = status.staged + status.modified + status.untracked;
    format!(
        "{changed} changed · ↑{} ↓{}",
        optional_number(status.ahead),
        optional_number(status.behind)
    )
}

fn compare_line(status: Option<&WorktreeStatus>, theme: &Theme) -> Line<'static> {
    let base = status
        .and_then(|value| value.base.as_deref())
        .unwrap_or("—");
    let value = format!(
        "↑{} ↓{}",
        optional_number(status.and_then(|value| value.ahead)),
        optional_number(status.and_then(|value| value.behind))
    );
    metric_line(&format!("Compare {base}"), &value, theme)
}

fn optional_number(value: Option<usize>) -> String {
    value.map_or_else(|| "—".to_string(), |value| value.to_string())
}

fn count(value: Option<u32>) -> String {
    value.map_or_else(|| "—".to_string(), |value| value.to_string())
}

fn short_head(head: &str) -> String {
    head.chars().take(7).collect()
}

fn metric_line(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<14}"),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(theme.fg)),
    ])
}

fn error_line(message: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("Error: {message}"),
        Style::default()
            .fg(theme.error)
            .add_modifier(Modifier::BOLD),
    ))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ratatui::{backend::TestBackend, buffer::Buffer, style::Color, Terminal};

    use super::*;
    use crate::tui::app::{reduce, Event, WorktreeId};

    fn identity(
        path: &str,
        worktree: &str,
        branch: Option<&str>,
        is_main: bool,
        is_current: bool,
    ) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new(path),
            worktree: worktree.to_string(),
            branch: branch.map(str::to_string),
            path: PathBuf::from(path),
            head: Some("1234567890abcdef".to_string()),
            is_main,
            is_current,
            detached: branch.is_none(),
        }
    }

    fn sample_state() -> AppState {
        let current = identity(
            "/worktrees/feature-auth",
            "feature-auth",
            Some("feature/auth"),
            false,
            true,
        );
        let main = identity("/repos/trench", "trench", Some("main"), true, false);
        let mut state = AppState::new(vec![current.clone(), main]);
        state.statuses.insert(
            current.id,
            WorktreeStatus {
                base: Some("main".to_string()),
                staged: 1,
                modified: 1,
                untracked: 1,
                ahead: Some(2),
                behind: Some(1),
            },
        );
        state
    }

    fn render_buffer(state: &mut AppState, width: u16, height: u16, theme_name: &str) -> Buffer {
        let _ = reduce(state, Event::ViewportChanged { width, height });
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::from_name(theme_name);
        terminal
            .draw(|frame| render(state, frame, frame.area(), &theme))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn text(buffer: &Buffer) -> String {
        lines(buffer).join("\n")
    }

    fn find_text(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let width = u16::try_from(needle.chars().count()).expect("test text fits in u16");
        for y in buffer.area.y..buffer.area.bottom() {
            for x in buffer.area.x..buffer.area.right().saturating_sub(width) {
                let candidate = (x..x + width)
                    .map(|column| buffer.cell((column, y)).unwrap().symbol())
                    .collect::<String>();
                if candidate == needle {
                    return (x, y);
                }
            }
        }
        panic!("missing {needle:?}\n{}", text(buffer));
    }

    #[test]
    fn resize_view_uses_strict_60_by_16_boundary() {
        let mut state = sample_state();
        let width_tiny = text(&render_buffer(&mut state, 59, 16, "ops"));
        let height_tiny = text(&render_buffer(&mut state, 60, 15, "ops"));
        let exact_minimum = text(&render_buffer(&mut state, 60, 16, "ops"));

        assert!(width_tiny.contains("Resize terminal"), "{width_tiny}");
        assert!(width_tiny.contains("Current       59×16"), "{width_tiny}");
        assert!(width_tiny.contains("Minimum       60×16"), "{width_tiny}");
        assert!(width_tiny.lines().last().unwrap().contains("[q] quit"));
        assert!(!width_tiny.lines().last().unwrap().contains("help"));
        assert!(height_tiny.contains("Resize terminal"), "{height_tiny}");
        assert!(exact_minimum.contains("Worktrees · 2"), "{exact_minimum}");
        assert!(
            !exact_minimum.contains("Resize terminal"),
            "{exact_minimum}"
        );
    }

    #[test]
    fn tiny_view_suppresses_dialogs_and_help_until_the_terminal_is_usable() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        state.create_dialog = Some(CreateDialog::new(
            "trench",
            Path::new("/worktrees"),
            refs,
            [],
        ));
        state.help_open = true;

        let output = text(&render_buffer(&mut state, 59, 16, "ops"));
        assert!(output.contains("Resize terminal"), "{output}");
        assert!(!output.contains("Create worktree"), "{output}");
        assert!(!output.contains("Help ·"), "{output}");
    }

    #[test]
    fn help_replaces_the_footer_and_dims_the_background() {
        let mut state = sample_state();
        state.help_open = true;
        let theme = crate::tui::theme::from_name("ops");

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();

        assert_eq!(footer, "[?] close help");
        assert_eq!(buffer.cell((0, 0)).unwrap().fg, theme.disabled_fg);
    }

    #[test]
    fn unavailable_help_binding_mutes_both_the_key_and_reason() {
        let main = identity("/repos/trench", "trench", Some("main"), true, true);
        let id = main.id.clone();
        let mut state = AppState::new(vec![main]);
        state.statuses.insert(id, WorktreeStatus::default());
        state.help_open = true;
        let theme = crate::tui::theme::from_name("ops");

        let buffer = render_buffer(&mut state, 120, 20, "ops");
        let binding = find_text(&buffer, "d       The main worktree cannot be removed");
        let key = buffer.cell(binding).unwrap();
        let reason = buffer.cell((binding.0 + 8, binding.1)).unwrap();

        assert_eq!(key.fg, theme.disabled_fg);
        assert_eq!(reason.fg, theme.disabled_fg);
        assert!(key.modifier.contains(Modifier::DIM));
    }

    #[test]
    fn wide_cockpit_contains_only_the_frozen_columns_and_inspector_fields() {
        let mut state = sample_state();
        let output = text(&render_buffer(&mut state, 120, 24, "catppuccin"));

        for expected in [
            "Worktrees · 2",
            "Worktree",
            "Branch",
            "Git",
            "feature-auth",
            "feature/auth",
            "3 changed · ↑2 ↓1",
            "Path",
            "/worktrees/feature-auth",
            "HEAD          1234567",
            "Staged        1",
            "Modified      1",
            "Untracked     1",
            "Compare main  ↑2 ↓1",
            "current",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
        for removed in [
            "Worktree Cockpit",
            "watch on",
            "Processes",
            "Procs",
            "Detail",
        ] {
            assert!(!output.contains(removed), "found {removed:?}\n{output}");
        }
    }

    #[test]
    fn narrow_layout_hides_inspector_until_i_toggles_it_for_the_run() {
        let mut state = sample_state();
        let hidden = text(&render_buffer(&mut state, 80, 20, "ops"));
        assert!(hidden.contains("Worktrees · 2"), "{hidden}");
        assert!(!hidden.contains("/worktrees/feature-auth"), "{hidden}");

        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('i')));
        let visible = text(&render_buffer(&mut state, 80, 20, "ops"));
        assert!(visible.contains("/worktrees/feature-auth"), "{visible}");
        assert!(!visible.contains("Worktrees · 2"), "{visible}");
    }

    #[test]
    fn keybars_end_in_help_and_only_expose_eligible_row_actions() {
        let main = identity("/repos/trench", "trench", Some("main"), true, true);
        let id = main.id.clone();
        let mut state = AppState::new(vec![main]);
        state.statuses.insert(id, WorktreeStatus::default());
        let buffer = render_buffer(&mut state, 120, 20, "ops");
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();

        assert!(footer.ends_with("? help"), "{footer}");
        assert!(!footer.contains("d remove"), "{footer}");
        assert!(footer.contains("s sync"), "{footer}");

        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('?')));
        let help = text(&render_buffer(&mut state, 120, 20, "ops"));
        assert!(
            help.contains("The main worktree cannot be removed"),
            "{help}"
        );
    }

    #[test]
    fn narrow_keybar_omits_but_keeps_hidden_shortcuts_routable() {
        let mut state = sample_state();
        let buffer = render_buffer(&mut state, 80, 20, "ops");
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();

        assert!(footer.contains("Enter switch"), "{footer}");
        assert!(footer.contains("c create"), "{footer}");
        assert!(footer.contains("/ search"), "{footer}");
        assert!(footer.ends_with("? help"), "{footer}");
        assert!(!footer.contains("o open"), "{footer}");

        let effects = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('o')));
        assert!(matches!(
            effects.as_slice(),
            [crate::tui::app::Effect::Open(_)]
        ));
    }

    #[test]
    fn contextual_help_uses_two_columns_wide_and_one_column_narrow() {
        let mut state = sample_state();
        state.help_open = true;
        let wide = lines(&render_buffer(&mut state, 120, 24, "ops"));
        let narrow = lines(&render_buffer(&mut state, 80, 24, "ops"));
        let wide_enter = wide.iter().position(|line| line.contains("Enter")).unwrap();
        let wide_refresh = wide
            .iter()
            .position(|line| line.contains("r       refresh"))
            .unwrap();
        let narrow_enter = narrow
            .iter()
            .position(|line| line.contains("Enter"))
            .unwrap();
        let narrow_refresh = narrow
            .iter()
            .position(|line| line.contains("r       refresh"))
            .unwrap();

        assert_eq!(wide_enter, wide_refresh, "{}", wide.join("\n"));
        assert_ne!(narrow_enter, narrow_refresh, "{}", narrow.join("\n"));
    }

    #[test]
    fn selected_row_uses_the_configured_theme() {
        let mut state = sample_state();
        let buffer = render_buffer(&mut state, 80, 20, "catppuccin");
        let theme = crate::tui::theme::from_name("catppuccin");
        let output = text(&buffer);
        let selected_cells = buffer
            .content()
            .iter()
            .filter(|cell| cell.bg == theme.selection_bg)
            .count();

        assert!(selected_cells > 0);
        assert_ne!(theme.selection_bg, Color::Reset);
        assert!(output.contains("› * feature-auth"), "{output}");
    }

    #[test]
    fn selected_sync_strategy_has_a_distinct_surface_in_every_theme() {
        use crate::{ref_catalog::RefSnapshot, tui::sync_flow::SyncDialog};

        for theme_name in ["ops", "catppuccin", "gruvbox", "minimal"] {
            let mut state = sample_state();
            let target = state.identities[1].clone();
            let refs = RefSnapshot::from_parts(
                ["main", "release"],
                ["origin/main"],
                Some("origin/main"),
                Some("main"),
                true,
            );
            state.sync_dialog = Some(SyncDialog::new(&target, refs, Some("release")));
            let theme = crate::tui::theme::from_name(theme_name);
            let buffer = render_buffer(&mut state, 100, 24, theme_name);
            let selected = find_text(&buffer, "[● Rebase]");
            let idle = find_text(&buffer, "[○ Merge]");

            assert_eq!(buffer.cell(selected).unwrap().bg, theme.selection_bg);
            assert_eq!(buffer.cell(idle).unwrap().bg, theme.control_bg);
            assert_ne!(
                buffer.cell(selected).unwrap().bg,
                buffer.cell(idle).unwrap().bg,
                "{theme_name} rendered selected and idle choices alike"
            );
        }
    }

    #[test]
    fn stale_row_values_and_ref_activity_remain_visible_during_refresh() {
        let mut state = sample_state();
        let id = state.identities[0].id.clone();
        state.refresh.waiting_rows.insert(id);
        state.refresh.updating_refs = true;
        state.refresh.spinner_tick = 2;

        let output = text(&render_buffer(&mut state, 120, 24, "ops"));

        assert!(output.contains("Worktrees · 2 · Updating refs"), "{output}");
        assert!(output.contains("3 changed · ↑2 ↓1"), "{output}");
        assert!(output.contains(flux_frame(2)), "{output}");
    }

    #[test]
    fn identity_renders_with_inline_flux_motion_before_first_status() {
        let row = identity(
            "/worktrees/feature-auth",
            "feature-auth",
            Some("feature/auth"),
            false,
            true,
        );
        let id = row.id.clone();
        let mut state = AppState::new(vec![row]);
        state.refresh.waiting_rows.insert(id);
        state.refresh.spinner_tick = 1;

        let output = text(&render_buffer(&mut state, 80, 20, "ops"));

        assert!(output.contains("feature-auth"), "{output}");
        assert!(output.contains("Updating"), "{output}");
        assert!(output.contains(flux_frame(1)), "{output}");
    }

    #[test]
    fn launcher_search_keeps_the_filtered_cockpit_and_input_keybar_visible() {
        let mut state = sample_state();
        let main_id = state.identities[1].id.clone();
        state.statuses.insert(main_id, WorktreeStatus::default());
        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('/')));
        for character in "mai".chars() {
            let _ = reduce(
                &mut state,
                Event::Input(crate::tui::keymap::Key::Char(character)),
            );
        }

        let buffer = render_buffer(&mut state, 120, 24, "ops");
        let output = text(&buffer);
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();
        assert!(output.contains("Search"), "{output}");
        assert!(output.contains("> mai▌"), "{output}");
        assert!(output.contains("trench"), "{output}");
        assert!(!output.contains("feature-auth"), "{output}");
        assert!(footer.contains("Esc clear"), "{footer}");
        assert!(!footer.contains("c create"), "{footer}");
        assert!(!footer.contains("? help"), "{footer}");
    }

    #[test]
    fn empty_launcher_search_is_a_visible_focused_input() {
        let mut state = sample_state();
        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('/')));

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let output = text(&buffer);
        let theme = crate::tui::theme::from_name("ops");
        let placeholder = find_text(&buffer, "Type to filter worktrees");

        assert!(output.contains("Search · typing"), "{output}");
        assert!(output.contains("> Type to filter worktrees▌"), "{output}");
        assert!(output.contains("2 results"), "{output}");
        assert_eq!(buffer.cell(placeholder).unwrap().bg, theme.control_bg);
        assert!(buffer
            .content()
            .iter()
            .any(|cell| cell.symbol() == "│" && cell.fg == theme.border_active));
    }

    #[test]
    fn long_focused_inputs_keep_their_suffix_and_cursor_at_minimum_width() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let long_value = "feature/a-very-long-branch-name-that-keeps-going-past-the-control";
        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch(long_value);
        state.create_dialog = Some(dialog);

        let create = text(&render_buffer(&mut state, 60, 16, "ops"));
        assert!(create.contains("past-the-control▌"), "{create}");
        assert!(create.contains("> …"), "{create}");

        state.create_dialog = None;
        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('/')));
        let long_query = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        for character in long_query.chars() {
            let _ = reduce(
                &mut state,
                Event::Input(crate::tui::keymap::Key::Char(character)),
            );
        }
        let search = text(&render_buffer(&mut state, 60, 16, "ops"));
        assert!(search.contains("xxxxxxxx▌"), "{search}");
        assert!(search.contains("> …"), "{search}");
    }

    #[test]
    fn long_input_scrolls_to_keep_a_moved_cursor_visible() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::{
                create_flow::{CreateDialog, CreateKey},
                line_input::LineEdit,
            },
        };

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/a-very-long-branch-name-that-keeps-going-past-the-control");
        dialog.handle_key(CreateKey::Edit(LineEdit::Start));
        state.create_dialog = Some(dialog);

        let output = text(&render_buffer(&mut state, 60, 16, "ops"));

        assert!(output.contains("> ▌feature/a"), "{output}");
        assert!(!output.contains("past-the-control▌"), "{output}");
    }

    #[test]
    fn wide_unicode_inputs_keep_their_cursor_at_minimum_width() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let long_value = "界".repeat(40);
        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch(&long_value);
        state.create_dialog = Some(dialog);

        let create = text(&render_buffer(&mut state, 60, 16, "ops"));
        let create_input = create
            .lines()
            .find(|line| line.contains("> …"))
            .expect("ellipsized create input");
        assert!(create_input.contains('▌'), "{create}");

        state.create_dialog = None;
        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('/')));
        for character in long_value.chars() {
            let _ = reduce(
                &mut state,
                Event::Input(crate::tui::keymap::Key::Char(character)),
            );
        }
        let search = text(&render_buffer(&mut state, 60, 16, "ops"));
        let search_input = search
            .lines()
            .find(|line| line.contains("> …"))
            .expect("ellipsized search input");
        assert!(search_input.contains('▌'), "{search}");
    }

    #[test]
    fn launcher_no_results_renders_no_actionable_row() {
        let mut state = sample_state();
        let _ = reduce(&mut state, Event::Input(crate::tui::keymap::Key::Char('/')));
        for character in "xyz".chars() {
            let _ = reduce(
                &mut state,
                Event::Input(crate::tui::keymap::Key::Char(character)),
            );
        }

        let buffer = render_buffer(&mut state, 80, 20, "ops");
        let output = text(&buffer);
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();
        assert!(output.contains("No matching worktrees"), "{output}");
        assert!(!output.contains("feature-auth"), "{output}");
        assert!(state.selected_visible().is_none());
        assert!(!footer.contains("Enter switch"), "{footer}");
        assert!(!footer.contains("o open"), "{footer}");
        assert!(footer.contains("Esc clear"), "{footer}");
        assert!(!footer.contains("? help"), "{footer}");
    }

    #[test]
    fn completed_rows_stop_spinner_without_blanking_their_values() {
        let mut state = sample_state();
        let output = text(&render_buffer(&mut state, 120, 24, "ops"));

        assert!(output.contains("3 changed · ↑2 ↓1"), "{output}");
        assert!(!output.contains("Updating refs"), "{output}");
        for glyph in FluxFrames::BRAILLE {
            assert!(
                !output.contains(*glyph),
                "unexpected spinner {glyph}\n{output}"
            );
        }
    }

    #[test]
    fn fetch_failure_warning_uses_one_brief_non_blocking_line() {
        let mut state = sample_state();
        state.refresh.warning = Some("Could not update origin; showing local refs".to_string());

        let output = text(&render_buffer(&mut state, 120, 24, "ops"));

        assert!(
            output.contains("Could not update origin; showing local refs"),
            "{output}"
        );
        assert!(output.contains("3 changed · ↑2 ↓1"), "{output}");
    }

    #[test]
    fn create_form_and_picker_transform_over_the_unchanged_cockpit_with_contextual_help() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::create_flow::{CreateDialog, CreateKey, OriginRefresh},
        };

        let refs = RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/topic"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/auth");
        state.create_dialog = Some(dialog);

        let form = text(&render_buffer(&mut state, 100, 24, "ops"));
        for expected in [
            "Create worktree",
            "Branch",
            "feature/auth",
            "Create from",
            "origin/main",
            "Worktree",
            "feature-auth",
            "/worktrees/trench/feature-auth",
        ] {
            assert!(form.contains(expected), "missing {expected:?}\n{form}");
        }
        assert!(!form.contains("Hooks"), "{form}");
        assert!(form
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));

        let dialog = state.create_dialog.as_mut().unwrap();
        dialog.handle_key(CreateKey::Tab);
        dialog.set_origin_refresh(OriginRefresh::Loading);
        let picker = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert_eq!(picker.matches("Create worktree").count(), 1, "{picker}");
        assert!(picker.contains("Select base"), "{picker}");
        assert!(picker.contains("Updating origin"), "{picker}");
        assert!(picker.contains("release"), "{picker}");
        assert!(picker
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));

        state.help_open = true;
        let help = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert!(help.contains("Help · Create worktree"), "{help}");
        assert!(help.contains("select base"), "{help}");
        assert!(!help.contains("Help · Worktrees"), "{help}");
    }

    #[test]
    fn focused_create_branch_is_visibly_an_input_in_the_rendered_terminal() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/auth");
        state.create_dialog = Some(dialog);

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let output = text(&buffer);
        let theme = crate::tui::theme::from_name("ops");
        let input_cell = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "f" && cell.bg == theme.control_bg)
            .expect("typed branch should sit on a distinct input surface");

        assert!(output.contains("Branch · typing"), "{output}");
        assert!(output.contains("> feature/auth▌"), "{output}");
        assert_eq!(input_cell.fg, theme.fg);
        assert!(
            buffer
                .content()
                .iter()
                .any(|cell| cell.symbol() == "│" && cell.fg == theme.border_active),
            "focused input should have an active border\n{output}"
        );
    }

    #[test]
    fn changed_form_errors_are_labeled_and_error_toned() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/auth");
        dialog.set_validation_error(Some("branch changed while open".to_string()));
        state.create_dialog = Some(dialog);
        let theme = crate::tui::theme::from_name("ops");

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let error = find_text(&buffer, "Error: branch changed while open");
        assert_eq!(buffer.cell(error).unwrap().fg, theme.error);
    }

    #[test]
    fn selected_create_suggestions_and_base_candidates_have_a_selection_surface() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::create_flow::{CreateDialog, CreateKey},
        };

        let refs = RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/topic"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/auth");
        state.create_dialog = Some(dialog);
        let theme = crate::tui::theme::from_name("ops");

        let suggestions = render_buffer(&mut state, 100, 24, "ops");
        let suggestion = find_text(&suggestions, "› Create \"feature/auth\" as new branch");
        assert_eq!(suggestions.cell(suggestion).unwrap().bg, theme.selection_bg);
        assert_eq!(suggestions.cell(suggestion).unwrap().fg, theme.selection_fg);

        state
            .create_dialog
            .as_mut()
            .unwrap()
            .handle_key(CreateKey::Tab);
        let candidates = render_buffer(&mut state, 100, 24, "ops");
        let selected = find_text(&candidates, "› main");
        let unselected = find_text(&candidates, "release");
        assert_eq!(candidates.cell(selected).unwrap().bg, theme.selection_bg);
        assert_eq!(candidates.cell(selected).unwrap().fg, theme.selection_fg);
        assert_ne!(candidates.cell(unselected).unwrap().bg, theme.selection_bg);
    }

    #[test]
    fn create_base_selector_and_picker_search_look_like_controls() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::create_flow::{CreateDialog, CreateKey},
        };

        let refs = RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/auth");
        state.create_dialog = Some(dialog);
        let theme = crate::tui::theme::from_name("ops");

        let form = render_buffer(&mut state, 100, 24, "ops");
        let selector = find_text(&form, "[ origin/main  ▾ ]");
        assert_eq!(
            form.cell(selector).unwrap().bg,
            theme.control_bg,
            "base selector needs a distinct control surface\n{}",
            text(&form)
        );

        state
            .create_dialog
            .as_mut()
            .unwrap()
            .handle_key(CreateKey::Tab);
        let picker = render_buffer(&mut state, 100, 24, "ops");
        let placeholder = find_text(&picker, "Type to filter bases");
        assert!(
            text(&picker).contains("Search · typing"),
            "{}",
            text(&picker)
        );
        assert!(
            text(&picker).contains("> Type to filter bases▌"),
            "{}",
            text(&picker)
        );
        assert_eq!(picker.cell(placeholder).unwrap().bg, theme.control_bg);
    }

    #[test]
    fn dialog_keybar_replaces_the_cockpit_keybar_instead_of_leaking_through() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut dialog = CreateDialog::new("trench", Path::new("/worktrees"), refs, []);
        dialog.set_branch("feature/auth");
        state.create_dialog = Some(dialog);

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let footer = lines(&buffer).last().unwrap().trim_end().to_string();

        assert_eq!(footer.matches("[?] help").count(), 1, "{footer}");
        for leaked in ["s sync", "/ search", "r refresh", "i inspector", "q quit"] {
            assert!(!footer.contains(leaked), "leaked {leaked:?}: {footer}");
        }
        assert_eq!(footer, "[Enter] create  [Tab] base  [Esc] close  [?] help");
    }

    #[test]
    fn create_and_sync_keybars_distinguish_primary_and_disabled_actions() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::{create_flow::CreateDialog, sync_flow::SyncDialog},
        };

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        state.create_dialog = Some(CreateDialog::new(
            "trench",
            Path::new("/worktrees"),
            refs,
            [],
        ));
        let theme = crate::tui::theme::from_name("ops");

        let disabled_create = render_buffer(&mut state, 100, 24, "ops");
        let disabled = find_text(&disabled_create, "[Enter] create (branch required)");
        assert_eq!(
            disabled_create.cell(disabled).unwrap().fg,
            theme.disabled_fg
        );

        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_branch("feature/auth");
        let enabled_create = render_buffer(&mut state, 100, 24, "ops");
        let primary = find_text(&enabled_create, "[Enter] create");
        assert_eq!(
            enabled_create.cell(primary).unwrap().bg,
            Color::Rgb(240, 139, 101)
        );
        assert_eq!(
            enabled_create.cell(primary).unwrap().fg,
            Color::Rgb(20, 20, 19)
        );

        let target = state.identities[1].clone();
        state.create_dialog = None;
        state.sync_dialog = Some(SyncDialog::new(
            &target,
            RefSnapshot::from_parts(
                std::iter::empty::<&str>(),
                std::iter::empty::<&str>(),
                None::<&str>,
                None::<&str>,
                false,
            ),
            None,
        ));
        let disabled_sync = render_buffer(&mut state, 100, 24, "ops");
        let disabled = find_text(&disabled_sync, "[Enter] sync (select a base)");
        assert_eq!(disabled_sync.cell(disabled).unwrap().fg, theme.disabled_fg);

        state
            .sync_dialog
            .as_mut()
            .unwrap()
            .handle_key(crate::tui::sync_flow::SyncKey::Tab);
        let empty_picker = render_buffer(&mut state, 100, 24, "ops");
        let disabled = find_text(&empty_picker, "[Enter] select (no matches)");
        assert_eq!(empty_picker.cell(disabled).unwrap().fg, theme.disabled_fg);
    }

    #[test]
    fn create_sync_and_help_keep_controls_visible_at_exact_minimum_size() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::{
                create_flow::CreateDialog,
                sync_flow::{SyncDialog, SyncKey},
            },
        };

        let refs = RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        let mut create = CreateDialog::new("trench", Path::new("/worktrees"), refs.clone(), []);
        create.set_branch("feature/auth");
        state.create_dialog = Some(create);

        let create = text(&render_buffer(&mut state, 60, 16, "ops"));
        for visible in ["Branch · typing", "[Enter] create", "[Esc] close"] {
            assert!(create.contains(visible), "missing {visible:?}\n{create}");
        }

        let target = state.identities[1].clone();
        let mut sync = SyncDialog::new(&target, refs, Some("main"));
        state.create_dialog = None;
        state.sync_dialog = Some(sync);
        let sync_form = text(&render_buffer(&mut state, 60, 16, "ops"));
        let sync_footer = sync_form.lines().last().unwrap().trim_end();
        assert!(sync_footer.contains("[Enter] sync"), "{sync_form}");
        assert!(
            sync_footer.ends_with("[Esc] close  [?] help"),
            "{sync_form}"
        );

        sync = state.sync_dialog.take().unwrap();
        sync.handle_key(SyncKey::Tab);
        state.sync_dialog = Some(sync);
        let picker = text(&render_buffer(&mut state, 60, 16, "ops"));
        for visible in ["Search · typing", "[Enter] select", "[Esc] back"] {
            assert!(picker.contains(visible), "missing {visible:?}\n{picker}");
        }

        state.sync_dialog = None;
        state.help_open = true;
        let help = text(&render_buffer(&mut state, 60, 16, "ops"));
        assert!(help.contains("Help · Worktrees"), "{help}");
        assert!(help.lines().last().unwrap().contains("[?] close help"));
    }

    #[test]
    fn active_dialog_has_a_high_contrast_border_and_title() {
        use crate::{ref_catalog::RefSnapshot, tui::create_flow::CreateDialog};

        let refs = RefSnapshot::from_parts(
            ["main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut state = sample_state();
        state.create_dialog = Some(CreateDialog::new(
            "trench",
            Path::new("/worktrees"),
            refs,
            [],
        ));

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let theme = crate::tui::theme::from_name("ops");
        let (title_x, title_y) = find_text(&buffer, "Create worktree");
        let title = buffer.cell((title_x, title_y)).unwrap();
        let border_x = (0..title_x)
            .rev()
            .find(|x| buffer.cell((*x, title_y)).unwrap().symbol() == "┌")
            .expect("dialog top border");
        let border = buffer.cell((border_x, title_y)).unwrap();
        let blank_inner = buffer.cell((border_x + 2, title_y + 8)).unwrap();

        assert_eq!(title.fg, theme.accent);
        assert!(title.modifier.contains(Modifier::BOLD));
        assert_eq!(border.fg, theme.border_active);
        assert_ne!(border.bg, theme.bg_panel);
        assert_eq!(blank_inner.bg, theme.bg_elevated);
        assert_ne!(blank_inner.bg, theme.bg_panel);
    }

    #[test]
    fn sync_form_picker_and_operation_are_contextual_and_end_with_help() {
        use crate::{
            hooks::{
                types::{HookStep, OutputStream},
                HookEvent,
            },
            operation::{OperationEvent, OperationKind, OperationStage},
            ref_catalog::RefSnapshot,
            tui::{
                operation_modal::OperationModal,
                sync_flow::{SyncDialog, SyncKey},
            },
        };

        let mut state = sample_state();
        let target = state.identities[1].clone();
        let refs = RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/topic"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        state.sync_dialog = Some(SyncDialog::new(&target, refs, Some("release")));

        let form = text(&render_buffer(&mut state, 100, 24, "ops"));
        for expected in [
            "Sync worktree",
            target.worktree.as_str(),
            target.branch.as_deref().unwrap(),
            "Base",
            "release",
            "Strategy",
            "Rebase",
        ] {
            assert!(form.contains(expected), "missing {expected:?}\n{form}");
        }
        assert!(form
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));

        state.sync_dialog.as_mut().unwrap().handle_key(SyncKey::Tab);
        let picker = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert!(picker.contains("Sync worktree · Select base"), "{picker}");
        assert!(picker.contains("origin/topic"), "{picker}");
        assert!(picker
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));

        state.help_open = true;
        let help = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert!(help.contains("Help · Sync worktree"), "{help}");
        assert!(help.contains("select base"), "{help}");

        let mut modal = OperationModal::new(OperationKind::Sync);
        modal.apply(OperationEvent::StageStarted {
            stage: OperationStage::PreHook,
        });
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PreSync,
            step: HookStep::Run,
            stream: OutputStream::Stdout,
            line: "sync hook output".to_string(),
        });
        state.help_open = false;
        state.operation_modal = Some(modal);
        let operation = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert!(operation.contains("Pre-sync hook"), "{operation}");
        assert!(operation.contains("sync hook output"), "{operation}");
        assert!(!operation.lines().last().unwrap().contains("cancel"));
        assert!(operation
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));
    }

    #[test]
    fn sync_base_strategy_and_picker_search_render_as_controls() {
        use crate::{
            ref_catalog::RefSnapshot,
            tui::sync_flow::{SyncDialog, SyncKey},
        };

        let mut state = sample_state();
        let target = state.identities[1].clone();
        let refs = RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/topic"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        state.sync_dialog = Some(SyncDialog::new(&target, refs, Some("release")));
        let theme = crate::tui::theme::from_name("ops");

        let form = render_buffer(&mut state, 100, 24, "ops");
        let base = find_text(&form, "[ release  ▾ ]");
        let selected_strategy = find_text(&form, "[● Rebase]");
        let other_strategy = find_text(&form, "[○ Merge]");
        assert_eq!(form.cell(base).unwrap().bg, theme.control_bg);
        assert_eq!(form.cell(selected_strategy).unwrap().bg, theme.selection_bg);
        assert_eq!(form.cell(other_strategy).unwrap().bg, theme.control_bg);

        state.sync_dialog.as_mut().unwrap().handle_key(SyncKey::Tab);
        let picker = render_buffer(&mut state, 100, 24, "ops");
        let placeholder = find_text(&picker, "Type to filter bases");
        assert!(
            text(&picker).contains("Search · typing"),
            "{}",
            text(&picker)
        );
        assert_eq!(picker.cell(placeholder).unwrap().bg, theme.control_bg);
    }

    #[test]
    fn operation_modal_renders_named_stage_elapsed_spinner_and_hook_output_without_percentages() {
        use std::time::Duration;

        use crate::{
            hooks::{
                types::{HookStep, OutputStream},
                HookEvent,
            },
            operation::{OperationEvent, OperationKind, OperationStage},
            tui::operation_modal::OperationModal,
        };

        let mut state = sample_state();
        let mut modal = OperationModal::new(OperationKind::Create);
        modal.apply(OperationEvent::StageStarted {
            stage: OperationStage::PreHook,
        });
        modal.apply(OperationEvent::Output {
            hook: HookEvent::PreCreate,
            step: HookStep::Run,
            stream: OutputStream::Stdout,
            line: "installing dependencies".to_string(),
        });
        modal.tick(Duration::from_millis(1_250));
        state.operation_modal = Some(modal);

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let output = text(&buffer);
        assert!(output.contains("Create worktree"), "{output}");
        assert!(output.contains("Pre-create hook"), "{output}");
        assert!(output.contains("1.2s"), "{output}");
        assert!(output.contains("installing dependencies"), "{output}");
        assert!(!output.contains('%'), "{output}");
        assert!(output
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));

        state.help_open = true;
        let help = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert!(help.contains("Help · Operation"), "{help}");
        assert!(help.contains("scroll output"), "{help}");
    }

    #[test]
    fn operation_modal_styles_success_failure_and_warning_states_semantically() {
        use std::time::Duration;

        use crate::{
            operation::{OperationEvent, OperationKind, OperationStage},
            tui::operation_modal::OperationModal,
        };

        let mut modal = OperationModal::new(OperationKind::Create);
        modal.apply(OperationEvent::StageStarted {
            stage: OperationStage::Revalidate,
        });
        modal.apply(OperationEvent::StageFinished {
            stage: OperationStage::Revalidate,
            duration: Duration::from_millis(10),
            success: true,
        });
        modal.apply(OperationEvent::StageStarted {
            stage: OperationStage::PreHook,
        });
        modal.apply(OperationEvent::StageFinished {
            stage: OperationStage::PreHook,
            duration: Duration::from_millis(10),
            success: false,
        });
        modal.apply(OperationEvent::Warning {
            stage: OperationStage::PostHook,
            message: "cache cleanup failed".to_string(),
        });
        let mut state = sample_state();
        state.operation_modal = Some(modal);
        let theme = crate::tui::theme::from_name("ops");

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let success = find_text(&buffer, "✓ Revalidate");
        let failure = find_text(&buffer, "× Pre-create hook");
        let warning = find_text(&buffer, "Warning: cache cleanup failed");

        assert_eq!(buffer.cell(success).unwrap().fg, theme.success);
        assert_eq!(buffer.cell(failure).unwrap().fg, theme.error);
        assert_eq!(buffer.cell(warning).unwrap().fg, theme.warning);
    }

    #[test]
    fn remove_dialog_renders_worktree_first_unchecked_local_branch_only_and_help_last() {
        let directory = tempfile::tempdir().unwrap();
        let repository = git2::Repository::init(directory.path()).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repository.index().unwrap().write_tree().unwrap();
        let tree = repository.find_tree(tree_id).unwrap();
        let commit_id = repository
            .commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
            .unwrap();
        drop(tree);
        let commit = repository.find_commit(commit_id).unwrap();
        repository.branch("feature", &commit, false).unwrap();
        drop(commit);
        let path = directory.path().join("worktrees").join("feature");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let reference = repository.find_reference("refs/heads/feature").unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(&reference));
        repository
            .worktree("feature", &path, Some(&options))
            .unwrap();
        let assessment = crate::cli::commands::remove::stateless::RemovalAssessment::discover(
            directory.path(),
            path.to_str().unwrap(),
            Some("main"),
        )
        .unwrap();
        let mut state = AppState::new(vec![identity(
            path.to_str().unwrap(),
            "feature",
            Some("feature"),
            false,
            false,
        )]);
        state.remove_dialog = Some(
            crate::tui::remove_flow::RemoveDialog::new(WorktreeId::new(&path), assessment).unwrap(),
        );
        let theme = crate::tui::theme::from_name("ops");

        let buffer = render_buffer(&mut state, 100, 24, "ops");
        let output = text(&buffer);
        assert!(
            output.contains("The worktree directory will be removed."),
            "{output}"
        );
        assert!(
            output.contains("› [ ] Also delete local branch feature"),
            "{output}"
        );
        let checkbox = find_text(&buffer, "› [ ] Also delete local branch feature");
        assert_eq!(buffer.cell(checkbox).unwrap().bg, theme.control_bg);
        let destructive = find_text(&buffer, "[Enter] remove");
        assert_eq!(buffer.cell(destructive).unwrap().bg, theme.danger_bg);
        assert_eq!(buffer.cell(destructive).unwrap().fg, theme.danger_fg);
        assert!(!output.to_lowercase().contains("remote branch"), "{output}");
        assert!(output
            .lines()
            .last()
            .unwrap()
            .trim_end()
            .ends_with("[?] help"));

        state
            .remove_dialog
            .as_mut()
            .unwrap()
            .handle_key(crate::tui::remove_flow::RemoveKey::Space);
        let minimum = text(&render_buffer(&mut state, 60, 16, "ops"));
        for visible in [
            "Remove worktree",
            "› [x] Also delete local branch feature",
            "[Enter] remove",
            "[Esc] close",
        ] {
            assert!(minimum.contains(visible), "missing {visible:?}\n{minimum}");
        }

        state.help_open = true;
        let help = text(&render_buffer(&mut state, 100, 24, "ops"));
        assert!(help.contains("Help · Remove worktree"), "{help}");
        assert!(help.contains("toggle local branch deletion"), "{help}");
        assert!(help.contains("remove worktree"), "{help}");
        assert!(!help.contains("search branches"), "{help}");
    }
}
