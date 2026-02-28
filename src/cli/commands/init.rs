use std::path::{Path, PathBuf};

use anyhow::Result;

const TRENCH_TOML_FILENAME: &str = ".trench.toml";

/// The scaffold content for `.trench.toml`.
const SCAFFOLD: &str = "# trench — project configuration\n";

/// Execute `trench init` — scaffold a commented `.trench.toml` at the repo root.
pub fn execute(repo_root: &Path, _force: bool) -> Result<PathBuf> {
    let path = repo_root.join(TRENCH_TOML_FILENAME);
    std::fs::write(&path, SCAFFOLD)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_trench_toml_at_repo_root() {
        let dir = TempDir::new().unwrap();

        let result = execute(dir.path(), false);

        assert!(result.is_ok(), "init should succeed: {:?}", result.err());
        let created_path = result.unwrap();
        assert_eq!(created_path, dir.path().join(".trench.toml"));
        assert!(created_path.exists(), ".trench.toml should exist on disk");

        let contents = std::fs::read_to_string(&created_path).unwrap();
        assert!(!contents.is_empty(), "file should not be empty");
    }
}
