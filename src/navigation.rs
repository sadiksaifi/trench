use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use crate::worktree_catalog::{CatalogError, WorktreeCatalog};

#[derive(Debug, thiserror::Error)]
pub enum EditorCommandError {
    #[error(
        "no editor configured; set EDITOR or VISUAL, or add [editor] command = \"...\" to your config"
    )]
    Missing,
    #[error("invalid editor command {command:?}: {source}")]
    Invalid {
        command: String,
        #[source]
        source: shell_words::ParseError,
    },
    #[error("editor command is empty")]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    program: OsString,
    args: Vec<OsString>,
}

impl EditorCommand {
    pub fn from_environment(configured: Option<&str>) -> Result<Self, EditorCommandError> {
        let editor = std::env::var("EDITOR").ok();
        let visual = std::env::var("VISUAL").ok();
        Self::from_sources(configured, editor.as_deref(), visual.as_deref())
    }

    fn from_sources(
        configured: Option<&str>,
        editor: Option<&str>,
        visual: Option<&str>,
    ) -> Result<Self, EditorCommandError> {
        let source = [configured, editor, visual]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|candidate| !candidate.is_empty())
            .ok_or(EditorCommandError::Missing)?;
        let words =
            shell_words::split(source).map_err(|source_error| EditorCommandError::Invalid {
                command: source.to_string(),
                source: source_error,
            })?;
        let (program, args) = words.split_first().ok_or(EditorCommandError::Empty)?;
        Ok(Self {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
        })
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn status(&self, path: &Path) -> std::io::Result<ExitStatus> {
        Command::new(&self.program)
            .args(&self.args)
            .arg(path)
            .status()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPlan {
    pub path: PathBuf,
    pub editor: EditorCommand,
}

impl OpenPlan {
    pub fn resolve(
        cwd: &Path,
        selector: &str,
        configured_editor: Option<&str>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            path: resolve_path(cwd, selector)?,
            editor: EditorCommand::from_environment(configured_editor)?,
        })
    }
}

pub fn resolve_path(cwd: &Path, selector: &str) -> Result<PathBuf, CatalogError> {
    Ok(WorktreeCatalog::discover(cwd)?
        .resolve(selector)?
        .path
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(path: &Path) -> git2::Repository {
        let repo = git2::Repository::init(path).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[])
                .unwrap();
        }
        repo
    }

    fn add_worktree(repo: &git2::Repository, branch: &str, path: &Path) {
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let local = repo.branch(branch, &commit, false).unwrap();
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(local.get()));
        repo.worktree("feature-auth", path, Some(&options)).unwrap();
    }

    #[test]
    fn path_resolution_uses_the_live_catalog() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("feature-auth");
        std::fs::create_dir(&main).unwrap();
        let repo = init_repo(&main);
        add_worktree(&repo, "feature/auth", &linked);

        assert_eq!(
            resolve_path(&main, "feature/auth").unwrap(),
            linked.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_path(&main, "feature-auth").unwrap(),
            linked.canonicalize().unwrap()
        );
    }

    #[test]
    fn editor_command_uses_config_then_editor_then_visual() {
        let configured =
            EditorCommand::from_sources(Some("code --reuse-window"), Some("vim"), Some("nvim"))
                .unwrap();
        assert_eq!(configured.program(), OsStr::new("code"));
        assert_eq!(configured.args(), ["--reuse-window"]);

        let editor = EditorCommand::from_sources(None, Some("vim -f"), Some("nvim")).unwrap();
        assert_eq!(editor.program(), OsStr::new("vim"));
        assert_eq!(editor.args(), ["-f"]);

        let visual = EditorCommand::from_sources(None, Some("  "), Some("nvim")).unwrap();
        assert_eq!(visual.program(), OsStr::new("nvim"));
        assert!(visual.args().is_empty());
    }

    #[test]
    fn editor_command_preserves_shell_quoted_arguments_without_a_shell() {
        let command = EditorCommand::from_sources(
            Some("code --profile 'Work Trees' --literal=\"two words\""),
            None,
            None,
        )
        .unwrap();

        assert_eq!(command.program(), OsStr::new("code"));
        assert_eq!(
            command.args(),
            ["--profile", "Work Trees", "--literal=two words"]
        );
    }

    #[test]
    fn missing_or_malformed_editor_command_is_an_error() {
        assert!(matches!(
            EditorCommand::from_sources(None, Some(" "), None),
            Err(EditorCommandError::Missing)
        ));
        assert!(matches!(
            EditorCommand::from_sources(Some("code 'unterminated"), None, None),
            Err(EditorCommandError::Invalid { .. })
        ));
    }
}
