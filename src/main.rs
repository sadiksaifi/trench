mod cli;
mod config;
mod git;
mod hooks;
mod output;
mod paths;
mod state;
mod tui;

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::IsTerminal;

use output::OutputConfig;

#[derive(Parser, Debug)]
#[command(name = "trench", version, about = "A fast, ergonomic, headless-first Git worktree manager")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Output as JSON
    #[arg(long, global = true)]
    json: bool,

    /// Output in porcelain format
    #[arg(long, global = true)]
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
    },
    /// Switch to a worktree
    Switch {
        /// Branch name or sanitized name of the worktree
        branch: String,

        /// Print only the worktree path (for shell integration)
        #[arg(long)]
        print_path: bool,
    },
    /// Open a worktree in $EDITOR
    Open,
    /// List all worktrees
    List,
    /// Show worktree status
    Status,
    /// Sync worktree with base branch
    Sync,
    /// View event log
    Log,
    /// Initialize .trench.toml in current directory
    Init,
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
    let _output_config = cli.output_config();

    if cli.should_launch_tui(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        return tui::run();
    }

    let dry_run = cli.dry_run;
    let json = cli.json;

    match cli.command {
        Some(Commands::Create { branch, from }) => {
            run_create(&branch, from.as_deref(), dry_run, json)
        }
        Some(Commands::Remove { branch, force }) => run_remove(&branch, force),
        Some(Commands::Switch { branch, print_path }) => run_switch(&branch, print_path),
        Some(Commands::List) => run_list(),
        Some(_) => {
            // Other commands not yet implemented
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
        Ok(path) => {
            println!("{}", path.display());
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

fn run_remove(identifier: &str, force: bool) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    // If not forced, look up the worktree to show details in the confirmation prompt
    if !force {
        let repo_info = git::discover_repo(&cwd)?;
        let repo_path_str = repo_info.path.to_str().unwrap_or("");
        if let Some(repo) = db.get_repo_by_path(repo_path_str)? {
            if let Some(wt) = db.find_worktree_by_identifier(repo.id, identifier)? {
                eprint!(
                    "Remove worktree '{}' at {}? [y/N] ",
                    wt.name, wt.path
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
    }

    match cli::commands::remove::execute(identifier, &cwd, &db) {
        Ok(name) => {
            eprintln!("Removed worktree '{name}'");
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

fn run_list() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    let output = cli::commands::list::execute(&cwd, &db)?;
    if output.ends_with('\n') {
        print!("{output}");
    } else {
        println!("{output}");
    }
    Ok(())
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
            "trench", "--json", "--porcelain", "--no-color", "--quiet", "--verbose", "--dry-run",
        ])
        .expect("all global flags should be accepted");

        assert!(cli.json);
        assert!(cli.porcelain);
        assert!(cli.no_color);
        assert!(cli.quiet);
        assert!(cli.verbose);
        assert!(cli.dry_run);
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
        // switch and remove require a branch argument, so test them separately
        let subcommands = [
            "open", "list", "status", "sync", "log", "init",
        ];
        for sub in subcommands {
            let result = Cli::try_parse_from(["trench", sub]);
            assert!(
                result.is_ok(),
                "subcommand '{}' should be accepted, got: {:?}",
                sub,
                result.unwrap_err()
            );
        }
        // remove and switch need a branch arg
        let result = Cli::try_parse_from(["trench", "remove", "my-feature"]);
        assert!(result.is_ok(), "remove with branch should be accepted");
        let result = Cli::try_parse_from(["trench", "switch", "my-feature"]);
        assert!(result.is_ok(), "switch with branch should be accepted");
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
        let cli =
            Cli::try_parse_from(["trench", "--dry-run", "--json", "create", "my-feature"])
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
            Some(Commands::Remove { branch, force }) => {
                assert_eq!(branch, "my-feature");
                assert!(!force);
            }
            _ => panic!("expected Commands::Remove"),
        }
    }

    #[test]
    fn remove_subcommand_accepts_force_flag() {
        let cli = Cli::try_parse_from(["trench", "remove", "my-feature", "--force"])
            .expect("remove with --force should succeed");
        match cli.command {
            Some(Commands::Remove { branch, force }) => {
                assert_eq!(branch, "my-feature");
                assert!(force);
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
    fn cli_produces_output_config() {
        let cli = Cli::try_parse_from(["trench", "--no-color", "--quiet"])
            .expect("flags should parse");
        let config = cli.output_config();
        assert!(!config.should_color());
        assert!(config.is_quiet());
        assert!(!config.is_verbose());
    }
}
