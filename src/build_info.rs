/// Version identity embedded into this executable at compile time.
pub const VERSION: &str = env!("TRENCH_BUILD_VERSION");

/// Full Git commit hash embedded into this executable, when available.
pub const COMMIT: Option<&str> = option_env!("TRENCH_BUILD_COMMIT");

/// Exact canonical annotated tag at the build commit, when present.
pub const EXACT_TAG: Option<&str> = option_env!("TRENCH_BUILD_EXACT_TAG");

/// Whether the Git checkout was dirty, when Git metadata was available.
pub const DIRTY: Option<bool> = match option_env!("TRENCH_BUILD_DIRTY") {
    Some(value) => Some(is_true(value)),
    _ => None,
};

/// Whether this executable was selected as an official release build.
pub const OFFICIAL: bool = match option_env!("TRENCH_BUILD_OFFICIAL") {
    Some(value) => is_true(value),
    None => false,
};

pub const fn is_development() -> bool {
    !OFFICIAL
}

const fn is_true(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 4 && bytes[0] == b't' && bytes[1] == b'r' && bytes[2] == b'u' && bytes[3] == b'e'
}
