use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};

/// A single file that was copied during the copy step.
#[derive(Debug, Clone)]
pub struct CopiedFile {
    /// File name (relative path from source root).
    pub name: String,
    /// Source path (absolute if `source_dir` is absolute).
    pub source: PathBuf,
    /// Destination path (absolute if `dest_dir` is absolute).
    pub destination: PathBuf,
}

/// Result of executing the copy step.
#[derive(Debug, Clone)]
pub struct CopyResult {
    /// Files that were copied.
    pub copied: Vec<CopiedFile>,
}

/// Execute the copy step of a hook: resolve glob patterns against `source_dir`
/// and copy matching files to `dest_dir`.
///
/// Patterns prefixed with `!` are exclusion patterns.
/// Execution order: includes are matched first, then excludes filter them out.
/// Per FR-21.
pub fn execute_copy_step(
    source_dir: &Path,
    dest_dir: &Path,
    patterns: &[String],
) -> Result<CopyResult> {
    let mut include_builder = GlobSetBuilder::new();
    let mut exclude_builder = GlobSetBuilder::new();

    for pattern in patterns {
        if let Some(stripped) = pattern.strip_prefix('!') {
            let glob = Glob::new(stripped)
                .with_context(|| format!("invalid exclusion glob: {stripped}"))?;
            exclude_builder.add(glob);
        } else {
            let glob =
                Glob::new(pattern).with_context(|| format!("invalid glob: {pattern}"))?;
            include_builder.add(glob);
        }
    }

    let includes = include_builder.build().context("failed to build include glob set")?;
    let excludes = exclude_builder.build().context("failed to build exclude glob set")?;

    let mut copied = Vec::new();

    collect_matching_files(source_dir, source_dir, dest_dir, &includes, &excludes, &mut copied)?;

    Ok(CopyResult { copied })
}

fn collect_matching_files(
    root: &Path,
    current: &Path,
    dest_dir: &Path,
    includes: &globset::GlobSet,
    excludes: &globset::GlobSet,
    copied: &mut Vec<CopiedFile>,
) -> Result<()> {
    let entries = std::fs::read_dir(current)
        .with_context(|| format!("failed to read directory: {}", current.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type: {}", path.display()))?;

        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            collect_matching_files(root, &path, dest_dir, includes, excludes, copied)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .context("failed to compute relative path")?;

        if includes.is_match(relative) && !excludes.is_match(relative) {
            let dest_path = dest_dir.join(relative);
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &dest_path)
                .with_context(|| format!("failed to copy {} → {}", path.display(), dest_path.display()))?;

            copied.push(CopiedFile {
                name: relative.to_string_lossy().into_owned(),
                source: path,
                destination: dest_path,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn copies_files_matching_glob_pattern() {
        // Setup: source dir with .env, .env.local, and unrelated file
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join(".env"), "SECRET=abc").unwrap();
        std::fs::write(source.path().join(".env.local"), "LOCAL=xyz").unwrap();
        std::fs::write(source.path().join("README.md"), "# Hello").unwrap();

        let dest = TempDir::new().unwrap();

        let patterns = vec![".env*".to_string()];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        // Both .env files copied, README not copied
        assert_eq!(result.copied.len(), 2);
        assert!(dest.path().join(".env").exists());
        assert!(dest.path().join(".env.local").exists());
        assert!(!dest.path().join("README.md").exists());

        // Content preserved
        assert_eq!(
            std::fs::read_to_string(dest.path().join(".env")).unwrap(),
            "SECRET=abc"
        );
        assert_eq!(
            std::fs::read_to_string(dest.path().join(".env.local")).unwrap(),
            "LOCAL=xyz"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let source = TempDir::new().unwrap();
        let script_path = source.path().join("setup.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho hello").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let dest = TempDir::new().unwrap();

        let patterns = vec!["setup.sh".to_string()];
        execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        let dest_perms = std::fs::metadata(dest.path().join("setup.sh"))
            .unwrap()
            .permissions()
            .mode();

        // Verify the executable bit is preserved (at least owner execute)
        assert_ne!(dest_perms & 0o100, 0, "executable permission should be preserved");
    }

    #[test]
    fn result_contains_source_and_destination_for_each_copied_file() {
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join(".env"), "SECRET=abc").unwrap();

        let dest = TempDir::new().unwrap();

        let patterns = vec![".env".to_string()];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        assert_eq!(result.copied.len(), 1);
        let entry = &result.copied[0];
        assert_eq!(entry.name, ".env");
        assert_eq!(entry.source, source.path().join(".env"));
        assert_eq!(entry.destination, dest.path().join(".env"));
    }

    #[test]
    fn no_op_when_no_files_match() {
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join("README.md"), "# Hello").unwrap();

        let dest = TempDir::new().unwrap();

        let patterns = vec![".env*".to_string()];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        assert!(result.copied.is_empty());
    }

    #[test]
    fn exclusion_pattern_filters_out_matches() {
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join(".env"), "SECRET=abc").unwrap();
        std::fs::write(source.path().join(".env.local"), "LOCAL=xyz").unwrap();
        std::fs::write(source.path().join(".env.example"), "EXAMPLE=template").unwrap();

        let dest = TempDir::new().unwrap();

        let patterns = vec![".env*".to_string(), "!.env.example".to_string()];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        assert_eq!(result.copied.len(), 2);
        assert!(dest.path().join(".env").exists());
        assert!(dest.path().join(".env.local").exists());
        assert!(!dest.path().join(".env.example").exists());
    }

    #[test]
    fn multiple_include_patterns_match_different_file_types() {
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join("config.json"), "{}").unwrap();
        std::fs::write(source.path().join("settings.toml"), "[ui]").unwrap();
        std::fs::write(source.path().join("main.rs"), "fn main(){}").unwrap();

        let dest = TempDir::new().unwrap();

        let patterns = vec!["*.json".to_string(), "*.toml".to_string()];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        assert_eq!(result.copied.len(), 2);
        assert!(dest.path().join("config.json").exists());
        assert!(dest.path().join("settings.toml").exists());
        assert!(!dest.path().join("main.rs").exists());
    }

    #[test]
    fn empty_patterns_copies_nothing() {
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join(".env"), "SECRET=abc").unwrap();

        let dest = TempDir::new().unwrap();

        let patterns: Vec<String> = vec![];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        assert!(result.copied.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinked_directories() {
        // Create an external directory with a file that should NOT be copied
        let external = TempDir::new().unwrap();
        std::fs::write(external.path().join("secret.env"), "LEAKED=true").unwrap();

        // Create source directory with a real file and a symlink to the external dir
        let source = TempDir::new().unwrap();
        std::fs::write(source.path().join(".env"), "OK=true").unwrap();
        std::os::unix::fs::symlink(external.path(), source.path().join("linked")).unwrap();

        let dest = TempDir::new().unwrap();

        // Match everything
        let patterns = vec!["**/*".to_string()];
        let result = execute_copy_step(source.path(), dest.path(), &patterns).unwrap();

        // Only the real .env should be copied; the symlinked dir must be skipped
        assert_eq!(result.copied.len(), 1);
        assert_eq!(result.copied[0].name, ".env");
        assert!(!dest.path().join("linked").exists());
        assert!(!dest.path().join("linked/secret.env").exists());
    }
}
