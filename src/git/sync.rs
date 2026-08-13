use std::path::PathBuf;

use git2::{Oid, Repository};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Rebase,
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPlan {
    pub worktree_path: PathBuf,
    pub branch_ref: String,
    pub expected_head: Oid,
    pub base_oid: Oid,
    pub strategy: Strategy,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncGitError {
    #[error("sync preconditions changed before mutation")]
    PreconditionsChanged,
    #[error("sync conflict")]
    Conflict,
    #[error("rollback failed: {0}")]
    Rollback(String),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

pub fn execute(plan: &TransactionPlan) -> Result<Oid, SyncGitError> {
    let repo = Repository::open(&plan.worktree_path)?;
    validate_clean_attached(&repo, &plan.branch_ref, plan.expected_head)?;
    if repo
        .graph_ahead_behind(plan.expected_head, plan.base_oid)?
        .1
        == 0
    {
        return Ok(plan.expected_head);
    }
    let result = match plan.strategy {
        Strategy::Rebase => rebase_in_memory(&repo, plan),
        Strategy::Merge => merge_in_memory(&repo, plan),
    };
    let new_head = result?;
    if new_head == plan.expected_head {
        return Ok(new_head);
    }
    let mut transaction = repo.transaction()?;
    transaction.lock_ref(&plan.branch_ref)?;
    // Freeze the ref while performing the final eligibility check and safe
    // checkout so another Git actor cannot move the branch between them.
    validate_clean_attached(&repo, &plan.branch_ref, plan.expected_head)?;
    let object = repo.find_object(new_head, None)?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.safe().update_index(true).overwrite_ignored(false);
    if let Err(error) = repo.checkout_tree(&object, Some(&mut checkout)) {
        return restore_prestate(
            &repo,
            &plan.branch_ref,
            new_head,
            plan.expected_head,
            format!("checkout failed: {error}"),
        );
    }
    if let Err(error) = transaction
        .set_target(&plan.branch_ref, new_head, None, "trench sync")
        .and_then(|()| transaction.commit())
    {
        return restore_prestate(
            &repo,
            &plan.branch_ref,
            new_head,
            plan.expected_head,
            format!("reference update failed: {error}"),
        );
    }
    Ok(new_head)
}

fn validate_clean_attached(
    repo: &Repository,
    branch_ref: &str,
    expected_head: Oid,
) -> Result<(), SyncGitError> {
    if repo.state() != git2::RepositoryState::Clean {
        return Err(SyncGitError::PreconditionsChanged);
    }
    let head = repo
        .head()
        .map_err(|_| SyncGitError::PreconditionsChanged)?;
    if !head.is_branch()
        || head.name() != Some(branch_ref)
        || head.target() != Some(expected_head)
        || repo
            .statuses(Some(
                git2::StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true),
            ))?
            .iter()
            .next()
            .is_some()
    {
        return Err(SyncGitError::PreconditionsChanged);
    }
    Ok(())
}

fn rebase_in_memory(repo: &Repository, plan: &TransactionPlan) -> Result<Oid, SyncGitError> {
    let branch = repo.find_annotated_commit(plan.expected_head)?;
    let base = repo.find_annotated_commit(plan.base_oid)?;
    let mut options = git2::RebaseOptions::new();
    options.inmemory(true);
    let mut rebase = repo.rebase(Some(&branch), Some(&base), None, Some(&mut options))?;
    let signature = repo.signature()?;
    let mut head = None;
    while let Some(operation) = rebase.next() {
        operation.map_err(|_| SyncGitError::Conflict)?;
        if rebase.inmemory_index()?.has_conflicts() {
            return Err(SyncGitError::Conflict);
        }
        head = Some(rebase.commit(None, &signature, None).map_err(|error| {
            if error.code() == git2::ErrorCode::Unmerged {
                SyncGitError::Conflict
            } else {
                SyncGitError::Git(error)
            }
        })?);
    }
    rebase.finish(None)?;
    Ok(head.unwrap_or(plan.base_oid))
}

fn merge_in_memory(repo: &Repository, plan: &TransactionPlan) -> Result<Oid, SyncGitError> {
    let base = repo.find_annotated_commit(plan.base_oid)?;
    let (analysis, _) = repo.merge_analysis(&[&base])?;
    if analysis.is_up_to_date() {
        return Ok(plan.expected_head);
    }
    if analysis.is_fast_forward() {
        return Ok(plan.base_oid);
    }
    let ours = repo.find_commit(plan.expected_head)?;
    let theirs = repo.find_commit(plan.base_oid)?;
    let mut index = repo.merge_commits(&ours, &theirs, None)?;
    if index.has_conflicts() {
        return Err(SyncGitError::Conflict);
    }
    let tree_oid = index.write_tree_to(repo)?;
    let tree = repo.find_tree(tree_oid)?;
    let signature = repo.signature()?;
    let branch = branch_shorthand(&plan.branch_ref);
    Ok(repo.commit(
        None,
        &signature,
        &signature,
        &format!("trench sync: merge base into {branch}"),
        &tree,
        &[&ours, &theirs],
    )?)
}

fn restore_prestate(
    repo: &Repository,
    branch_ref: &str,
    applied: Oid,
    original: Oid,
    cause: String,
) -> Result<Oid, SyncGitError> {
    let cleanup = (|| {
        let live = repo
            .find_reference(branch_ref)?
            .target()
            .ok_or_else(|| git2::Error::from_str("sync branch has no direct target"))?;
        if live != original && live != applied {
            return Err(git2::Error::from_str("sync branch changed before rollback"));
        }
        let object = repo.find_object(original, None)?;
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.safe().update_index(true).overwrite_ignored(false);
        repo.checkout_tree(&object, Some(&mut checkout))?;
        if live == applied {
            repo.reference_matching(branch_ref, original, true, applied, "trench sync rollback")?;
        }
        repo.cleanup_state()?;
        Ok::<(), git2::Error>(())
    })();
    match cleanup {
        Ok(()) => Err(SyncGitError::Git(git2::Error::from_str(&cause))),
        Err(error) => Err(SyncGitError::Rollback(format!("{cause}; {error}"))),
    }
}

fn branch_shorthand(reference: &str) -> &str {
    reference.strip_prefix("refs/heads/").unwrap_or(reference)
}
