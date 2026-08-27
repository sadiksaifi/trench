use std::cmp::Reverse;

use crate::tui::app::WorktreeIdentity;

/// Rank the live catalog by a case-insensitive fuzzy subsequence match.
///
/// A row uses its strongest Worktree or Branch score. Equal scores preserve
/// catalog order, followed by Worktree and path for a total deterministic
/// ordering.
pub fn rank<'a>(rows: &'a [WorktreeIdentity], query: &str) -> Vec<&'a WorktreeIdentity> {
    let mut matches = rows
        .iter()
        .enumerate()
        .filter_map(|(catalog_index, row)| {
            let worktree_score = fuzzy_score(&row.worktree, query);
            let branch_score = row
                .branch
                .as_deref()
                .and_then(|branch| fuzzy_score(branch, query));
            worktree_score
                .max(branch_score)
                .map(|score| (score, catalog_index, row))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        (
            Reverse(left.0),
            left.1,
            left.2.worktree.as_str(),
            left.2.path.as_path(),
        )
            .cmp(&(
                Reverse(right.0),
                right.1,
                right.2.worktree.as_str(),
                right.2.path.as_path(),
            ))
    });
    matches.into_iter().map(|(_, _, row)| row).collect()
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<u32> {
    let candidate = candidate.to_lowercase().chars().collect::<Vec<_>>();
    let query = query.to_lowercase().chars().collect::<Vec<_>>();
    if query.is_empty() {
        return Some(0);
    }

    let mut positions = Vec::with_capacity(query.len());
    let mut next = 0;
    for wanted in &query {
        let offset = candidate[next..]
            .iter()
            .position(|character| character == wanted)?;
        let position = next + offset;
        positions.push(position);
        next = position + 1;
    }

    let first = positions[0];
    let last = *positions.last().expect("non-empty query has a last match");
    let span = last - first + 1;
    let contiguous_pairs = positions
        .windows(2)
        .filter(|pair| pair[1] == pair[0] + 1)
        .count();
    let exact = u32::from(query.len() == candidate.len()) * 10_000;
    let prefix = u32::from(first == 0) * 2_000;
    let compact = 1_000_u32.saturating_sub(u32::try_from(span).unwrap_or(u32::MAX));
    let adjacent = u32::try_from(contiguous_pairs).unwrap_or(u32::MAX) * 100;
    let shorter = 500_u32.saturating_sub(u32::try_from(candidate.len()).unwrap_or(u32::MAX));
    Some(exact + prefix + compact + adjacent + shorter)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::tui::app::{WorktreeId, WorktreeIdentity};

    fn identity(path: &str, worktree: &str, branch: Option<&str>) -> WorktreeIdentity {
        WorktreeIdentity {
            id: WorktreeId::new(path),
            worktree: worktree.to_string(),
            branch: branch.map(str::to_string),
            path: PathBuf::from(path),
            head: Some("1234567890abcdef".to_string()),
            is_main: false,
            is_current: false,
            detached: branch.is_none(),
        }
    }

    #[test]
    fn rank_fuzzy_matches_worktree_and_branch_in_deterministic_order() {
        let rows = vec![
            identity("/worktrees/docs", "docs", Some("feature/auth-docs")),
            identity("/worktrees/auth", "feature-auth", Some("feature/login")),
            identity("/worktrees/main", "trench", Some("main")),
        ];

        let ranked = rank(&rows, "auth");

        assert_eq!(
            ranked
                .into_iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
            [rows[1].id.clone(), rows[0].id.clone()]
        );
    }
}
