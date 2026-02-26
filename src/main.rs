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

        /// Base branch to create from (defaults to repo's HEAD branch)
        #[arg(long)]
        from: Option<String>,
    },
    /// Remove a worktree
    Remove,
    /// Switch to a worktree
    Switch,
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

    match cli.command {
        Some(Commands::Create { branch, from }) => {
            run_create(&branch, from.as_deref())
        }
        Some(_) => {
            // Other commands not yet implemented
            Ok(())
        }
        None => {
            anyhow::bail!("TUI requires an interactive terminal (stdin and stdout must be a TTY)");
        }
    }
}

fn run_create(branch: &str, from: Option<&str>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let worktree_root = paths::worktree_root()?;
    let db_path = paths::data_dir()?.join("trench.db");
    let db = state::Database::open(&db_path)?;

    match cli::commands::create::execute(
        branch,
        from,
        &cwd,
        &worktree_root,
        paths::DEFAULT_WORKTREE_TEMPLATE,
        &db,
    ) {
        Ok(path) => {
            println!("{}", path.display());
            Ok(())
        }
        Err(e) => {
            if e.downcast_ref::<git::GitError>().is_some_and(|g| {
                matches!(g, git::GitError::BranchAlreadyExists { .. })
            }) {
                eprintln!("error: {e}");
                std::process::exit(3);
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
        let subcommands = [
            "remove", "switch", "open", "list", "status", "sync", "log", "init",
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
    fn cli_produces_output_config() {
        let cli = Cli::try_parse_from(["trench", "--no-color", "--quiet"])
            .expect("flags should parse");
        let config = cli.output_config();
        assert!(!config.should_color());
        assert!(config.is_quiet());
        assert!(!config.is_verbose());
    }
}
