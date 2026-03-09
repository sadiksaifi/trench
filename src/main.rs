mod adopt;
mod cli;
mod config;
mod git;
mod hooks;
mod output;
mod paths;
mod state;
mod tui;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;

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
    /// Sync worktree with base branch
    Sync {
        /// Branch name or sanitized name of the worktree to sync
        branch: String,
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

    match cli.command {
        Some(Commands::Create { branch, from }) => {
            run_create(&branch, from.as_deref(), dry_run, json)
        }
        Some(Commands::Remove {
            branch,
            force,
            prune,
        }) => run_remove(&branch, force, prune),
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
        Some(Commands::Sync { branch }) => run_sync(&branch),
        Some(Commands::Log) => {
            // Log command not yet implemented
            Ok(())
        }
        None => {
            anyhow::bail!("TUI requires an interactive terminal (stdin and stdout must be a TTY)");
        }
    }
}

fn run_create(branch: &str, from: Option<&str>, dry_run: bool, json: bool) -> anyhow::Result<()> {
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

    match cli::commands::create::execute(
        branch,
        from,
        &cwd,
        &worktree_root,
        &resolved.worktrees.root,
        &db,
    ) {
        Ok(result) => {
            if json {
                let hooks_status = cli::commands::create::HooksStatus::None;
                let json_output = result.to_json_output(hooks_status);
                println!("{}", output::json::format_json_value(&json_output)?);
            } else {
                println!("{}", result.path.display());
            }
            Ok(())
        }
        Err(e) => {
            if let Some(git_err) = e.downcast_ref::<git::GitError>() {
                match git_err {
                    git::GitError::BranchAlreadyExists { .. }
                    | git::GitError::RemoteBranchAlreadyExists { .. } => {
                        eprintln!("error: {e}");
                        std::process::exit(3);
                    }
                    git::GitError::BaseBranchNotFound { .. } => {
                        eprintln!("error: {e}");
                        std::process::exit(2);
                    }
                    _ => {}
                }
            }
            Err(e)
        }
    }
}

fn run_remove(identifier: &str, force: bool, prune: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    // If not forced, resolve the worktree (adopting if unmanaged) for the prompt
    if !force {
        let repo_info = git::discover_repo(&cwd)?;
        if let Ok((_repo, wt)) = adopt::resolve_or_adopt(identifier, &repo_info, &db) {
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
            if std::io::stdin().read_line(&mut input).is_err() {
                eprintln!("error: failed to read input");
                return Ok(());
            }
            if !input.trim().eq_ignore_ascii_case("y") {
                eprintln!("Cancelled.");
                return Ok(());
            }
        }
    }

    match cli::commands::remove::execute(identifier, &cwd, &db, prune) {
        Ok(result) => {
            if result.pruned_remote {
                eprintln!("Removed worktree '{}' and remote branch", result.name);
            } else {
                eprintln!("Removed worktree '{}'", result.name);
            }
            Ok(())
        }
        Err(e) => {
            if let Some(git_err) = e.downcast_ref::<git::GitError>() {
                if matches!(git_err, git::GitError::WorktreeNotFound { .. }) {
                    eprintln!("error: {e}");
                    std::process::exit(2);
                }
            }
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                std::process::exit(2);
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
                std::process::exit(2);
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
                std::process::exit(status.code().unwrap_or(1));
            }

            // Record DB side-effects only after a successful launch
            cli::commands::open::record_open(&db, result.repo_id, result.wt_id)?;

            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                std::process::exit(2);
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

    let output = if json {
        cli::commands::list::execute_json(&cwd, &db, tag)?
    } else if porcelain {
        cli::commands::list::execute_porcelain(&cwd, &db, tag)?
    } else {
        cli::commands::list::execute(&cwd, &db, tag)?
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
                std::process::exit(2);
            }
            Err(e)
        }
    }
}

fn run_sync(identifier: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    match cli::commands::sync::execute(identifier, &cwd, &db) {
        Ok(result) => {
            eprintln!(
                "Resolved worktree '{}' (sync not yet implemented)",
                result.name
            );
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not found") || msg.contains("not tracked") {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
            Err(e)
        }
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
                std::process::exit(6);
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
            Some(Commands::Create { branch, from }) => {
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
            Some(Commands::Create { branch, from }) => {
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
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(!force);
                assert!(!prune);
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
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(force);
                assert!(!prune);
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
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(!force);
                assert!(prune);
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
            }) => {
                assert_eq!(branch, "my-feature");
                assert!(force);
                assert!(prune);
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
}
