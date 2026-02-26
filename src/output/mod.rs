/// Resolved output configuration derived from CLI flags, environment variables,
/// and terminal detection. Intended to be constructed once at startup and passed
/// to all formatters and command handlers.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    color: bool,
}

impl OutputConfig {
    pub fn from_env(no_color: bool, _quiet: bool, _verbose: bool, is_tty: bool) -> Self {
        let color = !no_color && is_tty;
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
}
