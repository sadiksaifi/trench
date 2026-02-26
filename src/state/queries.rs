use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use super::{Database, Repo, Worktree};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs() as i64
}

impl Database {
    /// Insert a new repo and return the populated struct.
    pub fn insert_repo(
        &self,
        name: &str,
        path: &str,
        default_base: Option<&str>,
    ) -> Result<Repo> {
        let created_at = now();
        self.conn
            .execute(
                "INSERT INTO repos (name, path, default_base, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![name, path, default_base, created_at],
            )
            .context("failed to insert repo")?;

        let id = self.conn.last_insert_rowid();
        Ok(Repo {
            id,
            name: name.to_string(),
            path: path.to_string(),
            default_base: default_base.map(String::from),
            created_at,
        })
    }

    /// Get a repo by id. Returns `None` if not found.
    pub fn get_repo(&self, id: i64) -> Result<Option<Repo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, path, default_base, created_at FROM repos WHERE id = ?1")
            .context("failed to prepare get_repo query")?;

        let repo = stmt
            .query_row(rusqlite::params![id], |row| {
                Ok(Repo {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    default_base: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .optional()
            .context("failed to get repo")?;

        Ok(repo)
    }

    /// Insert a new worktree and return the populated struct.
    pub fn insert_worktree(
        &self,
        repo_id: i64,
        name: &str,
        branch: &str,
        path: &str,
        base_branch: Option<&str>,
    ) -> Result<Worktree> {
        let created_at = now();
        self.conn
            .execute(
                "INSERT INTO worktrees (repo_id, name, branch, path, base_branch, managed, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                rusqlite::params![repo_id, name, branch, path, base_branch, created_at],
            )
            .context("failed to insert worktree")?;

        let id = self.conn.last_insert_rowid();
        Ok(Worktree {
            id,
            repo_id,
            name: name.to_string(),
            branch: branch.to_string(),
            path: path.to_string(),
            base_branch: base_branch.map(String::from),
            managed: true,
            adopted_at: None,
            last_accessed: None,
            created_at,
        })
    }

    /// Get a worktree by id. Returns `None` if not found.
    pub fn get_worktree(&self, id: i64) -> Result<Option<Worktree>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_id, name, branch, path, base_branch, managed, adopted_at, last_accessed, created_at
             FROM worktrees WHERE id = ?1",
        ).context("failed to prepare get_worktree query")?;

        let wt = stmt
            .query_row(rusqlite::params![id], |row| {
                Ok(Worktree {
                    id: row.get(0)?,
                    repo_id: row.get(1)?,
                    name: row.get(2)?,
                    branch: row.get(3)?,
                    path: row.get(4)?,
                    base_branch: row.get(5)?,
                    managed: row.get::<_, i64>(6)? != 0,
                    adopted_at: row.get(7)?,
                    last_accessed: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .optional()
            .context("failed to get worktree")?;

        Ok(wt)
    }

    /// List all worktrees belonging to a repo.
    pub fn list_worktrees(&self, repo_id: i64) -> Result<Vec<Worktree>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_id, name, branch, path, base_branch, managed, adopted_at, last_accessed, created_at
             FROM worktrees WHERE repo_id = ?1 ORDER BY created_at",
        ).context("failed to prepare list_worktrees query")?;

        let rows = stmt
            .query_map(rusqlite::params![repo_id], |row| {
                Ok(Worktree {
                    id: row.get(0)?,
                    repo_id: row.get(1)?,
                    name: row.get(2)?,
                    branch: row.get(3)?,
                    path: row.get(4)?,
                    base_branch: row.get(5)?,
                    managed: row.get::<_, i64>(6)? != 0,
                    adopted_at: row.get(7)?,
                    last_accessed: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .context("failed to list worktrees")?;

        let mut worktrees = Vec::new();
        for row in rows {
            worktrees.push(row.context("failed to read worktree row")?);
        }
        Ok(worktrees)
    }
}
