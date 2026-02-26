/// Resolved output configuration derived from CLI flags, environment variables,
/// and terminal detection. Intended to be constructed once at startup and passed
/// to all formatters and command handlers.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    color: bool,
}

impl OutputConfig {
    pub fn from_env(no_color: bool, _quiet: bool, _verbose: bool, is_tty: bool) -> Self {
        let env_no_color = std::env::var_os("NO_COLOR").is_some();
        let color = !no_color && !env_no_color && is_tty;
        Self { color }
    }

    pub fn should_color(&self) -> bool {
        self.color
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
}
