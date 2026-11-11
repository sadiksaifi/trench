//! The create dialog: branch field, base field, one list, one summary.

use std::path::Path;

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    output::text,
    paths,
    tui::{
        app::AppState,
        create_flow::{BranchRow, CreateDialog, CreateFocus, Problem, RowKind, Summary},
        theme::Theme,
        view::{
            list::spinner,
            widgets::{self, Hint},
        },
    },
};

const LABEL_WIDTH: usize = 6;
/// Below this content width the tag column is dropped to keep names readable.
const MIN_WIDTH_FOR_TAGS: usize = 44;
const SUMMARY_ROWS: usize = 3;

pub fn render(
    dialog: &CreateDialog,
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
) {
    // The dialog has one size, so nothing moves while the user types. Blank
    // spacer rows and list rows give way on short terminals.
    let layout = widgets::dialog_layout(area, 15);
    let rows = usize::from(layout.content.height);
    let spacers = rows >= 9;
    let list_rows = rows.saturating_sub(2 + SUMMARY_ROWS + usize::from(spacers) * 2);
    let width = usize::from(layout.content.width);

    widgets::dialog_frame(
        frame,
        layout,
        "Create worktree",
        origin_status(dialog, state, theme),
        theme,
    );

    let mut lines = vec![
        branch_field(dialog, width, theme),
        base_field(dialog, width, theme),
    ];
    if spacers {
        lines.push(Line::default());
    }
    let list = match dialog.focus() {
        CreateFocus::Branch => branch_rows(dialog, list_rows, width, theme),
        CreateFocus::Base => base_rows(dialog, list_rows, width, theme),
    };
    lines.extend(list);
    lines.resize(rows.saturating_sub(SUMMARY_ROWS), Line::default());
    lines.extend(summary(dialog, state, width, theme));

    frame.render_widget(
        Paragraph::new(lines).style(theme.on_dialog(theme.text)),
        layout.content,
    );
    widgets::hint_bar(frame, layout.hints, &hints(dialog), theme);
}

fn origin_status(dialog: &CreateDialog, state: &AppState, theme: &Theme) -> Option<Line<'static>> {
    if dialog.origin_spinner_visible() {
        Some(Line::styled(
            format!(" fetching origin {} ", spinner(state.refresh.spinner_tick)),
            theme.on_dialog(theme.muted),
        ))
    } else {
        dialog
            .warning()
            .map(|_| Line::styled(" could not fetch origin ", theme.on_dialog(theme.warning)))
    }
}

fn branch_field(dialog: &CreateDialog, width: usize, theme: &Theme) -> Line<'static> {
    widgets::field(
        "Branch",
        LABEL_WIDTH,
        widgets::input(
            dialog.branch_input(),
            "type a name, or pick a branch below",
            width.saturating_sub(LABEL_WIDTH + 2),
            dialog.focus() == CreateFocus::Branch,
            theme,
        ),
        theme,
    )
}

fn base_field(dialog: &CreateDialog, width: usize, theme: &Theme) -> Line<'static> {
    let room = width.saturating_sub(LABEL_WIDTH + 2);
    if dialog.focus() == CreateFocus::Base {
        return widgets::field(
            "From",
            LABEL_WIDTH,
            widgets::input(
                dialog.base_input(),
                "type to filter bases",
                room,
                true,
                theme,
            ),
            theme,
        );
    }
    let highlighted = dialog.highlighted_row();
    let exists = highlighted
        .as_ref()
        .is_some_and(|row| row.kind != RowKind::New);
    if exists {
        // The base is unused for an existing branch. Saying so beats letting
        // the field silently do nothing.
        return widgets::field(
            "From",
            LABEL_WIDTH,
            vec![Span::styled(
                text::truncate_end("not needed, the branch exists", room),
                theme.muted,
            )],
            theme,
        );
    }
    let hint = if dialog.base_applies() {
        "Tab to change"
    } else {
        ""
    };
    // The base always wins: the reminder is dropped when both do not fit,
    // because the hint bar names the same key.
    let base = text::truncate_middle(dialog.base().unwrap_or("none found"), room);
    let gap = room.saturating_sub(text::width(&base) + text::width(hint));
    let mut value = vec![Span::styled(base, theme.text)];
    if gap >= 2 {
        value.push(Span::raw(" ".repeat(gap)));
        value.push(Span::styled(hint, theme.muted));
    }
    widgets::field("From", LABEL_WIDTH, value, theme)
}

fn branch_rows(
    dialog: &CreateDialog,
    capacity: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let rows = dialog.rows();
    let window = widgets::window(rows.len(), dialog.highlight(), capacity);
    let mut lines = window
        .rows
        .clone()
        .map(|index| {
            let (tag, style) = tag(&rows[index], theme);
            widgets::row(
                rows[index].branch.clone(),
                (width >= MIN_WIDTH_FOR_TAGS).then_some((tag, style)),
                dialog.highlight() == Some(index),
                width,
                theme,
            )
        })
        .collect::<Vec<_>>();
    more(&mut lines, window.more, theme);
    lines
}

fn base_rows(
    dialog: &CreateDialog,
    capacity: usize,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let candidates = dialog.base_candidates();
    if candidates.is_empty() {
        return vec![Line::styled(
            format!("  No branch matches \"{}\".", dialog.base_input().value()),
            theme.muted,
        )];
    }
    let window = widgets::window(candidates.len(), Some(dialog.base_highlight()), capacity);
    let mut lines = window
        .rows
        .clone()
        .map(|index| {
            widgets::row(
                candidates[index].name.clone(),
                (width >= MIN_WIDTH_FOR_TAGS)
                    .then_some((widgets::ref_tag(candidates[index].kind), theme.muted)),
                dialog.base_highlight() == index,
                width,
                theme,
            )
        })
        .collect::<Vec<_>>();
    more(&mut lines, window.more, theme);
    lines
}

fn more(lines: &mut Vec<Line<'static>>, hidden: usize, theme: &Theme) {
    if hidden > 0 {
        lines.push(Line::styled(format!("  … {hidden} more"), theme.muted));
    }
}

/// Tags are padded to one width, so they form a left-aligned column.
fn tag(row: &BranchRow, theme: &Theme) -> (&'static str, Style) {
    match (&row.problem, &row.kind) {
        (Some(Problem::InvalidName(_)), _) => ("not valid     ", theme.error),
        (Some(_), _) => ("not available ", theme.error),
        (None, RowKind::New) => ("new branch    ", theme.accent),
        (None, RowKind::Local) => (widgets::LOCAL_TAG, theme.muted),
        (None, RowKind::Remote { .. }) => (widgets::REMOTE_TAG, theme.muted),
        (None, RowKind::HasWorktree { .. }) => ("has a worktree", theme.muted),
    }
}

/// Three rows that say what Enter will do, or why it will not.
fn summary(
    dialog: &CreateDialog,
    state: &AppState,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let plain = |value: String| Line::styled(text::truncate_end(&value, width), theme.text);
    let muted = |value: &str| Line::styled(text::truncate_end(value, width), theme.muted);
    let failure = |message: &str, advice: &str| {
        let mut lines = widgets::wrap(&format!("{} {message}", widgets::FAILED), width)
            .into_iter()
            .take(2)
            .map(|line| Line::styled(line, theme.error))
            .collect::<Vec<_>>();
        lines.push(muted(advice));
        lines
    };
    let home = paths::home_dir();
    let at = |prefix: &str, path: &Path| {
        let path = text::home_relative(path, home.as_deref());
        let room = width.saturating_sub(text::width(prefix) + 1);
        Line::from(vec![
            Span::styled(format!("{prefix} "), theme.muted),
            Span::styled(text::truncate_start(&path, room), theme.text),
        ])
    };
    let hooks = (!state.hooks.create.is_empty())
        .then(|| muted(&format!("Runs hooks: {}", state.hooks.create.join(", "))));

    let mut lines = if let Some(error) = dialog.validation_error() {
        failure(error, "Nothing was created.")
    } else {
        match dialog.summary() {
            Summary::Empty => vec![muted("Type a branch name, or press ↓ to pick one.")],
            Summary::Blocked(Problem::InvalidName(reason)) => {
                failure(&reason, "Fix the name to continue.")
            }
            Summary::Blocked(Problem::DirectoryTaken { owner, directory }) => failure(
                &format!("{owner} already uses directory {directory}"),
                "Pick a different branch name.",
            ),
            Summary::Blocked(Problem::NamespaceConflict { existing, name }) => failure(
                &format!("Branch {existing} exists, so Git cannot also have {name}"),
                "Pick a different branch name.",
            ),
            Summary::NoBase => failure("No base branch found.", "Press Tab and choose one."),
            Summary::New { branch, base, path } => {
                let mut lines = vec![
                    plain(format!("New branch {branch} from {base}")),
                    at("Worktree at", &path),
                ];
                lines.extend(hooks);
                lines
            }
            Summary::Local { branch, path } => {
                let mut lines = vec![
                    plain(format!("Existing local branch {branch}")),
                    at("Worktree at", &path),
                ];
                lines.extend(hooks);
                lines
            }
            Summary::Remote {
                branch,
                upstream,
                path,
            } => {
                let mut lines = vec![
                    plain(format!("New local branch {branch}, tracking {upstream}")),
                    at("Worktree at", &path),
                ];
                lines.extend(hooks);
                lines
            }
            Summary::HasWorktree { branch, path } => vec![
                plain(format!("{branch} already has a worktree")),
                at("at", &path),
                muted("Nothing will be created."),
            ],
        }
    };
    lines.truncate(SUMMARY_ROWS);
    lines.resize(SUMMARY_ROWS, Line::default());
    lines
}

fn hints(dialog: &CreateDialog) -> Vec<Hint> {
    match dialog.focus() {
        CreateFocus::Branch => {
            let navigates = matches!(
                dialog.highlighted_row().map(|row| row.kind),
                Some(RowKind::HasWorktree { .. })
            );
            let mut hints = vec![
                Hint::new(
                    "Enter",
                    if navigates { "show in list" } else { "create" },
                    0,
                )
                .enabled(navigates || dialog.can_submit()),
                Hint::new("↑/↓", "pick", 3),
            ];
            if dialog.base_applies() {
                hints.push(Hint::new("Tab", "base", 2));
            }
            hints.push(Hint::new("Esc", "cancel", 1));
            hints
        }
        CreateFocus::Base => vec![
            Hint::new("Enter", "use base", 0).enabled(dialog.can_submit()),
            Hint::new("↑/↓", "pick", 2),
            Hint::new("Esc", "back", 1),
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ratatui::style::Modifier;

    use crate::{
        ref_catalog::RefSnapshot,
        tui::{
            app::{AppState, WorktreeId},
            create_flow::{CreateDialog, CreateKey, ExistingWorktree},
            view::testing::{draw, lines, screen, state},
        },
    };

    fn refs() -> RefSnapshot {
        RefSnapshot::from_parts(
            ["main", "feature/login-page", "feature/auth"],
            ["origin/main", "origin/feature/login-v2"],
            Some("origin/main"),
            Some("main"),
            true,
        )
    }

    fn open(keys: &str) -> AppState {
        let mut state = state(&[("feature/auth", "/wt/acme/feature-auth")]);
        state.hooks.create = vec!["pre_create", "post_create"];
        let mut dialog = CreateDialog::new(
            "acme",
            Path::new("/wt"),
            refs(),
            None,
            [
                ExistingWorktree::new(Some("main"), WorktreeId::new("/code/acme"), "/code/acme"),
                ExistingWorktree::new(
                    Some("feature/auth"),
                    WorktreeId::new("/wt/acme/feature-auth"),
                    "/wt/acme/feature-auth",
                ),
            ],
        );
        for character in keys.chars() {
            dialog.handle_key(match character {
                '↓' => CreateKey::Down,
                '⇥' => CreateKey::Tab,
                other => CreateKey::Character(other),
            });
        }
        state.create_dialog = Some(dialog);
        state
    }

    #[test]
    fn empty_dialog_lists_branches_and_asks_for_a_name() {
        let screen = screen(&mut open(""), 60, 18);

        assert_eq!(
            screen[1..16],
            [
                "  ╭ Create worktree ─────────────────────────────────────╮",
                "  │ Branch  ▌type a name, or pick a branch below         │",
                "  │ From    origin/main                                  │",
                "  │                                                      │",
                "  │   feature/login-page                  local branch   │",
                "  │   feature/login-v2                    remote branch  │",
                "  │   feature/auth                        has a worktree │",
                "  │   main                                has a worktree │",
                "  │                                                      │",
                "  │                                                      │",
                "  │                                                      │",
                "  │ Type a branch name, or press ↓ to pick one.          │",
                "  │                                                      │",
                "  │                                                      │",
                "  ╰──────────────────────────────────────────────────────╯",
            ]
        );
        assert_eq!(screen[17], " Enter create  ↑/↓ pick  Esc cancel");
    }

    #[test]
    fn a_new_name_is_the_highlighted_row_and_the_summary_states_the_result() {
        let screen = screen(&mut open("feature/login"), 60, 18);

        assert_eq!(
            screen[2..8],
            [
                "  │ Branch  feature/login▌                               │",
                "  │ From    origin/main                    Tab to change │",
                "  │                                                      │",
                "  │ › feature/login                       new branch     │",
                "  │   feature/login-v2                    remote branch  │",
                "  │   feature/login-page                  local branch   │",
            ]
        );
        assert_eq!(
            screen[12..15],
            [
                "  │ New branch feature/login from origin/main            │",
                "  │ Worktree at /wt/acme/feature-login                   │",
                "  │ Runs hooks: pre_create, post_create                  │",
            ]
        );
        assert_eq!(screen[17], " Enter create  ↑/↓ pick  Tab base  Esc cancel");
    }

    #[test]
    fn picking_an_existing_branch_says_the_base_is_not_needed() {
        let screen = screen(&mut open("feature/login↓"), 60, 18);

        assert_eq!(
            screen[3],
            "  │ From    not needed, the branch exists                │"
        );
        assert_eq!(
            screen[6],
            "  │ › feature/login-v2                    remote branch  │"
        );
        assert_eq!(
            screen[12],
            "  │ New local branch feature/login-v2, tracking origin/… │"
        );
        assert_eq!(screen[17], " Enter create  ↑/↓ pick  Esc cancel");
    }

    #[test]
    fn a_branch_that_has_a_worktree_offers_to_show_it_instead() {
        let screen = screen(&mut open("feature/auth"), 60, 18);

        assert_eq!(
            screen[12..15],
            [
                "  │ feature/auth already has a worktree                  │",
                "  │ at /wt/acme/feature-auth                             │",
                "  │ Nothing will be created.                             │",
            ]
        );
        assert_eq!(screen[17], " Enter show in list  ↑/↓ pick  Esc cancel");
    }

    #[test]
    fn an_invalid_name_shows_the_reason_and_dims_enter() {
        let mut state = open("fix login");
        let buffer = draw(&mut state, 60, 18, "terminal");
        let screen = lines(&buffer);

        assert_eq!(
            screen[5],
            "  │ › fix login                           not valid      │"
        );
        assert_eq!(
            screen[12..14],
            [
                "  │ × Branch name cannot contain ' '                     │",
                "  │ Fix the name to continue.                            │",
            ]
        );
        assert!(buffer[(1, 17)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_name_that_collides_with_a_directory_or_a_branch_namespace_is_explained() {
        let taken = screen(&mut open("feature@auth"), 60, 18);
        assert_eq!(
            taken[12..14],
            [
                "  │ × feature/auth already uses directory feature-auth   │",
                "  │ Pick a different branch name.                        │",
            ]
        );

        let nested = screen(&mut open("main/hotfix"), 60, 18);
        assert_eq!(
            nested[12..15],
            [
                "  │ × Branch main exists, so Git cannot also have        │",
                "  │ main/hotfix                                          │",
                "  │ Pick a different branch name.                        │",
            ]
        );
    }

    #[test]
    fn the_base_field_filters_in_place_without_resizing_the_dialog() {
        let branch = screen(&mut open("feature/login"), 60, 18);
        let base = screen(&mut open("feature/login⇥"), 60, 18);

        assert_eq!(base[1], branch[1]);
        assert_eq!(base[15], branch[15]);
        assert_eq!(
            base[2..9],
            [
                "  │ Branch  feature/login                                │",
                "  │ From    ▌type to filter bases                        │",
                "  │                                                      │",
                "  │ › origin/main                         remote branch  │",
                "  │   feature/auth                        local branch   │",
                "  │   feature/login-page                  local branch   │",
                "  │   main                                local branch   │",
            ]
        );
        assert_eq!(base[17], " Enter use base  ↑/↓ pick  Esc back");
    }

    #[test]
    fn a_planning_error_from_enter_is_shown_in_the_summary() {
        let mut state = open("feature/login");
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_validation_error(Some(
                "path already exists: /wt/acme/feature-login".to_string(),
            ));
        let screen = screen(&mut state, 60, 18);

        assert_eq!(
            screen[12..14],
            [
                "  │ × path already exists: /wt/acme/feature-login        │",
                "  │ Nothing was created.                                 │",
            ]
        );
    }

    #[test]
    fn long_names_scroll_in_the_field_and_stay_distinct_in_the_list() {
        let name = "feature/JIRA-12345-implement-the-thing-nobody-asked-for-v2";
        let screen = screen(&mut open(name), 60, 18);

        assert_eq!(
            screen[2],
            "  │ Branch  …45-implement-the-thing-nobody-asked-for-v2▌ │"
        );
        assert_eq!(
            screen[5],
            "  │ › feature/JIRA-1234…ody-asked-for-v2  new branch     │"
        );
        assert!(screen.iter().all(|row| row.chars().count() <= 60));
    }

    #[test]
    fn the_dialog_fits_the_minimum_terminal_without_losing_the_summary() {
        let screen = screen(&mut open("feature/login"), 40, 10);

        assert_eq!(
            screen[..10],
            [
                " ╭ Create worktree ───────────────────╮",
                " │ Branch  feature/login▌             │",
                " │ From    origin/main  Tab to change │",
                " │ › feature/login                    │",
                " │   … 2 more                         │",
                " │ New branch feature/login from ori… │",
                " │ Worktree at /wt/acme/feature-login │",
                " │ Runs hooks: pre_create, post_crea… │",
                " ╰────────────────────────────────────╯",
                " Enter create  Tab base  Esc cancel",
            ]
        );
    }

    #[test]
    fn fetch_status_sits_in_the_title_bar() {
        let mut state = open("");
        state
            .create_dialog
            .as_mut()
            .unwrap()
            .set_origin_refresh(crate::tui::create_flow::OriginRefresh::Failed);
        let screen = screen(&mut state, 60, 18);

        assert_eq!(
            screen[1],
            "  ╭ Create worktree ───────────── could not fetch origin ╮"
        );
    }
}
