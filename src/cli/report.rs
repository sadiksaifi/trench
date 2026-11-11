//! Human-readable wording for command results and failures.
//!
//! Commands return typed plans, outcomes and failures. This module is the one
//! place that turns them into sentences, so `--json` output and error enums
//! stay stable while the wording can change.

use std::path::Path;

use crate::cli::commands::remove::stateless::RemovalAuthorizationError;
use crate::cli::commands::sync::stateless as sync;
use crate::config::HooksConfig;
use crate::create_plan::{CreateAction, CreatePlan, HookPolicy};
use crate::exit_code::ExitCode;
use crate::git::GitError;
use crate::operation::{
    ErrorClass, MutationState, OperationFailure, OperationKind, OperationStage,
};
use crate::output::human::{self, Report};
use crate::worktree_catalog::CatalogError;

const LIST_HINT: &str = "run `trench list` to see worktrees";

/// Map any top-level error to its report and exit code.
pub fn top_level(error: &anyhow::Error) -> (String, ExitCode) {
    let palette = human::Palette::stderr();
    let report = |report: Report| report.render(palette);
    if error.downcast_ref::<crate::config::ConfigError>().is_some() {
        return (
            report(Report::new(error.to_string())),
            ExitCode::ConfigError,
        );
    }
    if let Some(git) = error.downcast_ref::<GitError>() {
        return (report(git_error(git)), ExitCode::GitError);
    }
    match error.downcast_ref::<CatalogError>() {
        Some(CatalogError::Git(git)) => (report(git_error(git)), ExitCode::GitError),
        Some(catalog) => (report(catalog_error(catalog)), ExitCode::NotFound),
        None => (
            human::render_error_chain(error, palette),
            ExitCode::GeneralError,
        ),
    }
}

fn git_error(error: &GitError) -> Report {
    let report = Report::new(error.to_string());
    match error {
        GitError::NotAGitRepo { .. } => report.hint("run trench inside a Git repository"),
        _ => report,
    }
}

pub fn catalog_error(error: &CatalogError) -> Report {
    match error {
        CatalogError::Git(git) => git_error(git),
        CatalogError::NotFound { .. } => Report::new(error.to_string())
            .hint(LIST_HINT)
            .hint("a detached worktree is named detached@<commit>"),
        CatalogError::DirectoryMissing { worktree, .. } => Report::new(error.to_string()).hint(
            format!("run `trench remove {worktree}` to clean up its Git entry"),
        ),
        CatalogError::Ambiguous { selector, matches } => {
            let mut report = Report::new(format!("more than one worktree matches '{selector}'"));
            for path in matches {
                report = report.hint(format!("pass the path to pick one: {}", path.display()));
            }
            report
        }
    }
}

/// The config key of the hook that runs in `stage`, if the stage is a hook.
pub fn hook_name(operation: OperationKind, stage: OperationStage) -> Option<&'static str> {
    match (operation, stage) {
        (OperationKind::Create, OperationStage::PreHook) => Some("pre_create"),
        (OperationKind::Create, OperationStage::PostHook) => Some("post_create"),
        (OperationKind::Sync, OperationStage::PreHook) => Some("pre_sync"),
        (OperationKind::Sync, OperationStage::PostHook) => Some("post_sync"),
        (OperationKind::Remove, OperationStage::PreHook) => Some("pre_remove"),
        (OperationKind::Remove, OperationStage::PostHook) => Some("post_remove"),
        _ => None,
    }
}

/// Say which hook failed: `post_create hook failed: `make` exited with code 2`.
pub fn hook_failure_message(hook: &str, message: &str) -> String {
    if let Some(detail) = message.strip_prefix("command failed: ") {
        format!("{hook} hook failed: {detail}")
    } else if let Some(detail) = message.strip_prefix("hook timed out") {
        format!("{hook} hook timed out{detail}")
    } else {
        format!("{hook} hook failed: {message}")
    }
}

/// What state the repository is in after a failure, in one sentence.
///
/// Returns `None` when nothing was attempted and the error already says so.
pub fn state_after_failure(
    operation: OperationKind,
    state: MutationState,
    failed_in_hook: bool,
) -> Option<&'static str> {
    match (state, operation) {
        (MutationState::NotStarted, _) if failed_in_hook => Some("nothing was changed"),
        (MutationState::NotStarted, _) => None,
        (MutationState::RolledBack, OperationKind::Create) => {
            Some("nothing was created; the worktree and branch were rolled back")
        }
        (MutationState::RolledBack, OperationKind::Sync) => {
            Some("the branch was restored to where it was")
        }
        (MutationState::RolledBack, OperationKind::Remove) => Some("nothing was removed"),
        (MutationState::Applied, OperationKind::Create) => {
            Some("the worktree was created; only the hook failed")
        }
        (MutationState::Applied, OperationKind::Sync) => {
            Some("the sync itself succeeded; only the hook failed")
        }
        (MutationState::Applied, OperationKind::Remove) => {
            Some("the worktree was removed; only the hook failed")
        }
        (MutationState::PartiallyApplied, _) => {
            Some("some changes were applied; check the worktree before retrying")
        }
    }
}

pub fn operation_failure(operation: OperationKind, failure: &OperationFailure) -> Report {
    let hook = hook_name(operation, failure.stage)
        .filter(|_| matches!(failure.class, ErrorClass::Hook | ErrorClass::HookTimeout));
    let mut report = Report::new(match hook {
        Some(hook) => hook_failure_message(hook, &failure.message),
        None => failure.message.clone(),
    });
    if let Some(state) = state_after_failure(operation, failure.mutation_state, hook.is_some()) {
        report = report.hint(state);
    }
    if failure.class == ErrorClass::HookTimeout {
        if let Some(hook) = hook {
            report = report.hint(format!("raise timeout_secs in [hooks.{hook}]"));
        }
    }
    if let Some(path) = &failure.retained_quarantine {
        report = report.hint(format!("kept for recovery: {}", path.display()));
    }
    report
}

pub fn sync_failure(failure: &sync::SyncFailure) -> Report {
    let hook = match failure.stage {
        sync::SyncStage::PreHook => Some("pre_sync"),
        sync::SyncStage::PostHook => Some("post_sync"),
        _ => None,
    }
    .filter(|_| {
        matches!(
            failure.class,
            sync::SyncErrorClass::Hook | sync::SyncErrorClass::HookTimeout
        )
    });
    let mut report = Report::new(match hook {
        Some(hook) => hook_failure_message(hook, &failure.message),
        None => failure.message.clone(),
    });
    let state = match failure.mutation_state {
        sync::MutationState::NotStarted => MutationState::NotStarted,
        sync::MutationState::RolledBack => MutationState::RolledBack,
        sync::MutationState::Applied => MutationState::Applied,
        sync::MutationState::PartiallyApplied => MutationState::PartiallyApplied,
    };
    if let Some(state) = state_after_failure(OperationKind::Sync, state, hook.is_some()) {
        report = report.hint(state);
    }
    match (failure.class, hook) {
        (sync::SyncErrorClass::HookTimeout, Some(hook)) => {
            report.hint(format!("raise timeout_secs in [hooks.{hook}]"))
        }
        (sync::SyncErrorClass::InvalidTarget, _) => report.hint(LIST_HINT),
        (sync::SyncErrorClass::Dirty, _) => report.hint("commit or stash the changes, then sync"),
        _ => report,
    }
}

/// Word a planning error whose type carries more than its message.
pub fn sync_plan_error(error: &sync::SyncPlanError) -> Option<Report> {
    match error {
        sync::SyncPlanError::Catalog(catalog) => Some(catalog_error(catalog)),
        _ => None,
    }
}

/// Why `trench remove` refused, and the flag or command that unblocks it.
pub fn removal_refusal(error: &RemovalAuthorizationError, worktree: &str) -> Report {
    use RemovalAuthorizationError as Refusal;
    match error {
        Refusal::ConfirmationRequired => Report::new("removing a worktree needs confirmation")
            .hint("pass --yes when not running in a terminal"),
        Refusal::DirtyWorktree => {
            Report::new(format!("worktree {worktree} has uncommitted changes"))
                .hint("commit or stash them, or pass --force-worktree to discard them")
        }
        Refusal::UnverifiableWorktree { branch } => Report::new(format!(
            "local branch {branch} no longer exists, so worktree {worktree} cannot be verified clean"
        ))
        .hint("pass --force-worktree to remove it"),
        Refusal::LockedWorktree { path } => {
            Report::new(format!("worktree {worktree} is locked"))
                .hint(format!("run `git worktree unlock {}` first", path.display()))
        }
        Refusal::MissingLocalBranch { branch } => {
            Report::new(format!("local branch {branch} no longer exists"))
                .hint("omit --delete-branch to remove the worktree")
        }
        Refusal::BranchCheckedOutElsewhere { branch, path } => Report::new(format!(
            "branch {branch} is also checked out at {}",
            path.display()
        ))
        .hint("omit --delete-branch to remove only this worktree"),
        Refusal::UnmergedBranch { branch } => {
            Report::new(format!("branch {branch} is not merged"))
                .hint("pass --force-branch with --delete-branch to delete it anyway")
        }
        Refusal::MergeStatusUnavailable => {
            Report::new("cannot tell whether the branch is merged: its base was not found")
                .hint("pass --force-branch with --delete-branch to delete it anyway")
        }
        Refusal::MainWorktree
        | Refusal::ConfirmationReceiptMismatch
        | Refusal::ConfirmationReceiptUnexpected
        | Refusal::ForceBranchRequiresDeleteBranch
        | Refusal::DetachedHasNoLocalBranch => Report::new(error.to_string()),
    }
}

/// The names of the configured hooks among `pre` and `post`.
pub fn configured_hooks<'a>(pre: (&'a str, bool), post: (&'a str, bool)) -> Vec<&'a str> {
    [pre, post]
        .into_iter()
        .filter_map(|(name, configured)| configured.then_some(name))
        .collect()
}

pub fn create_hooks(hooks: Option<&HooksConfig>) -> Vec<&'static str> {
    configured_hooks(
        (
            "pre_create",
            hooks.is_some_and(|hooks| hooks.pre_create.is_some()),
        ),
        (
            "post_create",
            hooks.is_some_and(|hooks| hooks.post_create.is_some()),
        ),
    )
}

pub fn sync_hooks(hooks: Option<&HooksConfig>) -> Vec<&'static str> {
    configured_hooks(
        (
            "pre_sync",
            hooks.is_some_and(|hooks| hooks.pre_sync.is_some()),
        ),
        (
            "post_sync",
            hooks.is_some_and(|hooks| hooks.post_sync.is_some()),
        ),
    )
}

pub fn remove_hooks(hooks: Option<&HooksConfig>) -> Vec<&'static str> {
    configured_hooks(
        (
            "pre_remove",
            hooks.is_some_and(|hooks| hooks.pre_remove.is_some()),
        ),
        (
            "post_remove",
            hooks.is_some_and(|hooks| hooks.post_remove.is_some()),
        ),
    )
}

/// One line saying what `trench create` did.
pub fn created(plan: &CreatePlan) -> String {
    match &plan.action {
        CreateAction::NewBranch(base) => {
            format!("Created {} (new branch from {base})", plan.branch)
        }
        CreateAction::ExistingLocal => {
            format!("Created worktree for existing branch {}", plan.branch)
        }
        CreateAction::TrackRemote(upstream) => {
            format!("Created {}, tracking {upstream}", plan.branch)
        }
        CreateAction::Navigate(_) => {
            format!("{} already has a worktree; nothing created", plan.branch)
        }
    }
}

pub fn create_preview(plan: &CreatePlan, hooks: Option<&HooksConfig>) -> String {
    let (action, branch) = match &plan.action {
        CreateAction::NewBranch(base) => (
            format!("Would create worktree {}", plan.worktree),
            format!("{} (new, from {base})", plan.branch),
        ),
        CreateAction::ExistingLocal => (
            format!("Would create worktree {}", plan.worktree),
            format!("{} (existing local branch)", plan.branch),
        ),
        CreateAction::TrackRemote(upstream) => (
            format!("Would create worktree {}", plan.worktree),
            format!("{} (new, tracking {upstream})", plan.branch),
        ),
        CreateAction::Navigate(_) => (
            format!("Nothing to create: {} already has a worktree", plan.branch),
            plan.branch.clone(),
        ),
    };
    let mut fields = vec![
        ("Branch", branch),
        ("Path", plan.path.display().to_string()),
    ];
    if !matches!(plan.action, CreateAction::Navigate(_)) {
        fields.push((
            "Hooks",
            human::describe_hooks(plan.hook_policy == HookPolicy::Skip, &create_hooks(hooks)),
        ));
    }
    human::render_preview(&action, &fields)
}

fn counts(position: &sync::AheadBehind) -> String {
    format!("{} ahead, {} behind", position.ahead, position.behind)
}

/// `Rebased a onto b` or `Merged b into a`, in the tense given.
fn sync_action(strategy: sync::SyncStrategy, branch: &str, base: &str, done: bool) -> String {
    match (strategy, done) {
        (sync::SyncStrategy::Rebase, true) => format!("Rebased {branch} onto {base}"),
        (sync::SyncStrategy::Rebase, false) => format!("Would rebase {branch} onto {base}"),
        (sync::SyncStrategy::Merge, true) => format!("Merged {base} into {branch}"),
        (sync::SyncStrategy::Merge, false) => format!("Would merge {base} into {branch}"),
    }
}

/// One line saying what `trench sync` did.
pub fn synced(outcome: &sync::SyncOutcome) -> String {
    if outcome.before == outcome.after && outcome.before.behind == 0 {
        return format!(
            "{} is already up to date with {}",
            outcome.branch, outcome.base
        );
    }
    format!(
        "{}. Was {}; now {}.",
        sync_action(outcome.strategy, &outcome.branch, &outcome.base, true),
        counts(&outcome.before),
        counts(&outcome.after)
    )
}

pub fn sync_preview(preview: &sync::SyncPreview, hooks: Option<&HooksConfig>) -> String {
    let plan = &preview.plan;
    human::render_preview(
        &sync_action(plan.strategy, &plan.branch, &plan.base, false),
        &[
            ("Path", plan.path.display().to_string()),
            ("Now", counts(&plan.before)),
            (
                "Hooks",
                human::describe_hooks(
                    plan.hook_policy == sync::HookPolicy::Skip,
                    &sync_hooks(hooks),
                ),
            ),
        ],
    )
}

/// Tell the user a path they were given does not exist on disk.
pub fn missing_directory_warning(worktree: &str, path: &Path) -> (String, String) {
    (
        format!(
            "the directory of worktree {worktree} is missing: {}",
            path.display()
        ),
        format!("run `trench remove {worktree}` to clean up its Git entry"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::human::Palette;

    fn failure(
        stage: OperationStage,
        mutation_state: MutationState,
        class: ErrorClass,
        message: &str,
    ) -> OperationFailure {
        OperationFailure {
            stage,
            mutation_state,
            class,
            message: message.to_string(),
            retained_quarantine: None,
        }
    }

    fn plain(report: Report) -> String {
        report.render(Palette::PLAIN)
    }

    #[test]
    fn hook_failures_name_the_hook_and_the_resulting_state() {
        let rolled_back = failure(
            OperationStage::PostHook,
            MutationState::RolledBack,
            ErrorClass::Hook,
            "command failed: `bun install` exited with code 1",
        );
        assert_eq!(
            plain(operation_failure(OperationKind::Create, &rolled_back)),
            "error: post_create hook failed: `bun install` exited with code 1\n\
             hint: nothing was created; the worktree and branch were rolled back"
        );

        let before_change = failure(
            OperationStage::PreHook,
            MutationState::NotStarted,
            ErrorClass::Hook,
            "command failed: `false` exited with code 1",
        );
        assert_eq!(
            plain(operation_failure(OperationKind::Remove, &before_change)),
            "error: pre_remove hook failed: `false` exited with code 1\n\
             hint: nothing was changed"
        );
    }

    #[test]
    fn hook_timeouts_point_at_the_timeout_setting() {
        let timed_out = failure(
            OperationStage::PreHook,
            MutationState::NotStarted,
            ErrorClass::HookTimeout,
            "hook timed out after 1s",
        );
        assert_eq!(
            plain(operation_failure(OperationKind::Create, &timed_out)),
            "error: pre_create hook timed out after 1s\n\
             hint: nothing was changed\n\
             hint: raise timeout_secs in [hooks.pre_create]"
        );
    }

    #[test]
    fn failures_before_any_change_add_no_state_hint() {
        let refused = failure(
            OperationStage::Revalidate,
            MutationState::NotStarted,
            ErrorClass::PreconditionsChanged,
            "removal plan no longer matches live Git state",
        );
        assert_eq!(
            plain(operation_failure(OperationKind::Remove, &refused)),
            "error: removal plan no longer matches live Git state"
        );
    }

    #[test]
    fn partial_failures_say_so_and_name_the_recovery_path() {
        let mut partial = failure(
            OperationStage::RemoveWorktree,
            MutationState::PartiallyApplied,
            ErrorClass::Io,
            "I/O error: Permission denied (os error 13)",
        );
        partial.retained_quarantine = Some("/wt/.trench-remove-1-0".into());
        assert_eq!(
            plain(operation_failure(OperationKind::Remove, &partial)),
            "error: I/O error: Permission denied (os error 13)\n\
             hint: some changes were applied; check the worktree before retrying\n\
             hint: kept for recovery: /wt/.trench-remove-1-0"
        );
    }

    #[test]
    fn post_sync_hook_failure_says_the_sync_succeeded() {
        let failure = sync::SyncFailure {
            stage: sync::SyncStage::PostHook,
            mutation_state: sync::MutationState::Applied,
            class: sync::SyncErrorClass::Hook,
            message: "command failed: `make gen` exited with code 9".to_string(),
            elapsed: std::time::Duration::ZERO,
        };
        assert_eq!(
            plain(sync_failure(&failure)),
            "error: post_sync hook failed: `make gen` exited with code 9\n\
             hint: the sync itself succeeded; only the hook failed"
        );
    }

    #[test]
    fn removal_refusals_say_what_blocks_and_which_flag_unblocks() {
        use RemovalAuthorizationError as Refusal;
        assert_eq!(
            plain(removal_refusal(&Refusal::DirtyWorktree, "feature-auth")),
            "error: worktree feature-auth has uncommitted changes\n\
             hint: commit or stash them, or pass --force-worktree to discard them"
        );
        assert_eq!(
            plain(removal_refusal(
                &Refusal::LockedWorktree {
                    path: "/wt/feature-auth".into()
                },
                "feature-auth"
            )),
            "error: worktree feature-auth is locked\n\
             hint: run `git worktree unlock /wt/feature-auth` first"
        );
        assert_eq!(
            plain(removal_refusal(
                &Refusal::UnmergedBranch {
                    branch: "feature/auth".to_string()
                },
                "feature-auth"
            )),
            "error: branch feature/auth is not merged\n\
             hint: pass --force-branch with --delete-branch to delete it anyway"
        );
        assert_eq!(
            plain(removal_refusal(&Refusal::MainWorktree, "acme")),
            "error: the main worktree cannot be removed"
        );
    }

    #[test]
    fn unknown_worktrees_point_at_list_and_ambiguous_ones_list_their_paths() {
        let not_found = CatalogError::NotFound {
            selector: "nope".to_string(),
        };
        assert_eq!(
            plain(catalog_error(&not_found)),
            "error: worktree not found: nope\n\
             hint: run `trench list` to see worktrees\n\
             hint: a detached worktree is named detached@<commit>"
        );

        let ambiguous = CatalogError::Ambiguous {
            selector: "shared".to_string(),
            matches: vec!["/one/shared".into(), "/two/shared".into()],
        };
        assert_eq!(
            plain(catalog_error(&ambiguous)),
            "error: more than one worktree matches 'shared'\n\
             hint: pass the path to pick one: /one/shared\n\
             hint: pass the path to pick one: /two/shared"
        );
    }

    #[test]
    fn top_level_errors_keep_their_exit_codes() {
        let not_a_repo = anyhow::Error::from(GitError::NotAGitRepo {
            path: "/tmp".into(),
        });
        let (rendered, code) = top_level(&not_a_repo);
        assert_eq!(code, ExitCode::GitError);
        assert!(
            rendered.contains("not a git repository: /tmp"),
            "{rendered}"
        );
        assert!(rendered.contains("hint: run trench inside a Git repository"));

        let missing = anyhow::Error::from(CatalogError::NotFound {
            selector: "nope".to_string(),
        });
        assert_eq!(top_level(&missing).1, ExitCode::NotFound);
        assert_eq!(
            top_level(&anyhow::anyhow!("anything else")).1,
            ExitCode::GeneralError
        );
    }

    fn plan(action: CreateAction) -> CreatePlan {
        CreatePlan {
            dry_run: true,
            action,
            branch: "feature/auth".to_string(),
            worktree: "feature-auth".to_string(),
            path: "/wt/acme/feature-auth".into(),
            base: None,
            tracking: None,
            hook_policy: HookPolicy::Run,
            preconditions: Vec::new(),
            source_oid: None,
        }
    }

    #[test]
    fn create_result_line_states_each_outcome() {
        assert_eq!(
            created(&plan(CreateAction::NewBranch("origin/main".to_string()))),
            "Created feature/auth (new branch from origin/main)"
        );
        assert_eq!(
            created(&plan(CreateAction::ExistingLocal)),
            "Created worktree for existing branch feature/auth"
        );
        assert_eq!(
            created(&plan(CreateAction::TrackRemote(
                "origin/feature/auth".to_string()
            ))),
            "Created feature/auth, tracking origin/feature/auth"
        );
        assert_eq!(
            created(&plan(CreateAction::Navigate("feature-auth".to_string()))),
            "feature/auth already has a worktree; nothing created"
        );
    }

    #[test]
    fn create_preview_describes_the_branch_path_and_hooks() {
        let hooks = HooksConfig {
            post_create: Some(crate::config::HookDef::default()),
            ..HooksConfig::default()
        };
        assert_eq!(
            create_preview(
                &plan(CreateAction::NewBranch("main".to_string())),
                Some(&hooks)
            ),
            "Dry run: nothing was changed.\n\n\
             Would create worktree feature-auth\n\
             \x20 Branch  feature/auth (new, from main)\n\
             \x20 Path    /wt/acme/feature-auth\n\
             \x20 Hooks   post_create\n"
        );

        let mut skipped = plan(CreateAction::ExistingLocal);
        skipped.hook_policy = HookPolicy::Skip;
        let preview = create_preview(&skipped, Some(&hooks));
        assert!(preview.contains("Branch  feature/auth (existing local branch)"));
        assert!(preview.contains("Hooks   skipped (--no-hooks)"));

        assert_eq!(
            create_preview(
                &plan(CreateAction::Navigate("feature-auth".to_string())),
                None
            ),
            "Dry run: nothing was changed.\n\n\
             Nothing to create: feature/auth already has a worktree\n\
             \x20 Branch  feature/auth\n\
             \x20 Path    /wt/acme/feature-auth\n"
        );
    }

    fn outcome(
        strategy: sync::SyncStrategy,
        before: (usize, usize),
        after: (usize, usize),
    ) -> sync::SyncOutcome {
        sync::SyncOutcome {
            target: "feature-auth".to_string(),
            branch: "feature/auth".to_string(),
            path: "/wt/acme/feature-auth".into(),
            base: "origin/main".to_string(),
            strategy,
            before: sync::AheadBehind {
                ahead: before.0,
                behind: before.1,
            },
            after: sync::AheadBehind {
                ahead: after.0,
                behind: after.1,
            },
            mutation_state: sync::MutationState::Applied,
            elapsed: std::time::Duration::ZERO,
        }
    }

    #[test]
    fn sync_result_line_reads_as_a_sentence_for_each_strategy() {
        assert_eq!(
            synced(&outcome(sync::SyncStrategy::Rebase, (2, 5), (2, 0))),
            "Rebased feature/auth onto origin/main. Was 2 ahead, 5 behind; now 2 ahead, 0 behind."
        );
        assert_eq!(
            synced(&outcome(sync::SyncStrategy::Merge, (2, 5), (3, 0))),
            "Merged origin/main into feature/auth. Was 2 ahead, 5 behind; now 3 ahead, 0 behind."
        );
        assert_eq!(
            synced(&outcome(sync::SyncStrategy::Rebase, (2, 0), (2, 0))),
            "feature/auth is already up to date with origin/main"
        );
    }
}
