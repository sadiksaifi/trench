use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "trench", version, about = "A fast, ergonomic, headless-first Git worktree manager")]
struct Cli {}

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
}
