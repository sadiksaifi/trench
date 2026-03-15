mod adopt;
mod cli;
mod config;
mod exit_code;
mod git;
mod hooks;
mod output;
mod paths;
mod state;
mod tui;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;

use exit_code::ExitCode;

use output::OutputConfig;

#[derive(Parser, Debug)]
#[command(
    name = "trench",
    version,
    about = "A fast, ergonomic, headless-first Git worktree manager"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,

    /// Output in porcelain format
    #[arg(long, global = true, conflicts_with = "json")]
    porcelain: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Preview without executing
    #[arg(long, global = true)]
    dry_run: bool,
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
    },
    /// Remove a worktree
    Remove {
        /// Branch name or sanitized name of the worktree to remove
        branch: String,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,

        /// Also delete the corresponding remote branch
        #[arg(long)]
        prune: bool,

        /// Skip all lifecycle hooks (pre_remove, post_remove)
        #[arg(long)]
        no_hooks: bool,
    },
    /// Switch to a worktree
    Switch {
        /// Branch name or sanitized name of the worktree
        branch: String,

        /// Print only the worktree path (for shell integration)
        #[arg(long)]
        print_path: bool,
    },
    /// Manage tags on a worktree
    Tag {
        /// Branch name or sanitized name of the worktree
        branch: String,

        /// Tags to add (+name) or remove (-name). No arguments = list current tags
        #[arg(allow_hyphen_values = true)]
        tags: Vec<String>,
    },
    /// Open a worktree in $EDITOR
    Open {
        /// Branch name or sanitized name of the worktree
        branch: String,
    },
    /// List all worktrees
    List {
        /// Filter worktrees by tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// Show worktree status
    Status {
        /// Branch name or sanitized name for deep status view.
        /// Omit for summary of all worktrees.
        branch: Option<String>,
    },
    /// Sync a worktree with its base branch
    Sync {
        /// Branch name or sanitized name of the worktree to sync.
        /// Omit when using --all.
        branch: Option<String>,

        /// Sync all active worktrees. Requires --strategy.
        #[arg(long)]
        all: bool,

        /// Sync strategy: rebase or merge. Prompts interactively if omitted.
        #[arg(long)]
        strategy: Option<SyncStrategy>,

        /// Skip all lifecycle hooks (pre_sync, post_sync)
        #[arg(long)]
        no_hooks: bool,
    },
    /// View event log
    Log,
    /// Initialize .trench.toml in current directory
    Init {
        /// Overwrite existing .trench.toml
        #[arg(long)]
        force: bool,
    },
    /// Output shell function definition for eval.
    ///
    /// The `tr()` shell function wraps `trench switch --print-path` with `cd`
    /// so you can instantly navigate between worktrees.
    ///
    /// Note: this will shadow the POSIX `tr` utility (translate characters).
    /// To access the original, use `command tr`.
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
    fn output_config(&self) -> OutputConfig {
        let is_tty = std::io::stdout().is_terminal();
        OutputConfig::from_env(self.no_color, self.quiet, self.verbose, is_tty)
    }

    fn should_launch_tui(&self, stdin_is_tty: bool, stdout_is_tty: bool) -> bool {
        self.command.is_none() && stdin_is_tty && stdout_is_tty
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let output_config = cli.output_config();

    if cli.should_launch_tui(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        return tui::run();
    }

    let dry_run = cli.dry_run;
    let json = cli.json;
    let porcelain = cli.porcelain;

    let result = match cli.command {
        Some(Commands::Create {
            branch,
            from,
            no_hooks,
        }) => run_create(&branch, from.as_deref(), dry_run, json, no_hooks),
        Some(Commands::Remove {
            branch,
            force,
            prune,
            no_hooks,
        }) => run_remove(&branch, force, prune, no_hooks),
        Some(Commands::Switch { branch, print_path }) => run_switch(&branch, print_path),
        Some(Commands::Tag { branch, tags }) => run_tag(&branch, &tags),
        Some(Commands::Open { branch }) => run_open(&branch),
        Some(Commands::List { tag }) => run_list(tag.as_deref(), json, porcelain),
        Some(Commands::Status { branch }) => run_status(
            branch.as_deref(),
            json,
            porcelain,
            output_config.should_color(),
        ),
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
            all,
            strategy,
            no_hooks,
        }) => {
            if all && branch.is_some() {
                eprintln!("error: <BRANCH> cannot be used with --all");
                ExitCode::GeneralError.exit();
            }
            if all {
                if strategy.is_none() {
                    eprintln!("error: {}", cli::commands::sync::BatchSyncMissingStrategy);
                    ExitCode::MissingRequiredFlag.exit();
                }
                run_sync_all(strategy.unwrap(), json, dry_run, no_hooks)
            } else {
                let branch = branch.unwrap_or_else(|| {
                    eprintln!("error: <BRANCH> is required when --all is not set");
                    ExitCode::GeneralError.exit();
                });
                run_sync(&branch, strategy, json, dry_run, no_hooks)
            }
        }
        Some(Commands::Log) => {
            // Log command not yet implemented
            Ok(())
        }
        None => {
            anyhow::bail!("TUI requires an interactive terminal (stdin and stdout must be a TTY)");
        }
    };

    // Catch-all: map unhandled typed errors to their exit codes before
    // they fall through to anyhow's default "Error: ..." formatter.
    if let Err(ref e) = result {
        if e.downcast_ref::<git::GitError>().is_some() {
            eprintln!("Error: {e}");
            ExitCode::GitError.exit();
        }
    }

    result
}

fn run_create(
    branch: &str,
    from: Option<&str>,
    dry_run: bool,
    json: bool,
    no_hooks: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;

    // Load config once so both dry-run and actual execution use the same
    // resolved template and hooks.
    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);

    if dry_run {
        // Use the non-mutating path accessor — dry-run must not create dirs.
        let worktree_root = paths::worktree_root_path()?;
        let plan = cli::commands::create::execute_dry_run(
            branch,
            from,
            &cwd,
            &worktree_root,
            &resolved.worktrees.root,
            resolved.hooks.as_ref(),
        )?;

        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            print!("{plan}");
        }
        return Ok(());
    }

    // Only real execution creates the worktree root directory on disk.
    let worktree_root = paths::worktree_root()?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;

    match rt.block_on(cli::commands::create::execute_with_hooks(
        branch,
        from,
        &cwd,
        &worktree_root,
        &resolved.worktrees.root,
        &db,
        resolved.hooks.as_ref(),
        no_hooks,
    )) {
        Ok(outcome) => {
            // Report post_create hook failure to stderr
            if let Some(ref hook_err) = outcome.post_create_error {
                eprintln!("error: post_create hook failed: {hook_err:#}");
            }

            if json {
                let json_output = outcome.result.to_json_output(outcome.hooks_status);
                println!("{}", output::json::format_json_value(&json_output)?);
            } else {
                println!("{}", outcome.result.path.display());
            }

            // Exit 4 if post_create hook failed (FR-24: hard stop)
            if let Some(ref hook_err) = outcome.post_create_error {
                if hook_err
                    .chain()
                    .any(|c| c.downcast_ref::<hooks::runner::HookTimeoutError>().is_some())
                {
                    ExitCode::HookTimeout.exit();
                }
                ExitCode::HookFailed.exit();
            }
            Ok(())
        }
        Err(e) => {
            // Check for hook timeout first (more specific than hook failure)
            if e.chain()
                .any(|c| c.downcast_ref::<hooks::runner::HookTimeoutError>().is_some())
            {
                eprintln!("error: {e:#}");
                ExitCode::HookTimeout.exit();
            }
            // Check for hook failure (pre_create) via typed error
            if e.downcast_ref::<cli::commands::create::CreateError>()
                .is_some()
            {
                eprintln!("error: {e:#}");
                ExitCode::HookFailed.exit();
            }
            if let Some(git_err) = e.downcast_ref::<git::GitError>() {
                match git_err {
                    git::GitError::BranchAlreadyExists { .. }
                    | git::GitError::RemoteBranchAlreadyExists { .. } => {
                        eprintln!("error: {e}");
                        ExitCode::BranchExists.exit();
                    }
                    git::GitError::BaseBranchNotFound { .. } => {
                        eprintln!("error: {e}");
                        ExitCode::NotFound.exit();
                    }
                    _ => {}
                }
            }
            Err(e)
        }
    }
}

fn run_remove(identifier: &str, force: bool, prune: bool, no_hooks: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let repo_info = git::discover_repo(&cwd)?;

    // Skip config I/O when --no-hooks is set (escape hatch)
    let hooks_config = if no_hooks {
        None
    } else {
        let project_config = config::load_project_config(&repo_info.path)?;
        let global_config = config::load_global_config()?;
        config::resolve_config(None, project_config.as_ref(), &global_config).hooks
    };

    // If not forced, resolve the worktree (adopting if unmanaged) for the prompt
    let resolved = if !force {
        if let Ok((repo, wt)) = adopt::resolve_or_adopt(identifier, &repo_info, &db) {
            let prune_hint = if prune {
                " (including remote branch)"
            } else {
                ""
            };
            eprint!(
                "Remove worktree '{}' at {}{}? [y/N] ",
                wt.name, wt.path, prune_hint
            );
            let mut input = String::new();
            std::io::stdin()
                .read_line(&mut input)
                .context("failed to read confirmation input")?;
            if !input.trim().eq_ignore_ascii_case("y") {
                eprintln!("Cancelled.");
                return Ok(());
            }
            Some((repo, wt))
        } else {
            None
        }
    } else {
        None
    };

    // Resolve worktree if not already done by the prompt flow
    let (repo, wt) = match resolved {
        Some((repo, wt)) => (repo, wt),
        None => adopt::resolve_or_adopt(identifier, &repo_info, &db)?,
    };

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;

    match rt.block_on(cli::commands::remove::execute_resolved_with_hooks(
        &repo,
        &wt,
        &repo_info,
        &db,
        prune,
        hooks_config.as_ref(),
        no_hooks,
    )) {
        Ok(outcome) => {
            // Report post_remove hook failure as warning (FR-24: WarnOnly)
            if let Some(ref hook_err) = outcome.post_remove_warning {
                eprintln!("warning: post_remove hook failed: {hook_err:#}");
            }

            if outcome.result.pruned_remote {
                eprintln!(
                    "Removed worktree '{}' and remote branch",
                    outcome.result.name
                );
            } else {
                eprintln!("Removed worktree '{}'", outcome.result.name);
            }
            Ok(())
        }
        Err(e) => {
            // Check for hook timeout first (more specific than hook failure)
            if e.chain()
                .any(|c| c.downcast_ref::<hooks::runner::HookTimeoutError>().is_some())
            {
                eprintln!("error: {e:#}");
                ExitCode::HookTimeout.exit();
            }
            // Check for pre_remove hook failure → exit code 4
            if e.downcast_ref::<cli::commands::remove::RemoveError>()
                .is_some()
            {
                eprintln!("error: {e:#}");
                ExitCode::HookFailed.exit();
            }
            if let Some(git_err) = e.downcast_ref::<git::GitError>() {
                if matches!(git_err, git::GitError::WorktreeNotFound { .. }) {
                    eprintln!("error: {e}");
                    ExitCode::NotFound.exit();
                }
            }
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                ExitCode::NotFound.exit();
            }
            Err(e)
        }
    }
}

fn run_switch(identifier: &str, print_path: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    match cli::commands::switch::execute(identifier, &cwd, &db) {
        Ok(result) => {
            if print_path {
                println!("{}", result.path);
            } else {
                println!("Switched to worktree '{}' at {}", result.name, result.path);
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                ExitCode::NotFound.exit();
            }
            Err(e)
        }
    }
}

fn run_open(identifier: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);

    match cli::commands::open::resolve(identifier, &cwd, &db, resolved.editor_command.as_deref()) {
        Ok(result) => {
            let parts = shell_words::split(&result.editor)
                .with_context(|| format!("invalid editor command: '{}'", result.editor))?;
            let (program, args) = parts
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("editor command is empty after parsing"))?;

            let status = std::process::Command::new(program)
                .args(args)
                .arg(&result.path)
                .status()
                .with_context(|| format!("failed to launch editor '{}'", result.editor))?;

            if !status.success() {
                ExitCode::GeneralError.exit();
            }

            // Record DB side-effects only after a successful launch
            cli::commands::open::record_open(&db, result.repo_id, result.wt_id)?;

            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                ExitCode::NotFound.exit();
            }
            Err(e)
        }
    }
}

fn run_tag(identifier: &str, tags: &[String]) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let output = cli::commands::tag::execute(identifier, tags, &cwd, &db)?;
    print!("{output}");
    Ok(())
}

fn run_list(tag: Option<&str>, json: bool, porcelain: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    // Load config to get scan paths (FR-30)
    let repo_info = git::discover_repo(&cwd)?;
    let project_config = config::load_project_config(&repo_info.path)?;
    let global_config = config::load_global_config()?;
    let resolved = config::resolve_config(None, project_config.as_ref(), &global_config);
    let scan_paths: Vec<String> = resolved
        .worktrees
        .scan
        .iter()
        .map(|p| paths::expand_tilde(p))
        .collect();

    let output = if json {
        cli::commands::list::execute_json(&cwd, &db, tag, &scan_paths)?
    } else if porcelain {
        cli::commands::list::execute_porcelain(&cwd, &db, tag, &scan_paths)?
    } else {
        cli::commands::list::execute(&cwd, &db, tag, &scan_paths)?
    };
    if output.ends_with('\n') {
        print!("{output}");
    } else {
        println!("{output}");
    }
    Ok(())
}

fn run_status(
    branch: Option<&str>,
    json: bool,
    porcelain: bool,
    use_color: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let result = if json {
        cli::commands::status::execute_json(&cwd, &db, branch)
    } else if porcelain {
        cli::commands::status::execute_porcelain(&cwd, &db, branch)
    } else {
        cli::commands::status::execute(&cwd, &db, branch, use_color)
    };

    match result {
        Ok(output) => {
            if output.ends_with('\n') {
                print!("{output}");
            } else {
                println!("{output}");
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") {
                eprintln!("error: {e}");
                ExitCode::NotFound.exit();
            }
            Err(e)
        }
    }
}

fn run_sync(
    identifier: &str,
    strategy: Option<SyncStrategy>,
    json: bool,
    dry_run: bool,
    no_hooks: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;

    // Determine strategy: use CLI flag, or prompt interactively
    // This runs BEFORE any DB work so dry-run can fail fast.
    let resolved_strategy = match strategy {
        Some(s) => s,
        None => {
            if dry_run {
                eprintln!("error: --strategy is required with --dry-run (use --strategy rebase or --strategy merge)");
                ExitCode::MissingRequiredFlag.exit();
            }
            if !std::io::stdin().is_terminal() {
                eprintln!("error: --strategy is required in non-interactive mode (use --strategy rebase or --strategy merge)");
                ExitCode::MissingRequiredFlag.exit();
            }
            eprint!("Sync strategy — (r)ebase or (m)erge? ");
            let mut input = String::new();
            std::io::stdin()
                .read_line(&mut input)
                .context("failed to read strategy input")?;
            match input.trim().to_lowercase().as_str() {
                "r" | "rebase" => SyncStrategy::Rebase,
                "m" | "merge" => SyncStrategy::Merge,
                other => {
                    eprintln!("error: unknown strategy '{other}'. Use 'rebase' or 'merge'.");
                    ExitCode::GeneralError.exit();
                }
            }
        }
    };

    let sync_strategy = match resolved_strategy {
        SyncStrategy::Rebase => cli::commands::sync::Strategy::Rebase,
        SyncStrategy::Merge => cli::commands::sync::Strategy::Merge,
    };

    // Load hooks config (needed for both dry-run preview and actual execution)
    let hooks_config = if no_hooks {
        None
    } else {
        let repo_info = git::discover_repo(&cwd)?;
        let project_config = config::load_project_config(&repo_info.path)?;
        let global_config = config::load_global_config()?;
        config::resolve_config(None, project_config.as_ref(), &global_config).hooks
    };

    // Dry-run: open existing DB (read-only) for accurate base-branch metadata
    if dry_run {
        let db_path = paths::data_dir_path()?.join("trench.db");
        let db = if db_path.exists() {
            Some(state::Database::open(&db_path)?)
        } else {
            None
        };
        let plan = cli::commands::sync::execute_dry_run(
            identifier,
            &cwd,
            db.as_ref(),
            sync_strategy,
            hooks_config.as_ref(),
            no_hooks,
        )?;
        if json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            print!("{plan}");
        }
        return Ok(());
    }

    // Real execution path — open DB here (after dry-run early-return)
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;

    match rt.block_on(cli::commands::sync::execute_with_hooks(
        identifier,
        &cwd,
        &db,
        sync_strategy,
        hooks_config.as_ref(),
        no_hooks,
    )) {
        Ok(outcome) => {
            // Report post_sync hook failure to stderr (FR-24: Report)
            if let Some(ref hook_err) = outcome.post_sync_error {
                eprintln!("error: post_sync hook failed: {hook_err:#}");
            }

            if json {
                println!(
                    "{}",
                    output::json::format_json_value(&outcome.result.to_json())?
                );
            } else {
                eprintln!(
                    "Synced '{}' via {}",
                    outcome.result.name, outcome.result.strategy
                );
                eprintln!(
                    "  before: ahead={}, behind={}",
                    outcome.result.before_ahead, outcome.result.before_behind
                );
                eprintln!(
                    "  after:  ahead={}, behind={}",
                    outcome.result.after_ahead, outcome.result.after_behind
                );
            }

            // Exit 4 if post_sync hook failed (FR-24: Report — non-zero exit but sync completed)
            if let Some(ref hook_err) = outcome.post_sync_error {
                if hook_err
                    .chain()
                    .any(|c| c.downcast_ref::<hooks::runner::HookTimeoutError>().is_some())
                {
                    ExitCode::HookTimeout.exit();
                }
                ExitCode::HookFailed.exit();
            }
            Ok(())
        }
        Err(e) => {
            // Check for hook timeout first (more specific than hook failure)
            if e.chain()
                .any(|c| c.downcast_ref::<hooks::runner::HookTimeoutError>().is_some())
            {
                eprintln!("error: {e:#}");
                ExitCode::HookTimeout.exit();
            }
            // Check for hook failure (pre_sync) via typed error
            if e.downcast_ref::<cli::commands::sync::SyncError>().is_some() {
                eprintln!("error: {e:#}");
                ExitCode::HookFailed.exit();
            }
            if let Some(git::GitError::MergeConflict { .. }) = e.downcast_ref::<git::GitError>() {
                eprintln!("error: {e}");
                ExitCode::GitError.exit();
            }
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                ExitCode::NotFound.exit();
            }
            Err(e)
        }
    }
}

fn run_sync_all(
    strategy: SyncStrategy,
    json: bool,
    dry_run: bool,
    no_hooks: bool,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir_path()?.join("trench.db");

    // If dry-run and DB doesn't exist yet, there are no tracked worktrees
    if dry_run && !db_path.exists() {
        if json {
            println!("[]");
        } else {
            eprintln!("No active worktrees to sync.");
        }
        return Ok(());
    }

    let db = state::Database::open(&db_path)?;
    let repo_info = git::discover_repo(&cwd)?;

    let db_repo = db
        .get_repo_by_path(repo_info.path.to_str().unwrap_or_default())?
        .ok_or_else(|| anyhow::anyhow!("repo not tracked by trench"))?;

    let worktrees = db.list_worktrees(db_repo.id)?;

    if worktrees.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("No active worktrees to sync.");
        }
        return Ok(());
    }

    let sync_strategy = match strategy {
        SyncStrategy::Rebase => cli::commands::sync::Strategy::Rebase,
        SyncStrategy::Merge => cli::commands::sync::Strategy::Merge,
    };

    // Load hooks config (needed for both dry-run preview and actual execution)
    let hooks_config = if no_hooks {
        None
    } else {
        let project_config = config::load_project_config(&repo_info.path)?;
        let global_config = config::load_global_config()?;
        config::resolve_config(None, project_config.as_ref(), &global_config).hooks
    };

    // Dry-run: show per-worktree plans and exit
    if dry_run {
        let plans = cli::commands::sync::execute_all_dry_run(
            &worktrees,
            &db_repo,
            &repo_info,
            sync_strategy,
            hooks_config.as_ref(),
            no_hooks,
        );
        if json {
            println!("{}", serde_json::to_string_pretty(&plans)?);
        } else {
            for plan in &plans {
                print!("{plan}");
            }
        }
        return Ok(());
    }

    let has_hooks = !no_hooks
        && hooks_config
            .as_ref()
            .map(|h| h.pre_sync.is_some() || h.post_sync.is_some())
            .unwrap_or(false);

    let results = if has_hooks {
        // Run with hooks per worktree
        let rt = tokio::runtime::Runtime::new().context("failed to create async runtime")?;
        let mut entries = Vec::new();
        for wt in &worktrees {
            match rt.block_on(cli::commands::sync::execute_with_hooks(
                &wt.branch,
                &cwd,
                &db,
                sync_strategy,
                hooks_config.as_ref(),
                no_hooks,
            )) {
                Ok(outcome) => {
                    if let Some(ref hook_err) = outcome.post_sync_error {
                        eprintln!(
                            "error: post_sync hook failed for '{}': {hook_err:#}",
                            wt.name
                        );
                    }
                    let has_hook_error = outcome.post_sync_error.is_some();
                    entries.push(cli::commands::sync::BatchSyncEntry {
                        name: wt.name.clone(),
                        status: if has_hook_error {
                            cli::commands::sync::BatchSyncStatus::Failure
                        } else {
                            cli::commands::sync::BatchSyncStatus::Success
                        },
                        result: Some(outcome.result),
                        error: outcome
                            .post_sync_error
                            .map(|e| format!("post_sync hook failed: {e:#}")),
                    });
                }
                Err(e) => {
                    eprintln!("error: sync failed for '{}': {e:#}", wt.name);
                    entries.push(cli::commands::sync::BatchSyncEntry {
                        name: wt.name.clone(),
                        status: cli::commands::sync::BatchSyncStatus::Failure,
                        result: None,
                        error: Some(format!("{e:#}")),
                    });
                }
            }
        }
        entries
    } else {
        // No hooks — use the batch function directly
        cli::commands::sync::execute_all(&worktrees, &db_repo, &repo_info, &db, sync_strategy)
    };

    // Output results
    let has_failures = results
        .iter()
        .any(|r| r.status != cli::commands::sync::BatchSyncStatus::Success);

    if json {
        let json_results: Vec<cli::commands::sync::BatchSyncEntryJson> =
            results.iter().map(|e| e.to_json()).collect();
        println!("{}", output::json::format_json(&json_results)?);
    } else {
        for entry in &results {
            if let Some(ref result) = entry.result {
                eprintln!("Synced '{}' via {}", entry.name, result.strategy);
                eprintln!(
                    "  before: ahead={}, behind={}",
                    result.before_ahead, result.before_behind
                );
                eprintln!(
                    "  after:  ahead={}, behind={}",
                    result.after_ahead, result.after_behind
                );
            } else if let Some(ref err) = entry.error {
                eprintln!("Failed '{}': {err}", entry.name);
            }
        }
        let success = results
            .iter()
            .filter(|r| r.status == cli::commands::sync::BatchSyncStatus::Success)
            .count();
        let failed = results
            .iter()
            .filter(|r| r.status == cli::commands::sync::BatchSyncStatus::Failure)
            .count();
        eprintln!(
            "\nBatch sync: {success} succeeded, {failed} failed ({} total)",
            results.len()
        );
    }

    if has_failures {
        ExitCode::GeneralError.exit();
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_flag_returns_version() {
        let result = Cli::try_parse_from(["trench", "--version"]);
        let err = result.unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        let output = err.to_string();
        assert!(
            output.contains(env!("CARGO_PKG_VERSION")),
            "Expected version output to contain '{}', got: {}",
            env!("CARGO_PKG_VERSION"),
            output
        );
    }

    #[test]
    fn global_flags_are_accepted() {
        let cli = Cli::try_parse_from([
            "trench",
            "--json",
            "--no-color",
            "--quiet",
            "--verbose",
            "--dry-run",
        ])
        .expect("all global flags should be accepted");

        assert!(cli.json);
        assert!(cli.no_color);
        assert!(cli.quiet);
        assert!(cli.verbose);
        assert!(cli.dry_run);

        let cli2 = Cli::try_parse_from(["trench", "--porcelain"])
            .expect("porcelain flag should be accepted");
        assert!(cli2.porcelain);
    }

    #[test]
    fn json_and_porcelain_conflict() {
        let result = Cli::try_parse_from(["trench", "--json", "--porcelain"]);
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "expected ArgumentConflict, got: {err}"
        );
    }

    #[test]
    fn global_flags_short_forms() {
        let cli =
            Cli::try_parse_from(["trench", "-q", "-v"]).expect("short flags should be accepted");

        assert!(cli.quiet);
        assert!(cli.verbose);
    }

    #[test]
    fn global_flags_default_to_false() {
        let cli = Cli::try_parse_from(["trench"]).expect("no flags should parse fine");

        assert!(!cli.json);
        assert!(!cli.porcelain);
        assert!(!cli.no_color);
        assert!(!cli.quiet);
        assert!(!cli.verbose);
        assert!(!cli.dry_run);
    }

    #[test]
    fn all_subcommands_are_accepted() {
        // open, switch, and remove require a branch argument, so test them separately
        let subcommands = ["list", "status", "log", "init"];
        // shell-init and completions require a shell argument
        for sub in ["shell-init", "completions"] {
            let result = Cli::try_parse_from(["trench", sub, "bash"]);
            assert!(
                result.is_ok(),
                "subcommand '{sub}' with shell should be accepted, got: {:?}",
                result.unwrap_err()
            );
        }
        for sub in subcommands {
            let result = Cli::try_parse_from(["trench", sub]);
            assert!(
                result.is_ok(),
                "subcommand '{}' should be accepted, got: {:?}",
                sub,
                result.unwrap_err()
            );
        }
        // open, remove, and switch need a branch arg
        let result = Cli::try_parse_from(["trench", "open", "my-feature"]);
        assert!(result.is_ok(), "open with branch should be accepted");
        let result = Cli::try_parse_from(["trench", "remove", "my-feature"]);
        assert!(result.is_ok(), "remove with branch should be accepted");
        let result = Cli::try_parse_from(["trench", "switch", "my-feature"]);
        assert!(result.is_ok(), "switch with branch should be accepted");
        let result = Cli::try_parse_from(["trench", "sync", "my-feature"]);
        assert!(result.is_ok(), "sync with branch should be accepted");
    }

    #[test]
    fn open_subcommand_requires_branch() {
        let result = Cli::try_parse_from(["trench", "open"]);
        assert!(result.is_err(), "open without branch should fail");
    }

    #[test]
    fn open_subcommand_accepts_branch() {
        let cli = Cli::try_parse_from(["trench", "open", "my-feature"])
            .expect("open with branch should succeed");
        match cli.command {
            Some(Commands::Open { branch }) => {
                assert_eq!(branch, "my-feature");
            }
            _ => panic!("expected Commands::Open"),
        }
    }

    #[test]
    fn status_subcommand_accepts_optional_branch() {
        // No branch → summary mode
        let cli = Cli::try_parse_from(["trench", "status"])
            .expect("status without branch should succeed");
        match cli.command {
            Some(Commands::Status { branch }) => assert!(branch.is_none()),
            _ => panic!("expected Commands::Status"),
        }

        // With branch → deep mode
        let cli = Cli::try_parse_from(["trench", "status", "my-feature"])
            .expect("status with branch should succeed");
        match cli.command {
            Some(Commands::Status { branch }) => {
                assert_eq!(branch.as_deref(), Some("my-feature"));
            }
            _ => panic!("expected Commands::Status"),
        }
    }

    #[test]
    fn create_subcommand_requires_branch() {
        let result = Cli::try_parse_from(["trench", "create"]);
        assert!(result.is_err(), "create without branch should fail");
    }

    #[test]
    fn create_subcommand_accepts_branch() {
        let cli = Cli::try_parse_from(["trench", "create", "my-feature"])
            .expect("create with branch should succeed");
        match cli.command {
            Some(Commands::Create { branch, from, .. }) => {
                assert_eq!(branch, "my-feature");
                assert!(from.is_none());
            }
            _ => panic!("expected Commands::Create"),
        }
    }

    #[test]
    fn create_subcommand_accepts_from_flag() {
        let cli = Cli::try_parse_from(["trench", "create", "my-feature", "--from", "develop"])
            .expect("create with --from should succeed");
        match cli.command {
            Some(Commands::Create { branch, from, .. }) => {
                assert_eq!(branch, "my-feature");
                assert_eq!(from.as_deref(), Some("develop"));
            }
            _ => panic!("expected Commands::Create"),
        }
    }

    #[test]
    fn no_subcommand_is_valid() {
        // No subcommand = TUI mode, so it should parse successfully
        let cli = Cli::try_parse_from(["trench"]).expect("no subcommand should be valid");
        assert!(cli.command.is_none());
    }

    #[test]
    fn help_flag_shows_usage() {
        let result = Cli::try_parse_from(["trench", "--help"]);
        let err = result.unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let output = err.to_string();
        // Help should mention key subcommands
        assert!(output.contains("create"), "help should mention 'create'");
        assert!(output.contains("list"), "help should mention 'list'");
        assert!(output.contains("sync"), "help should mention 'sync'");
    }

    #[test]
    fn global_flags_work_with_subcommands() {
        let cli = Cli::try_parse_from(["trench", "--json", "list"])
            .expect("global flags should work with subcommands");
        assert!(cli.json);
        assert!(cli.command.is_some());
    }

    #[test]
    fn should_launch_tui_when_interactive() {
        let cli = Cli::try_parse_from(["trench"]).unwrap();
        assert!(cli.should_launch_tui(true, true));
    }

    #[test]
    fn should_not_launch_tui_with_subcommand() {
        let cli = Cli::try_parse_from(["trench", "list"]).unwrap();
        assert!(!cli.should_launch_tui(true, true));
    }

    #[test]
    fn should_not_launch_tui_when_stdin_not_tty() {
        let cli = Cli::try_parse_from(["trench"]).unwrap();
        assert!(!cli.should_launch_tui(false, true));
    }

    #[test]
    fn should_not_launch_tui_when_stdout_not_tty() {
        let cli = Cli::try_parse_from(["trench"]).unwrap();
        assert!(!cli.should_launch_tui(true, false));
    }

    #[test]
    fn dry_run_flag_works_with_create_subcommand() {
        let cli = Cli::try_parse_from(["trench", "--dry-run", "create", "my-feature"])
            .expect("--dry-run with create should parse");
        assert!(cli.dry_run);
        assert!(matches!(
            cli.command,
            Some(Commands::Create { ref branch, .. }) if branch == "my-feature"
        ));
    }

    #[test]
    fn dry_run_and_json_flags_work_together_with_create() {
        let cli = Cli::try_parse_from(["trench", "--dry-run", "--json", "create", "my-feature"])
            .expect("--dry-run --json with create should parse");
        assert!(cli.dry_run);
        assert!(cli.json);
    }

    #[test]
    fn create_subcommand_accepts_no_hooks_flag() {
        let cli = Cli::try_parse_from(["trench", "create", "my-feature", "--no-hooks"])
            .expect("create with --no-hooks should succeed");
        match cli.command {
            Some(Commands::Create {
                branch, no_hooks, ..
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(no_hooks);
            }
            _ => panic!("expected Commands::Create"),
        }
    }

    #[test]
    fn create_subcommand_no_hooks_defaults_to_false() {
        let cli = Cli::try_parse_from(["trench", "create", "my-feature"])
            .expect("create without --no-hooks should succeed");
        match cli.command {
            Some(Commands::Create { no_hooks, .. }) => {
                assert!(!no_hooks, "no_hooks should default to false");
            }
            _ => panic!("expected Commands::Create"),
        }
    }

    #[test]
    fn remove_subcommand_requires_branch() {
        let result = Cli::try_parse_from(["trench", "remove"]);
        assert!(result.is_err(), "remove without branch should fail");
    }

    #[test]
    fn remove_subcommand_accepts_branch() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature"])
            .expect("remove with branch should succeed");
        match cli.command {
            Some(Commands::Remove {
                branch,
                force,
                prune,
                no_hooks,
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(!force);
                assert!(!prune);
                assert!(!no_hooks);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn remove_subcommand_accepts_force_flag() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature", "--force"])
            .expect("remove with --force should succeed");
        match cli.command {
            Some(Commands::Remove {
                branch,
                force,
                prune,
                no_hooks,
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(force);
                assert!(!prune);
                assert!(!no_hooks);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn switch_subcommand_requires_branch() {
        let result = Cli::try_parse_from(["trench", "switch"]);
        assert!(result.is_err(), "switch without branch should fail");
    }

    #[test]
    fn switch_subcommand_accepts_branch() {
        let cli = Cli::try_parse_from(["trench", "switch", "my-feature"])
            .expect("switch with branch should succeed");
        match cli.command {
            Some(Commands::Switch { branch, print_path }) => {
                assert_eq!(branch, "my-feature");
                assert!(!print_path);
            }
            _ => panic!("expected Commands::Switch"),
        }
    }

    #[test]
    fn switch_subcommand_accepts_print_path_flag() {
        let cli = Cli::try_parse_from(["trench", "switch", "my-feature", "--print-path"])
            .expect("switch with --print-path should succeed");
        match cli.command {
            Some(Commands::Switch { branch, print_path }) => {
                assert_eq!(branch, "my-feature");
                assert!(print_path);
            }
            _ => panic!("expected Commands::Switch"),
        }
    }

    #[test]
    fn tag_subcommand_requires_branch() {
        let result = Cli::try_parse_from(["trench", "tag"]);
        assert!(result.is_err(), "tag without branch should fail");
    }

    #[test]
    fn tag_subcommand_accepts_branch_only() {
        let cli = Cli::try_parse_from(["trench", "tag", "my-feature"])
            .expect("tag with branch should succeed");
        match cli.command {
            Some(Commands::Tag { branch, tags }) => {
                assert_eq!(branch, "my-feature");
                assert!(tags.is_empty());
            }
            _ => panic!("expected Commands::Tag"),
        }
    }

    #[test]
    fn tag_subcommand_accepts_add_and_remove_args() {
        let cli = Cli::try_parse_from(["trench", "tag", "my-feature", "+wip", "-old", "+review"])
            .expect("tag with +/- args should succeed");
        match cli.command {
            Some(Commands::Tag { branch, tags }) => {
                assert_eq!(branch, "my-feature");
                assert_eq!(tags, vec!["+wip", "-old", "+review"]);
            }
            _ => panic!("expected Commands::Tag"),
        }
    }

    #[test]
    fn list_subcommand_accepts_tag_filter() {
        let cli = Cli::try_parse_from(["trench", "list", "--tag", "wip"])
            .expect("list with --tag should succeed");
        match cli.command {
            Some(Commands::List { tag }) => {
                assert_eq!(tag.as_deref(), Some("wip"));
            }
            _ => panic!("expected Commands::List"),
        }
    }

    #[test]
    fn init_subcommand_defaults_force_to_false() {
        let cli = Cli::try_parse_from(["trench", "init"]).expect("init should parse");
        match cli.command {
            Some(Commands::Init { force }) => {
                assert!(!force, "force should default to false");
            }
            _ => panic!("expected Commands::Init"),
        }
    }

    #[test]
    fn init_subcommand_accepts_force_flag() {
        let cli =
            Cli::try_parse_from(["trench", "init", "--force"]).expect("init --force should parse");
        match cli.command {
            Some(Commands::Init { force }) => {
                assert!(force, "force should be true");
            }
            _ => panic!("expected Commands::Init"),
        }
    }

    #[test]
    fn remove_subcommand_accepts_prune_flag() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature", "--prune"])
            .expect("remove with --prune should succeed");
        match cli.command {
            Some(Commands::Remove {
                branch,
                force,
                prune,
                no_hooks,
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(!force);
                assert!(prune);
                assert!(!no_hooks);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn remove_subcommand_accepts_force_and_prune_combined() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature", "--force", "--prune"])
            .expect("remove with --force --prune should succeed");
        match cli.command {
            Some(Commands::Remove {
                branch,
                force,
                prune,
                no_hooks,
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(force);
                assert!(prune);
                assert!(!no_hooks);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn remove_subcommand_prune_defaults_to_false() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature"])
            .expect("remove with branch should succeed");
        match cli.command {
            Some(Commands::Remove { prune, .. }) => {
                assert!(!prune);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn remove_subcommand_accepts_no_hooks_flag() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature", "--no-hooks"])
            .expect("remove with --no-hooks should succeed");
        match cli.command {
            Some(Commands::Remove { no_hooks, .. }) => {
                assert!(no_hooks);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn shell_init_help_explains_eval_installation() {
        let result = Cli::try_parse_from(["trench", "shell-init", "--help"]);
        let err = result.unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let output = err.to_string();
        assert!(
            output.contains("eval"),
            "shell-init help should explain eval installation, got:\n{output}"
        );
        assert!(
            output.contains("shell-init"),
            "shell-init help should mention the command name"
        );
    }

    #[test]
    fn shell_init_help_shows_shell_config_examples() {
        let result = Cli::try_parse_from(["trench", "shell-init", "--help"]);
        let err = result.unwrap_err();
        let output = err.to_string();
        assert!(
            output.contains(".bashrc") || output.contains(".zshrc"),
            "shell-init help should reference shell config files, got:\n{output}"
        );
    }

    #[test]
    fn shell_init_help_warns_about_posix_tr_shadowing() {
        let result = Cli::try_parse_from(["trench", "shell-init", "--help"]);
        let err = result.unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        let output = err.to_string();
        assert!(
            output.contains("shadow"),
            "shell-init help should warn about POSIX tr shadowing, got:\n{output}"
        );
        assert!(
            output.contains("command tr"),
            "shell-init help should explain how to access the POSIX tr utility, got:\n{output}"
        );
    }

    #[test]
    fn shell_init_subcommand_requires_shell_argument() {
        let result = Cli::try_parse_from(["trench", "shell-init"]);
        assert!(result.is_err(), "shell-init without shell should fail");
    }

    #[test]
    fn shell_init_subcommand_accepts_bash() {
        let cli = Cli::try_parse_from(["trench", "shell-init", "bash"])
            .expect("shell-init bash should succeed");
        assert!(matches!(cli.command, Some(Commands::ShellInit { .. })));
    }

    #[test]
    fn shell_init_subcommand_accepts_zsh() {
        let cli = Cli::try_parse_from(["trench", "shell-init", "zsh"])
            .expect("shell-init zsh should succeed");
        assert!(matches!(cli.command, Some(Commands::ShellInit { .. })));
    }

    #[test]
    fn shell_init_subcommand_accepts_fish() {
        let cli = Cli::try_parse_from(["trench", "shell-init", "fish"])
            .expect("shell-init fish should succeed");
        assert!(matches!(cli.command, Some(Commands::ShellInit { .. })));
    }

    #[test]
    fn shell_init_rejects_unknown_shell() {
        let result = Cli::try_parse_from(["trench", "shell-init", "powershell"]);
        assert!(result.is_err(), "shell-init should reject unknown shells");
    }

    #[test]
    fn completions_subcommand_requires_shell_argument() {
        let result = Cli::try_parse_from(["trench", "completions"]);
        assert!(result.is_err(), "completions without shell should fail");
    }

    #[test]
    fn completions_subcommand_accepts_bash() {
        let cli = Cli::try_parse_from(["trench", "completions", "bash"])
            .expect("completions bash should succeed");
        assert!(matches!(cli.command, Some(Commands::Completions { .. })));
    }

    #[test]
    fn completions_for_real_cli_contain_subcommands() {
        let mut buf = Vec::new();
        cli::commands::completions::generate::<Cli>(ShellType::Bash, &mut buf);
        let output = String::from_utf8(buf).expect("completions should be valid utf-8");
        assert!(
            output.contains("create"),
            "bash completions should include 'create' subcommand"
        );
        assert!(
            output.contains("switch"),
            "bash completions should include 'switch' subcommand"
        );
        assert!(
            output.contains("shell-init"),
            "bash completions should include 'shell-init' subcommand"
        );
        assert!(
            output.contains("completions"),
            "bash completions should include 'completions' subcommand"
        );
    }

    #[test]
    fn cli_produces_output_config() {
        let cli =
            Cli::try_parse_from(["trench", "--no-color", "--quiet"]).expect("flags should parse");
        let config = cli.output_config();
        assert!(!config.should_color());
        assert!(config.is_quiet());
        assert!(!config.is_verbose());
    }

    #[test]
    fn sync_subcommand_accepts_strategy_rebase() {
        let cli = Cli::try_parse_from(["trench", "sync", "foo", "--strategy", "rebase"])
            .expect("sync with --strategy rebase should parse");
        match cli.command {
            Some(Commands::Sync {
                branch, strategy, ..
            }) => {
                assert_eq!(branch, Some("foo".to_string()));
                assert_eq!(strategy, Some(SyncStrategy::Rebase));
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_subcommand_accepts_strategy_merge() {
        let cli = Cli::try_parse_from(["trench", "sync", "foo", "--strategy", "merge"])
            .expect("sync with --strategy merge should parse");
        match cli.command {
            Some(Commands::Sync {
                branch, strategy, ..
            }) => {
                assert_eq!(branch, Some("foo".to_string()));
                assert_eq!(strategy, Some(SyncStrategy::Merge));
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_subcommand_strategy_defaults_to_none() {
        let cli = Cli::try_parse_from(["trench", "sync", "foo"])
            .expect("sync without --strategy should parse");
        match cli.command {
            Some(Commands::Sync {
                branch, strategy, ..
            }) => {
                assert_eq!(branch, Some("foo".to_string()));
                assert!(strategy.is_none());
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_subcommand_rejects_invalid_strategy() {
        let result = Cli::try_parse_from(["trench", "sync", "foo", "--strategy", "squash"]);
        assert!(result.is_err(), "invalid strategy should be rejected");
    }

    #[test]
    fn sync_subcommand_accepts_no_hooks_flag() {
        let cli = Cli::try_parse_from([
            "trench",
            "sync",
            "foo",
            "--strategy",
            "rebase",
            "--no-hooks",
        ])
        .expect("sync with --no-hooks should parse");
        match cli.command {
            Some(Commands::Sync {
                branch, no_hooks, ..
            }) => {
                assert_eq!(branch, Some("foo".to_string()));
                assert!(no_hooks, "--no-hooks should be true");
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_subcommand_no_hooks_defaults_to_false() {
        let cli = Cli::try_parse_from(["trench", "sync", "foo"])
            .expect("sync without --no-hooks should parse");
        match cli.command {
            Some(Commands::Sync { no_hooks, .. }) => {
                assert!(!no_hooks, "--no-hooks should default to false");
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_all_flag_parses_with_strategy() {
        let cli = Cli::try_parse_from(["trench", "sync", "--all", "--strategy", "rebase"])
            .expect("sync --all --strategy rebase should parse");
        match cli.command {
            Some(Commands::Sync {
                branch,
                all,
                strategy,
                ..
            }) => {
                assert!(branch.is_none(), "branch should be None when --all is used");
                assert!(all, "--all should be true");
                assert_eq!(strategy, Some(SyncStrategy::Rebase));
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_all_flag_parses_without_branch() {
        let cli = Cli::try_parse_from(["trench", "sync", "--all", "--strategy", "merge"])
            .expect("sync --all --strategy merge should parse");
        match cli.command {
            Some(Commands::Sync {
                branch,
                all,
                strategy,
                ..
            }) => {
                assert!(branch.is_none());
                assert!(all);
                assert_eq!(strategy, Some(SyncStrategy::Merge));
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_branch_still_works_without_all() {
        let cli = Cli::try_parse_from(["trench", "sync", "my-feature", "--strategy", "rebase"])
            .expect("sync with branch should still parse");
        match cli.command {
            Some(Commands::Sync { branch, all, .. }) => {
                assert_eq!(branch, Some("my-feature".to_string()));
                assert!(!all, "--all should default to false");
            }
            _ => panic!("expected Commands::Sync"),
        }
    }

    #[test]
    fn sync_all_without_strategy_parses_but_strategy_is_none() {
        // CLI parsing succeeds — the exit-code-8 validation happens at runtime
        let cli = Cli::try_parse_from(["trench", "sync", "--all"])
            .expect("sync --all without --strategy should still parse");
        match cli.command {
            Some(Commands::Sync { all, strategy, .. }) => {
                assert!(all);
                assert!(strategy.is_none(), "--strategy should be None");
            }
            _ => panic!("expected Commands::Sync"),
        }
    }
}
