use std::path::Path;

use anyhow::Result;

use crate::create_plan::{CreatePlan, CreatePlanner, HookPolicy};

pub fn execute_dry_run(
    branch: &str,
    from: Option<&str>,
    cwd: &Path,
    worktree_root: &Path,
    configured_base: Option<&str>,
    no_hooks: bool,
) -> Result<CreatePlan> {
    let hook_policy = if no_hooks {
        HookPolicy::Skip
    } else {
        HookPolicy::Run
    };
    CreatePlanner::discover(cwd, worktree_root, configured_base, hook_policy)?
        .plan(branch, from)
        .map_err(Into::into)
}
