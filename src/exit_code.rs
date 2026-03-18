/// Typed exit codes for the trench CLI (FR-37).
///
/// Every process exit must use one of these variants instead of raw integers.
/// This ensures consistent, documented exit codes across all commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// 0 — Success
    Success,
    /// 1 — General error
    GeneralError,
    /// 2 — Not found
    NotFound,
    /// 3 — Branch exists
    BranchExists,
    /// 4 — Hook failed
    HookFailed,
    /// 5 — Git error
    GitError,
    /// 6 — Config error
    ConfigError,
    /// 7 — Hook timeout
    HookTimeout,
    /// 8 — Missing required flag
    MissingRequiredFlag,
    /// 9 — Flag conflict
    FlagConflict,
}

impl ExitCode {
    /// Return the numeric exit code for this variant.
    pub fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::GeneralError => 1,
            Self::NotFound => 2,
            Self::BranchExists => 3,
            Self::HookFailed => 4,
            Self::GitError => 5,
            Self::ConfigError => 6,
            Self::HookTimeout => 7,
            Self::MissingRequiredFlag => 8,
            Self::FlagConflict => 9,
        }
    }

    /// Terminate the process with this exit code.
    pub fn exit(self) -> ! {
        std::process::exit(self.code())
    }
}

impl std::fmt::Display for ExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let desc = match self {
            Self::Success => "success",
            Self::GeneralError => "general error",
            Self::NotFound => "not found",
            Self::BranchExists => "branch exists",
            Self::HookFailed => "hook failed",
            Self::GitError => "git error",
            Self::ConfigError => "config error",
            Self::HookTimeout => "hook timeout",
            Self::MissingRequiredFlag => "missing required flag",
            Self::FlagConflict => "flag conflict",
        };
        write!(f, "{} ({desc})", self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_variants_map_to_documented_exit_codes() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::GeneralError.code(), 1);
        assert_eq!(ExitCode::NotFound.code(), 2);
        assert_eq!(ExitCode::BranchExists.code(), 3);
        assert_eq!(ExitCode::HookFailed.code(), 4);
        assert_eq!(ExitCode::GitError.code(), 5);
        assert_eq!(ExitCode::ConfigError.code(), 6);
        assert_eq!(ExitCode::HookTimeout.code(), 7);
        assert_eq!(ExitCode::MissingRequiredFlag.code(), 8);
        assert_eq!(ExitCode::FlagConflict.code(), 9);
    }

    #[test]
    fn display_includes_code_and_description() {
        assert_eq!(format!("{}", ExitCode::Success), "0 (success)");
        assert_eq!(format!("{}", ExitCode::GeneralError), "1 (general error)");
        assert_eq!(format!("{}", ExitCode::NotFound), "2 (not found)");
        assert_eq!(format!("{}", ExitCode::BranchExists), "3 (branch exists)");
        assert_eq!(format!("{}", ExitCode::HookFailed), "4 (hook failed)");
        assert_eq!(format!("{}", ExitCode::GitError), "5 (git error)");
        assert_eq!(format!("{}", ExitCode::ConfigError), "6 (config error)");
        assert_eq!(format!("{}", ExitCode::HookTimeout), "7 (hook timeout)");
        assert_eq!(
            format!("{}", ExitCode::MissingRequiredFlag),
            "8 (missing required flag)"
        );
        assert_eq!(
            format!("{}", ExitCode::FlagConflict),
            "9 (flag conflict)"
        );
    }

    #[test]
    fn flag_conflict_variant_exists() {
        assert_eq!(ExitCode::FlagConflict.code(), 9);
        assert_eq!(format!("{}", ExitCode::FlagConflict), "9 (flag conflict)");
    }

    #[test]
    fn enum_has_exactly_ten_variants() {
        // Verify all 10 codes are distinct
        let codes: Vec<i32> = vec![
            ExitCode::Success.code(),
            ExitCode::GeneralError.code(),
            ExitCode::NotFound.code(),
            ExitCode::BranchExists.code(),
            ExitCode::HookFailed.code(),
            ExitCode::GitError.code(),
            ExitCode::ConfigError.code(),
            ExitCode::HookTimeout.code(),
            ExitCode::MissingRequiredFlag.code(),
            ExitCode::FlagConflict.code(),
        ];
        let mut unique = codes.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 10);
        assert_eq!(unique, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }
}
