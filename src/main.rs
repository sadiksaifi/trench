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
use output::human::{self, Report};

const TUI_SWITCH_PATH_FILE_ENV: &str = "TRENCH_TUI_SWITCH_PATH_FILE";

#[derive(Parser, Debug)]
#[command(
    name = "trench",
    version = build_info::VERSION,
    about = "Manage Git worktrees: create, switch, sync and remove them",
    after_help = "Run `trench` with no command in a terminal to open the interactive list.\n\n\
        Examples:\n  \
        trench create feature/login\n  \
        trench create feature/login --from develop\n  \
        trench sync feature/login --strategy rebase\n  \
        trench remove feature/login --delete-branch",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a worktree for a new or existing branch
    #[command(
        after_help = "If BRANCH exists locally, trench checks it out. If it exists only on origin,\n\
        trench creates a local branch that tracks it. Otherwise trench creates a new\n\
        branch from BASE. Prints the worktree path on stdout."
    )]
    Create {
        /// Branch to check out in the new worktree
        branch: String,

        /// Start a new branch from BASE [default: git.default_base, else origin's default branch]
        #[arg(long, value_name = "BASE")]
        from: Option<String>,

        /// Do not run pre_create and post_create hooks
        #[arg(long)]
        no_hooks: bool,

        /// Show what would happen and change nothing
        #[arg(long)]
        dry_run: bool,

        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// List worktrees
    List {
        /// Print the worktrees as JSON
        #[arg(long, conflicts_with = "porcelain")]
        json: bool,

        /// Print one `name:branch:path` line per worktree
        #[arg(long, conflicts_with = "json")]
        porcelain: bool,
    },
    /// Print a worktree's path (the `tn` shell function uses it to cd)
    Switch {
        /// Branch name, worktree directory name, or path
        #[arg(value_name = "WORKTREE")]
        branch: String,

        /// Print only the worktree path (for shell integration)
        #[arg(long, hide = true)]
        print_path: bool,
    },
    /// Open a worktree in your editor
    Open {
        /// Branch name, worktree directory name, or path
        #[arg(value_name = "WORKTREE")]
        branch: String,
    },
    /// Rebase or merge a worktree's branch onto its base
    #[command(
        after_help = "Fetches origin first when it can. Refuses a worktree with uncommitted changes\n\
        or a detached HEAD."
    )]
    Sync {
        /// Branch name, worktree directory name, or path
        #[arg(value_name = "WORKTREE")]
        branch: String,

        /// How to bring in the base
        #[arg(long)]
        strategy: SyncStrategy,

        /// Branch or ref to sync onto [default: git.default_base, else origin's default branch]
        #[arg(long)]
        base: Option<String>,

        /// Do not run pre_sync and post_sync hooks
        #[arg(long)]
        no_hooks: bool,

        /// Show what would happen and change nothing
        #[arg(long)]
        dry_run: bool,

        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a worktree, and optionally its branch
    #[command(
        after_help = "Refuses a worktree with uncommitted changes unless --force-worktree is given.\n\
        Asks for confirmation in a terminal; pass --yes in scripts."
    )]
    Remove {
        /// Branch name, worktree directory name, or path
        #[arg(value_name = "WORKTREE")]
        branch: String,

        /// Do not ask for confirmation
        #[arg(long)]
        yes: bool,

        /// Also delete the local branch
        #[arg(long)]
        delete_branch: bool,

        /// Remove even with uncommitted changes, which are lost
        #[arg(long)]
        force_worktree: bool,

        /// With --delete-branch: delete the branch even if it is not merged
        #[arg(long)]
        force_branch: bool,

        /// Do not run pre_remove and post_remove hooks
        #[arg(long)]
        no_hooks: bool,

        /// Show what would happen and change nothing
        #[arg(long)]
        dry_run: bool,

        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Write a commented .trench.toml to the repository root
    Init {
        /// Overwrite an existing .trench.toml
        #[arg(long)]
        force: bool,
    },
    /// Print the `tn` shell function
    ///
    /// The `tn` shell function wraps `trench switch --print-path` with `cd`,
    /// so you can move between worktrees.
    ///
    /// Add one of these to your shell configuration file:
    ///
    ///   # ~/.bashrc
    ///   eval "$(trench shell-init bash)"
    ///
    ///   # ~/.zshrc
    ///   eval "$(trench shell-init zsh)"
    ///
    ///   # ~/.config/fish/config.fish
    ///   trench shell-init fish | source
    #[command(name = "shell-init", verbatim_doc_comment)]
    ShellInit {
        /// Target shell
        shell: ShellType,
    },
    /// Print shell completions
    Completions {
        /// Target shell
        shell: ShellType,
    },
    /// Upgrade trench
    Upgrade,
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
    /// The TUI needs a real terminal on both ends that can address the screen.
    fn should_launch_tui(&self, stdin_is_tty: bool, stdout_is_tty: bool, term: &str) -> bool {
        self.command.is_none() && stdin_is_tty && stdout_is_tty && term != "dumb"
    }
}

fn main() {
    let read_only_startup = std::env::args_os()
        .any(|argument| argument == "--dry-run" || argument == "--version" || argument == "-V");
    if !read_only_startup {
        logging::init();
    }
    if let Err(error) = run(Cli::parse()) {
        let (rendered, code) = cli::report::top_level(&error);
        eprintln!("{rendered}");
        code.exit();
    }
}

/// Print a failure and exit with its code.
fn fail(report: Report, code: ExitCode) -> ! {
    report.print();
    code.exit()
}

fn run(cli: Cli) -> anyhow::Result<()> {
    if cli.should_launch_tui(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        &std::env::var("TERM").unwrap_or_default(),
    ) {
        match tui::run()? {
            tui::runtime::TuiExit::Switch(path) => write_tui_switch_path(&path)?,
            tui::runtime::TuiExit::Quit => {}
        }
        return Ok(());
    }

    match cli.command {
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
        None => fail(
            Report::new("the TUI needs an interactive terminal")
                .hint("run a command instead, for example `trench list`"),
            ExitCode::GeneralError,
        ),
    }
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
        if std::io::stdout().is_terminal() {
            human::print_hint(SWITCH_HINT);
        }
    }
    Ok(())
}

const SWITCH_HINT: &str =
    "trench cannot change your shell's directory; run `tn switch <worktree>` to cd";

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

    let plan = cli::commands::create::execute_dry_run(
        branch,
        from,
        &cwd,
        &worktree_root,
        resolved.git.default_base.as_deref(),
        no_hooks,
    )?;
    if dry_run {
        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            print!(
                "{}",
                cli::report::create_preview(&plan, resolved.hooks.as_ref())
            );
        }
        return Ok(());
    }

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
    let request = operation::OperationRequest::Create(operation::CreateRequest {
        plan,
        repo_path: repo_info.path,
        worktree_root,
        hooks: resolved.hooks,
    });

    match rt.block_on(operation::execute(request, cli_emitter(json).as_ref())) {
        Ok(operation::OperationOutcome::Create(outcome)) => {
            if json {
                println!("{}", output::json::format_json_value(&outcome)?);
            } else {
                eprintln!("{}", cli::report::created(&outcome.plan));
                if !outcome.plan.path.exists() {
                    let (warning, hint) = cli::report::missing_directory_warning(
                        &outcome.plan.worktree,
                        &outcome.plan.path,
                    );
                    human::print_warning(&warning);
                    human::print_hint(&hint);
                }
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
        Err(failure) => exit_operation_failure(operation::OperationKind::Create, &failure, json),
    }
}

/// Hook output goes to stderr raw under `--json` and labeled otherwise.
fn cli_emitter(json: bool) -> Box<dyn operation::Emitter> {
    if json {
        Box::new(operation::TerminalEmitter)
    } else {
        Box::new(operation::HumanEmitter::default())
    }
}

fn exit_operation_failure(
    kind: operation::OperationKind,
    failure: &operation::OperationFailure,
    json: bool,
) -> ! {
    if json {
        match output::json::format_json_value(failure) {
            Ok(rendered) => println!("{rendered}"),
            Err(error) => eprintln!(
                "{}",
                Report::new(error.to_string()).render(human::Palette::stderr())
            ),
        }
    } else {
        cli::report::operation_failure(kind, failure).print();
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
        InteractiveTerminal, RemovalAssessment, RemovalAssessmentError, RemovalAuthorizationError,
        RemoveOptions,
    };

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let configured_base = resolved.git.default_base.clone();
    let hooks_description = human::describe_hooks(
        request.no_hooks,
        &cli::report::remove_hooks(resolved.hooks.as_ref()),
    );
    let hooks_config = (!request.no_hooks).then_some(resolved.hooks).flatten();
    let assessment =
        match RemovalAssessment::discover(&cwd, request.identifier, configured_base.as_deref()) {
            Ok(assessment) => assessment,
            Err(RemovalAssessmentError::Catalog(error)) => return Err(error.into()),
            Err(error) => fail(Report::new(error.to_string()), ExitCode::GeneralError),
        };
    let worktree = assessment.worktree().to_string();
    let options = RemoveOptions {
        yes: request.yes,
        force_worktree: request.force_worktree,
        delete_branch: request.delete_branch,
        force_branch: request.force_branch,
        no_hooks: request.no_hooks,
        dry_run: request.dry_run,
    };
    let refuse = |error: RemovalAuthorizationError| -> ! {
        let code = match error {
            RemovalAuthorizationError::ConfirmationRequired => ExitCode::MissingRequiredFlag,
            RemovalAuthorizationError::ForceBranchRequiresDeleteBranch => ExitCode::FlagConflict,
            _ => ExitCode::GeneralError,
        };
        fail(cli::report::removal_refusal(&error, &worktree), code)
    };
    let plan = if request.dry_run || request.yes {
        assessment.authorize(options)
    } else if let Some(terminal) = InteractiveTerminal::detect() {
        // Report anything that blocks the removal before asking, so the user
        // is never asked to confirm something that will then be refused.
        if let Err(error) = assessment.clone().authorize(RemoveOptions {
            dry_run: true,
            ..options
        }) {
            refuse(error);
        }
        let prompt = assessment.prompt(request.delete_branch);
        match assessment.confirm_interactively(terminal, || {
            prompt_yes_no(&prompt).map_err(|error| std::io::Error::other(error.to_string()))
        })? {
            Some(receipt) => assessment.authorize_confirmed(options, receipt),
            None => {
                eprintln!("Nothing removed.");
                return Ok(());
            }
        }
    } else {
        Err(RemovalAuthorizationError::ConfirmationRequired)
    };
    let plan = plan.unwrap_or_else(|error| refuse(error));
    if request.dry_run {
        if request.json {
            println!("{}", output::json::format_json_value(&plan)?);
        } else {
            print!("{}", plan.render_preview(&hooks_description));
        }
        return Ok(());
    }

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
    match rt.block_on(operation::execute(
        operation::OperationRequest::Remove(operation::RemoveRequest {
            plan,
            hooks: hooks_config,
        }),
        cli_emitter(request.json).as_ref(),
    )) {
        Ok(operation::OperationOutcome::Remove(outcome)) => {
            if request.json {
                println!("{}", output::json::format_json_value(&outcome)?);
            } else {
                for line in outcome.summary_lines() {
                    eprintln!("{line}");
                }
                if let Some(warning) = &outcome.warning {
                    human::print_warning(warning);
                }
            }
            Ok(())
        }
        Ok(_) => unreachable!("remove request returned a create outcome"),
        Err(failure) => {
            exit_operation_failure(operation::OperationKind::Remove, &failure, request.json)
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
    let result = cli::commands::switch::execute(identifier, &cwd)?;
    println!("{}", result.path);
    // A person who typed `trench switch` at a prompt expects to move; a
    // script or `cd "$(trench switch x)"` only wants the path.
    if !print_path && std::io::stdout().is_terminal() {
        human::print_hint(SWITCH_HINT);
    }
    Ok(())
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
    if json {
        println!("{}", cli::commands::list::execute_json(&cwd)?);
        return Ok(());
    }
    if porcelain {
        print!("{}", cli::commands::list::execute_porcelain(&cwd)?);
        return Ok(());
    }
    let interactive = std::io::stdout().is_terminal();
    let home = paths::home_dir();
    let layout = if interactive {
        cli::commands::list::HumanLayout {
            terminal_width: crossterm::terminal::size()
                .ok()
                .map(|(columns, _)| usize::from(columns)),
            home: home.as_deref(),
            palette: output::human::Palette::stdout(),
        }
    } else {
        cli::commands::list::HumanLayout::PIPED
    };
    let list = cli::commands::list::execute(&cwd, layout)?;
    print!("{}", list.text);
    if interactive && list.worktrees == 1 {
        output::human::print_hint("create a worktree with `trench create <branch>`");
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
                let report = cli::report::sync_plan_error(&error);
                return report_sync_failure(
                    &error.into_failure(planning_started.elapsed()),
                    report,
                    Vec::new(),
                    json,
                    false,
                );
            }
        };
        let preview = cli::commands::sync::stateless::preview(plan);
        if json {
            println!("{}", serde_json::to_string_pretty(&preview)?);
        } else {
            print!(
                "{}",
                cli::report::sync_preview(&preview, hooks_config.as_ref())
            );
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
    let emitter = CliSyncEmitter::new(json);
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
            let report = cli::report::sync_plan_error(&error);
            return report_sync_failure(
                &error.into_failure(planning_started.elapsed()),
                report,
                emitter.stages(),
                json,
                true,
            );
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
                eprintln!("{}", cli::report::synced(&outcome));
            }
            Ok(())
        }
        Err(failure) => report_sync_failure(&failure, None, emitter.stages(), json, true),
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

#[derive(Debug)]
struct CliSyncEmitter {
    events: std::sync::Mutex<Vec<cli::commands::sync::stateless::SyncEvent>>,
    /// Labels hook output and prints warnings; `None` under `--json`, where
    /// stderr carries raw hook lines only.
    human: Option<operation::HookOutputLabels>,
}

impl CliSyncEmitter {
    fn new(json: bool) -> Self {
        Self {
            events: std::sync::Mutex::default(),
            human: (!json).then(operation::HookOutputLabels::default),
        }
    }

    fn stages(&self) -> Vec<SyncStageOutput> {
        self.events
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
        match (&event, &self.human) {
            (
                cli::commands::sync::stateless::SyncEvent::HookOutput { hook, line, .. },
                Some(labels),
            ) => {
                labels.print(hook, line);
            }
            (cli::commands::sync::stateless::SyncEvent::HookOutput { line, .. }, None) => {
                eprintln!("{line}");
            }
            (cli::commands::sync::stateless::SyncEvent::Warning { message, .. }, Some(_)) => {
                human::print_warning(message);
            }
            _ => {}
        }
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

/// `planning_report` carries the typed wording of an error raised before a
/// plan existed; execution failures are worded from `failure` itself.
fn report_sync_failure(
    failure: &cli::commands::sync::stateless::SyncFailure,
    planning_report: Option<Report>,
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
        planning_report
            .unwrap_or_else(|| cli::report::sync_failure(failure))
            .print();
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
                fail(Report::new(e.to_string()), ExitCode::ConfigError);
            }
            Err(e)
        }
    }
}
