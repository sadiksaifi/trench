use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};

/// A single file that was copied during the copy step.
#[derive(Debug, Clone)]
pub struct CopiedFile {
    /// File name (relative path from source root).
    pub name: String,
    /// Absolute source path.
    pub source: PathBuf,
    /// Absolute destination path.
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

        if path.is_dir() {
            collect_matching_files(root, &path, dest_dir, includes, excludes, copied)?;
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .context("failed to compute relative path")?;
        let rel_str = relative.to_string_lossy();

        if includes.is_match(&*rel_str) && !excludes.is_match(&*rel_str) {
            let dest_path = dest_dir.join(relative);
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &dest_path)
                .with_context(|| format!("failed to copy {} → {}", path.display(), dest_path.display()))?;

            copied.push(CopiedFile {
                name: rel_str.into_owned(),
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
}
