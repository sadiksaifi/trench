pub mod build_info;
mod cli;
mod config;
mod create_plan;
mod exit_code;
mod git;
mod hooks;
mod installation;
mod logging;
mod navigation;
mod operation;
mod output;
mod paths;
mod ref_catalog;
mod tui;
mod upgrade;
mod worktree_catalog;
mod worktree_policy;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{BufRead, IsTerminal, Write};

use exit_code::ExitCode;

const TUI_SWITCH_PATH_FILE_ENV: &str = "TRENCH_TUI_SWITCH_PATH_FILE";

#[derive(Parser, Debug)]
#[command(
    name = "trench",
    version = build_info::VERSION,
    about = "A fast, ergonomic, headless-first Git worktree manager",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new worktree
    Create {
        /// Branch name for the new worktree
        branch: String,

        /// Base branch to create from (defaults to repo's HEAD branch).
        /// Falls back to origin/<base> if not found locally.
        #[arg(long)]
        from: Option<String>,

        /// Skip all lifecycle hooks (pre_create, post_create)
        #[arg(long)]
        no_hooks: bool,

        /// Preview without executing
        #[arg(long)]
        dry_run: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a worktree
    Remove {
        /// Branch name or sanitized name of the worktree to remove
        branch: String,

        /// Confirm removal without an interactive prompt
        #[arg(long)]
        yes: bool,

        /// Allow removal of a dirty worktree
        #[arg(long)]
        force_worktree: bool,

        /// Also delete the corresponding local branch after removing the worktree
        #[arg(long)]
        delete_branch: bool,

        /// Force deletion of an unmerged local branch
        #[arg(long)]
        force_branch: bool,

        /// Skip all lifecycle hooks (pre_remove, post_remove)
        #[arg(long)]
        no_hooks: bool,

        /// Preview without executing
        #[arg(long)]
        dry_run: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Switch to a worktree
    Switch {
        /// Branch name or sanitized name of the worktree
        branch: String,

        /// Print only the worktree path (for shell integration)
        #[arg(long, hide = true)]
        print_path: bool,
    },
    /// Open a worktree in $EDITOR
    Open {
        /// Branch name or sanitized name of the worktree
        branch: String,
    },
    /// List all worktrees
    List {
        /// Output as JSON
        #[arg(long, conflicts_with = "porcelain")]
        json: bool,

        /// Output in porcelain format
        #[arg(long, conflicts_with = "json")]
        porcelain: bool,
    },
    /// Sync a worktree with its base branch
    Sync {
        /// Branch name, worktree name, or path of the worktree to sync
        branch: String,

        /// Sync strategy: rebase or merge
        #[arg(long)]
        strategy: SyncStrategy,

        /// Base branch or ref to sync onto
        #[arg(long)]
        base: Option<String>,

        /// Skip all lifecycle hooks (pre_sync, post_sync)
        #[arg(long)]
        no_hooks: bool,

        /// Preview without executing
        #[arg(long)]
        dry_run: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Upgrade trench using its owning installation manager
    Upgrade,
    /// Initialize .trench.toml in current directory
    Init {
        /// Overwrite existing .trench.toml
        #[arg(long)]
        force: bool,
    },
    /// Output shell function definition for eval.
    ///
    /// The `tn()` shell function wraps `trench switch --print-path` with `cd`
    /// so you can instantly navigate between worktrees.
    ///
    /// Add this to your shell configuration file:
    ///
    ///   # ~/.bashrc
    ///   eval "$(trench shell-init bash)"
    ///
    ///   # ~/.zshrc
    ///   eval "$(trench shell-init zsh)"
    ///
    ///   # ~/.config/fish/config.fish
    ///   trench shell-init fish | source
    #[command(name = "shell-init")]
    ShellInit {
        /// Target shell
        shell: ShellType,
    },
    /// Generate shell completions for trench
    Completions {
        /// Target shell
        shell: ShellType,
    },
}

/// Supported shells for shell-init and completions
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ShellType {
    Bash,
    Zsh,
    Fish,
}

/// Sync strategy for `trench sync`
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SyncStrategy {
    Rebase,
    Merge,
}

impl Cli {
    fn should_launch_tui(&self, stdin_is_tty: bool, stdout_is_tty: bool) -> bool {
        self.command.is_none() && stdin_is_tty && stdout_is_tty
    }
}

fn main() -> anyhow::Result<()> {
    let read_only_startup = std::env::args_os().any(|argument| argument == "--dry-run");
    if !read_only_startup {
        logging::init();
    }
    let cli = Cli::parse();

    if cli.should_launch_tui(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        match tui::run() {
            Ok(tui::runtime::TuiExit::Switch(path)) => write_tui_switch_path(&path)?,
            Ok(tui::runtime::TuiExit::Quit) => {}
            Err(e) if e.downcast_ref::<config::ConfigError>().is_some() => {
                eprintln!("error: {e}");
                ExitCode::ConfigError.exit();
            }
            Err(e) => return Err(e),
        }
        return Ok(());
    }

    let result = match cli.command {
        Some(Commands::Create {
            branch,
            from,
            no_hooks,
            dry_run,
            json,
        }) => run_create(&branch, from.as_deref(), dry_run, json, no_hooks),
        Some(Commands::Remove {
            branch,
            yes,
            force_worktree,
            delete_branch,
            force_branch,
            no_hooks,
            dry_run,
            json,
        }) => run_remove(RemoveCliRequest {
            identifier: &branch,
            yes,
            force_worktree,
            delete_branch,
            force_branch,
            no_hooks,
            dry_run,
            json,
        }),
        Some(Commands::Switch { branch, print_path }) => run_switch(&branch, print_path),
        Some(Commands::Open { branch }) => run_open(&branch),
        Some(Commands::List { json, porcelain }) => run_list(json, porcelain),
        Some(Commands::Init { force }) => run_init(force),
        Some(Commands::ShellInit { shell }) => {
            print!("{}", cli::commands::shell_init::generate(shell));
            Ok(())
        }
        Some(Commands::Completions { shell }) => {
            cli::commands::completions::generate::<Cli>(shell, &mut std::io::stdout());
            Ok(())
        }
        Some(Commands::Sync {
            branch,
            strategy,
            no_hooks,
            base,
            dry_run,
            json,
        }) => run_sync(&branch, strategy, base.as_deref(), json, dry_run, no_hooks),
        Some(Commands::Upgrade) => run_upgrade(),
        None => {
            anyhow::bail!("TUI requires an interactive terminal (stdin and stdout must be a TTY)");
        }
    };

    // Catch-all: map unhandled typed errors to their exit codes before
    // they fall through to anyhow's default "Error: ..." formatter.
    if let Err(ref e) = result {
        if e.downcast_ref::<config::ConfigError>().is_some() {
            eprintln!("error: {e}");
            ExitCode::ConfigError.exit();
        }
        if let Some(git_error) = e.downcast_ref::<git::GitError>() {
            eprintln!("Error: {e}");
            if matches!(git_error, git::GitError::NotAGitRepo { .. }) {
                eprintln!("hint: Run `trench` inside a Git worktree.");
            }
            ExitCode::GitError.exit();
        }
        if let Some(catalog_error) = e.downcast_ref::<worktree_catalog::CatalogError>() {
            if let worktree_catalog::CatalogError::Git(git_error) = catalog_error {
                eprintln!("Error: {git_error}");
                if matches!(git_error, git::GitError::NotAGitRepo { .. }) {
                    eprintln!("hint: Run `trench` inside a Git worktree.");
                }
                ExitCode::GitError.exit();
            }
            eprintln!("error: {e}");
            ExitCode::NotFound.exit();
        }
    }

    result
}

fn run_upgrade() -> anyhow::Result<()> {
    match upgrade::execute()? {
        upgrade::UpgradeOutcome::Updated { from, to } => {
            println!("Upgraded trench from {from} to {to}");
        }
        upgrade::UpgradeOutcome::AlreadyCurrent { version } => {
            println!("trench {version} is already up to date");
        }
        upgrade::UpgradeOutcome::Homebrew(status) if !status.success() => {
            std::process::exit(status.code().unwrap_or(1));
        }
        upgrade::UpgradeOutcome::Homebrew(_) => {}
    }
    Ok(())
}

fn write_tui_switch_path(path: &std::path::Path) -> anyhow::Result<()> {
    if let Some(sink_path) = std::env::var_os(TUI_SWITCH_PATH_FILE_ENV) {
        std::fs::write(&sink_path, path.as_os_str().as_encoded_bytes()).with_context(|| {
            format!(
                "failed to write TUI switch path to {}",
                std::path::PathBuf::from(&sink_path).display()
            )
        })?;
    } else {
        println!("{}", path.display());
        eprintln!("{}", format_switch_hint());
    }
    Ok(())
}

fn format_switch_hint() -> &'static str {
    "hint: a child process cannot change its parent shell; use `tn switch <worktree>` to cd"
}

fn run_create(
    branch: &str,
    from: Option<&str>,
    dry_run: bool,
    json: bool,
    no_hooks: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;

    // Load config once so both dry-run and actual execution use the same root
    // and hooks.
    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let worktree_root = resolved.worktrees.root;

    if dry_run {
        let plan = cli::commands::create::execute_dry_run(
            branch,
            from,
            &cwd,
            &worktree_root,
            resolved.git.default_base.as_deref(),
            no_hooks,
        )?;

        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            print!("{plan}");
        }
        return Ok(());
    }

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
    let plan = cli::commands::create::execute_dry_run(
        branch,
        from,
        &cwd,
        &worktree_root,
        resolved.git.default_base.as_deref(),
        no_hooks,
    )?;
    let request = operation::OperationRequest::Create(operation::CreateRequest {
        plan,
        repo_path: repo_info.path,
        worktree_root,
        hooks: resolved.hooks,
    });

    match rt.block_on(operation::execute(request, &operation::TerminalEmitter)) {
        Ok(operation::OperationOutcome::Create(outcome)) => {
            if json {
                println!("{}", output::json::format_json_value(&outcome)?);
            } else {
                println!("{}", outcome.plan.path.display());
            }
            Ok(())
        }
        Ok(operation::OperationOutcome::Remove(_)) => {
            unreachable!("create request returned a remove outcome")
        }
        Ok(operation::OperationOutcome::Sync(_)) => {
            unreachable!("create request returned a sync outcome")
        }
        Err(failure) => {
            if json {
                println!("{}", output::json::format_json_value(&failure)?);
            } else {
                eprintln!("error: {failure}");
            }
            match failure.class {
                operation::ErrorClass::Hook => ExitCode::HookFailed.exit(),
                operation::ErrorClass::HookTimeout => ExitCode::HookTimeout.exit(),
                operation::ErrorClass::Git => ExitCode::GitError.exit(),
                operation::ErrorClass::Cancelled
                | operation::ErrorClass::PreconditionsChanged
                | operation::ErrorClass::Cleanup
                | operation::ErrorClass::Io
                | operation::ErrorClass::Internal => ExitCode::GeneralError.exit(),
            }
        }
    }
}

struct RemoveCliRequest<'a> {
    identifier: &'a str,
    yes: bool,
    force_worktree: bool,
    delete_branch: bool,
    force_branch: bool,
    no_hooks: bool,
    dry_run: bool,
    json: bool,
}

fn run_remove(request: RemoveCliRequest<'_>) -> anyhow::Result<()> {
    use cli::commands::remove::stateless::{
        InteractiveTerminal, RemovalAssessment, RemovalAuthorizationError, RemoveOptions,
    };

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let configured_base = resolved.git.default_base.clone();
    let hooks_config = (!request.no_hooks).then_some(resolved.hooks).flatten();
    let assessment =
        match RemovalAssessment::discover(&cwd, request.identifier, configured_base.as_deref()) {
            Ok(assessment) => assessment,
            Err(error) => {
                eprintln!("error: {error}");
                if error.to_string().contains("not found") {
                    ExitCode::NotFound.exit();
                }
                ExitCode::GeneralError.exit();
            }
        };
    let options = RemoveOptions {
        yes: request.yes,
        force_worktree: request.force_worktree,
        delete_branch: request.delete_branch,
        force_branch: request.force_branch,
        no_hooks: request.no_hooks,
        dry_run: request.dry_run,
    };
    let plan = if request.dry_run || request.yes {
        assessment.authorize(options)
    } else if let Some(terminal) = InteractiveTerminal::detect() {
        let prompt = format!(
            "Remove worktree '{}' at {}?",
            assessment.worktree(),
            assessment.path().display()
        );
        match assessment.confirm_interactively(terminal, || {
            prompt_yes_no(&prompt).map_err(|error| std::io::Error::other(error.to_string()))
        })? {
            Some(receipt) => assessment.authorize_confirmed(options, receipt),
            None => {
                eprintln!("Cancelled.");
                return Ok(());
            }
        }
    } else {
        Err(RemovalAuthorizationError::ConfirmationRequired)
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("error: {error}");
            match error {
                RemovalAuthorizationError::ConfirmationRequired => {
                    ExitCode::MissingRequiredFlag.exit()
                }
                RemovalAuthorizationError::ForceBranchRequiresDeleteBranch => {
                    ExitCode::FlagConflict.exit()
                }
                _ => ExitCode::GeneralError.exit(),
            }
        }
    };
    if request.dry_run {
        if request.json {
            println!("{}", output::json::format_json_value(&plan)?);
        } else {
            println!("{plan}");
        }
        return Ok(());
    }

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
    match rt.block_on(operation::execute(
        operation::OperationRequest::Remove(operation::RemoveRequest {
            plan,
            hooks: hooks_config,
        }),
        &operation::TerminalEmitter,
    )) {
        Ok(operation::OperationOutcome::Remove(outcome)) => {
            if request.json {
                println!("{}", output::json::format_json_value(&outcome)?);
            } else {
                eprintln!("{outcome}");
            }
            Ok(())
        }
        Ok(_) => unreachable!("remove request returned a create outcome"),
        Err(failure) => {
            if request.json {
                println!("{}", output::json::format_json_value(&failure)?);
            } else {
                eprintln!("error: {failure}");
            }
            match failure.class {
                operation::ErrorClass::Hook => ExitCode::HookFailed.exit(),
                operation::ErrorClass::HookTimeout => ExitCode::HookTimeout.exit(),
                operation::ErrorClass::Git => ExitCode::GitError.exit(),
                operation::ErrorClass::Cancelled
                | operation::ErrorClass::PreconditionsChanged
                | operation::ErrorClass::Cleanup
                | operation::ErrorClass::Io
                | operation::ErrorClass::Internal => ExitCode::GeneralError.exit(),
            }
        }
    }
}

fn prompt_yes_no(prompt: &str) -> anyhow::Result<bool> {
    let stdin = std::io::stdin();
    let stderr = std::io::stderr();
    let mut input = stdin.lock();
    let mut output = stderr.lock();
    prompt_yes_no_from(prompt, &mut input, &mut output)
}

fn prompt_yes_no_from<R: BufRead, W: Write>(
    prompt: &str,
    input: &mut R,
    output: &mut W,
) -> anyhow::Result<bool> {
    write!(output, "{prompt} [y/N] ")?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
}

fn run_switch(identifier: &str, print_path: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    match cli::commands::switch::execute(identifier, &cwd) {
        Ok(result) => {
            println!("{}", result.path);
            if !print_path {
                eprintln!("{}", format_switch_hint());
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                ExitCode::NotFound.exit();
            }
            Err(e.into())
        }
    }
}

fn run_open(identifier: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let repo_info = git::discover_repo(&cwd)?;

    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let editor_command = resolved.editor_command;

    cli::commands::open::execute(identifier, &cwd, editor_command.as_deref())
}

fn run_list(json: bool, porcelain: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let default_base = resolved.git.default_base.as_deref();

    let output = if json {
        cli::commands::list::execute_json(&cwd, default_base)?
    } else if porcelain {
        cli::commands::list::execute_porcelain(&cwd, default_base)?
    } else {
        cli::commands::list::execute(&cwd, default_base)?
    };
    if output.ends_with('\n') {
        print!("{output}");
    } else {
        println!("{output}");
    }
    Ok(())
}

fn run_sync(
    identifier: &str,
    strategy: SyncStrategy,
    explicit_base: Option<&str>,
    json: bool,
    dry_run: bool,
    no_hooks: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;

    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let hooks_config = if no_hooks { None } else { resolved.hooks };

    if dry_run {
        let strategy = match strategy {
            SyncStrategy::Rebase => cli::commands::sync::stateless::SyncStrategy::Rebase,
            SyncStrategy::Merge => cli::commands::sync::stateless::SyncStrategy::Merge,
        };
        let hook_policy = if no_hooks {
            cli::commands::sync::stateless::HookPolicy::Skip
        } else {
            cli::commands::sync::stateless::HookPolicy::Run
        };
        let planning_started = std::time::Instant::now();
        let plan = match cli::commands::sync::stateless::SyncPlanner::discover(
            &cwd,
            resolved.git.default_base.as_deref(),
        )
        .and_then(|planner| planner.plan(identifier, explicit_base, strategy, hook_policy))
        {
            Ok(plan) => plan,
            Err(error) => {
                return report_sync_failure(
                    &error.into_failure(planning_started.elapsed()),
                    Vec::new(),
                    json,
                    false,
                )
            }
        };
        let preview = cli::commands::sync::stateless::preview(plan);
        if json {
            println!("{}", serde_json::to_string_pretty(&preview)?);
        } else {
            println!("{preview}");
        }
        return Ok(());
    }

    let stateless_strategy = match strategy {
        SyncStrategy::Rebase => cli::commands::sync::stateless::SyncStrategy::Rebase,
        SyncStrategy::Merge => cli::commands::sync::stateless::SyncStrategy::Merge,
    };
    let hook_policy = if no_hooks {
        cli::commands::sync::stateless::HookPolicy::Skip
    } else {
        cli::commands::sync::stateless::HookPolicy::Run
    };
    let emitter = CliSyncEmitter::default();
    let planning_started = std::time::Instant::now();
    let plan = match cli::commands::sync::stateless::plan_after_best_effort_origin_fetch(
        &cwd,
        resolved.git.default_base.as_deref(),
        identifier,
        explicit_base,
        stateless_strategy,
        hook_policy,
        &emitter,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return report_sync_failure(
                &error.into_failure(planning_started.elapsed()),
                emitter.stages(),
                json,
                true,
            )
        }
    };
    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
    match rt.block_on(cli::commands::sync::stateless::execute(
        plan,
        hooks_config.as_ref(),
        &emitter,
    )) {
        Ok(outcome) => {
            if json {
                println!(
                    "{}",
                    output::json::format_json_value(&SyncSuccessOutput::new(
                        &outcome,
                        emitter.stages()
                    ))?
                );
            } else {
                println!("{outcome}");
            }
            Ok(())
        }
        Err(failure) => report_sync_failure(&failure, emitter.stages(), json, true),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct SyncStageOutput {
    stage: cli::commands::sync::stateless::SyncStage,
    success: bool,
}

#[derive(Debug, serde::Serialize)]
struct SyncSuccessOutput<'a> {
    ok: bool,
    target: &'a str,
    branch: &'a str,
    path: &'a std::path::Path,
    base: &'a str,
    strategy: cli::commands::sync::stateless::SyncStrategy,
    before: &'a cli::commands::sync::stateless::AheadBehind,
    after: &'a cli::commands::sync::stateless::AheadBehind,
    mutation_state: cli::commands::sync::stateless::MutationState,
    stages: Vec<SyncStageOutput>,
}

#[derive(Debug, serde::Serialize)]
struct SyncFailureOutput<'a> {
    ok: bool,
    failure: SyncFailureDetail<'a>,
    stages: Vec<SyncStageOutput>,
}

#[derive(Debug, serde::Serialize)]
struct SyncFailureDetail<'a> {
    stage: cli::commands::sync::stateless::SyncStage,
    mutation_state: cli::commands::sync::stateless::MutationState,
    class: cli::commands::sync::stateless::SyncErrorClass,
    message: &'a str,
}

impl<'a> SyncSuccessOutput<'a> {
    fn new(
        outcome: &'a cli::commands::sync::stateless::SyncOutcome,
        stages: Vec<SyncStageOutput>,
    ) -> Self {
        Self {
            ok: true,
            target: &outcome.target,
            branch: &outcome.branch,
            path: &outcome.path,
            base: &outcome.base,
            strategy: outcome.strategy,
            before: &outcome.before,
            after: &outcome.after,
            mutation_state: outcome.mutation_state,
            stages,
        }
    }
}

#[derive(Debug, Default)]
struct CliSyncEmitter(std::sync::Mutex<Vec<cli::commands::sync::stateless::SyncEvent>>);

impl CliSyncEmitter {
    fn stages(&self) -> Vec<SyncStageOutput> {
        self.0
            .lock()
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| match event {
                        cli::commands::sync::stateless::SyncEvent::StageFinished {
                            stage,
                            success,
                            ..
                        } => Some(SyncStageOutput {
                            stage: *stage,
                            success: *success,
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl cli::commands::sync::stateless::SyncEmitter for CliSyncEmitter {
    fn emit(&self, event: cli::commands::sync::stateless::SyncEvent) {
        match &event {
            cli::commands::sync::stateless::SyncEvent::StageFinished {
                stage,
                success: true,
                elapsed,
            } => logging::record(logging::DiagnosticEvent::debug(
                logging::Operation::Sync,
                sync_diagnostic_stage(*stage),
                *elapsed,
            )),
            cli::commands::sync::stateless::SyncEvent::Warning { stage, .. } => {
                logging::record(logging::DiagnosticEvent::warning(
                    logging::Operation::Sync,
                    sync_diagnostic_stage(*stage),
                    std::time::Duration::ZERO,
                    logging::DiagnosticError::Git,
                ));
            }
            _ => {}
        }
        if let cli::commands::sync::stateless::SyncEvent::HookOutput { line, .. } = &event {
            eprintln!("{line}");
        }
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}

fn report_sync_failure(
    failure: &cli::commands::sync::stateless::SyncFailure,
    stages: Vec<SyncStageOutput>,
    json: bool,
    record_diagnostics: bool,
) -> anyhow::Result<()> {
    use cli::commands::sync::stateless::SyncErrorClass;

    if record_diagnostics {
        logging::record(logging::DiagnosticEvent::error(
            logging::Operation::Sync,
            sync_diagnostic_stage(failure.stage),
            failure.elapsed,
            match failure.class {
                SyncErrorClass::InvalidTarget => logging::DiagnosticError::NotFound,
                SyncErrorClass::InvalidBase
                | SyncErrorClass::Dirty
                | SyncErrorClass::Detached
                | SyncErrorClass::OperationInProgress
                | SyncErrorClass::PreconditionsChanged
                | SyncErrorClass::Conflict => logging::DiagnosticError::InvalidInput,
                SyncErrorClass::Git | SyncErrorClass::Rollback => logging::DiagnosticError::Git,
                SyncErrorClass::Hook | SyncErrorClass::HookTimeout => {
                    logging::DiagnosticError::Hook
                }
            },
        ));
    }
    if json {
        println!(
            "{}",
            output::json::format_json_value(&SyncFailureOutput {
                ok: false,
                failure: SyncFailureDetail {
                    stage: failure.stage,
                    mutation_state: failure.mutation_state,
                    class: failure.class,
                    message: &failure.message,
                },
                stages,
            })?
        );
    } else {
        eprintln!("error: {}", failure.message);
    }
    match failure.class {
        SyncErrorClass::InvalidTarget => ExitCode::NotFound,
        SyncErrorClass::Git | SyncErrorClass::Rollback => ExitCode::GitError,
        SyncErrorClass::Hook => ExitCode::HookFailed,
        SyncErrorClass::HookTimeout => ExitCode::HookTimeout,
        SyncErrorClass::InvalidBase
        | SyncErrorClass::Dirty
        | SyncErrorClass::Detached
        | SyncErrorClass::OperationInProgress
        | SyncErrorClass::PreconditionsChanged
        | SyncErrorClass::Conflict => ExitCode::GeneralError,
    }
    .exit()
}

fn sync_diagnostic_stage(stage: cli::commands::sync::stateless::SyncStage) -> logging::Stage {
    match stage {
        cli::commands::sync::stateless::SyncStage::Fetch => logging::Stage::Resolve,
        cli::commands::sync::stateless::SyncStage::Validate => logging::Stage::Validate,
        cli::commands::sync::stateless::SyncStage::PreHook
        | cli::commands::sync::stateless::SyncStage::PostHook => logging::Stage::Hook,
        cli::commands::sync::stateless::SyncStage::Sync
        | cli::commands::sync::stateless::SyncStage::Rollback => logging::Stage::Git,
    }
}

fn run_init(force: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let repo_info = git::discover_repo(&cwd)?;

    match cli::commands::init::execute(&repo_info.path, force) {
        Ok(path) => {
            println!("Created {}", path.display());
            Ok(())
        }
        Err(e) => {
            if e.downcast_ref::<cli::commands::init::InitError>().is_some() {
                eprintln!("error: {e}");
                ExitCode::ConfigError.exit();
            }
            Err(e)
        }
    }
}
