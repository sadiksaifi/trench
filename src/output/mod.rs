/// Output verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Only errors and explicitly requested data.
    Quiet,
    /// Default output level.
    Normal,
    /// Debug-level logging enabled.
    Verbose,
}

/// Resolved output configuration derived from CLI flags, environment variables,
/// and terminal detection. Intended to be constructed once at startup and passed
/// to all formatters and command handlers.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    color: bool,
    verbosity: Verbosity,
}

impl OutputConfig {
    pub fn from_env(no_color: bool, quiet: bool, verbose: bool, is_tty: bool) -> Self {
        let env_no_color = std::env::var_os("NO_COLOR").is_some();
        let color = !no_color && !env_no_color && is_tty;

        let verbosity = if quiet {
            Verbosity::Quiet
        } else if verbose {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        };

        Self { color, verbosity }
    }

    pub fn should_color(&self) -> bool {
        self.color
    }

    pub fn is_quiet(&self) -> bool {
        self.verbosity == Verbosity::Quiet
    }

    pub fn is_verbose(&self) -> bool {
        self.verbosity == Verbosity::Verbose
    }

    pub fn verbosity(&self) -> Verbosity {
        self.verbosity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_flag_disables_color() {
        let config = OutputConfig::from_env(
            /* no_color */ true,
            /* quiet */ false,
            /* verbose */ false,
            /* is_tty */ true,
        );
        assert!(!config.should_color());
    }

    #[test]
    fn no_color_env_var_disables_color() {
        // NO_COLOR convention: any value (even empty) disables color
        std::env::set_var("NO_COLOR", "1");
        let config = OutputConfig::from_env(
            /* no_color */ false,
            /* quiet */ false,
            /* verbose */ false,
            /* is_tty */ true,
        );
        std::env::remove_var("NO_COLOR");
        assert!(!config.should_color());
    }

    #[test]
    fn defaults_enable_color_when_tty() {
        std::env::remove_var("NO_COLOR");
        let config = OutputConfig::from_env(false, false, false, /* is_tty */ true);
        assert!(config.should_color());
    }

    #[test]
    fn non_tty_auto_disables_color() {
        std::env::remove_var("NO_COLOR");
        let config = OutputConfig::from_env(false, false, false, /* is_tty */ false);
        assert!(!config.should_color());
    }

    #[test]
    fn quiet_flag_suppresses_info() {
        let config = OutputConfig::from_env(false, /* quiet */ true, false, true);
        assert!(config.is_quiet());
        assert_eq!(config.verbosity(), Verbosity::Quiet);
    }
}
