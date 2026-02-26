use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "trench", version, about = "A fast, ergonomic, headless-first Git worktree manager")]
struct Cli {
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

fn main() {
    let _cli = Cli::parse();
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
}
