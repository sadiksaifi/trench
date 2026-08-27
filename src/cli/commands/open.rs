use std::{path::Path, process::ExitStatus};

use anyhow::Context;

#[derive(Debug, thiserror::Error)]
#[error("editor exited with status {status}")]
pub struct EditorExitError {
    pub status: ExitStatus,
}

pub fn plan(
    identifier: &str,
    cwd: &Path,
    configured_editor: Option<&str>,
) -> anyhow::Result<crate::navigation::OpenPlan> {
    crate::navigation::OpenPlan::resolve(cwd, identifier, configured_editor)
}

pub fn execute(
    identifier: &str,
    cwd: &Path,
    configured_editor: Option<&str>,
) -> anyhow::Result<()> {
    let plan = plan(identifier, cwd, configured_editor)?;
    let status = plan.editor.status(&plan.path).with_context(|| {
        format!(
            "failed to launch editor {}",
            plan.editor.program().to_string_lossy()
        )
    })?;
    if !status.success() {
        return Err(EditorExitError { status }.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

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

    fn executable(path: &Path, script: &str) {
        fs::write(path, script).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn open_preserves_editor_arguments_and_appends_one_path_argument() {
        let root = tempfile::tempdir().unwrap();
        let repository = root.path().join("work tree");
        fs::create_dir(&repository).unwrap();
        init_repo(&repository);
        let editor = root.path().join("fake editor");
        let argv = root.path().join("argv");
        executable(
            &editor,
            &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", argv.display()),
        );
        let command = format!(
            "'{}' --profile 'Work Trees' --literal=\"two words\"",
            editor.display()
        );

        execute("work tree", &repository, Some(&command)).unwrap();

        assert_eq!(
            fs::read_to_string(argv).unwrap(),
            format!(
                "--profile\nWork Trees\n--literal=two words\n{}\n",
                repository.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn open_waits_and_reports_the_editor_exit_status() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let editor = root.path().join("failing-editor");
        executable(&editor, "#!/bin/sh\nexit 17\n");

        let error = execute(
            root.path().file_name().unwrap().to_str().unwrap(),
            root.path(),
            Some(editor.to_str().unwrap()),
        )
        .unwrap_err();

        let exit = error.downcast_ref::<EditorExitError>().unwrap();
        assert_eq!(exit.status.code(), Some(17));
    }
}
