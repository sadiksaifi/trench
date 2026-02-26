use anyhow::{bail, Context, Result};
use rusqlite::OptionalExtension;

use super::{unix_epoch_secs, Database, Repo, Worktree, WorktreeUpdate};

fn now() -> i64 {
    unix_epoch_secs() as i64
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

    /// Get a repo by its filesystem path. Returns `None` if not found.
    pub fn get_repo_by_path(&self, path: &str) -> Result<Option<Repo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, path, default_base, created_at FROM repos WHERE path = ?1")
            .context("failed to prepare get_repo_by_path query")?;

        let repo = stmt
            .query_row(rusqlite::params![path], |row| {
                Ok(Repo {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    default_base: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .optional()
            .context("failed to get repo by path")?;

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

    /// Update selected fields on a worktree. Only `Some` fields are written.
    pub fn update_worktree(&self, id: i64, update: &WorktreeUpdate) -> Result<()> {
        let mut sets = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref v) = update.last_accessed {
            sets.push("last_accessed = ?");
            params.push(Box::new(*v));
        }
        if let Some(ref v) = update.adopted_at {
            sets.push("adopted_at = ?");
            params.push(Box::new(*v));
        }
        if let Some(v) = update.managed {
            sets.push("managed = ?");
            params.push(Box::new(v as i64));
        }
        if let Some(ref v) = update.base_branch {
            sets.push("base_branch = ?");
            params.push(Box::new(v.clone()));
        }

        if sets.is_empty() {
            return Ok(());
        }

        params.push(Box::new(id));
        let sql = format!(
            "UPDATE worktrees SET {} WHERE id = ?",
            sets.join(", ")
        );
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let affected = self
            .conn
            .execute(&sql, param_refs.as_slice())
            .context("failed to update worktree")?;

        if affected == 0 {
            bail!("worktree with id {id} not found");
        }

        Ok(())
    }

    /// Insert an event and return its id.
    pub fn insert_event(
        &self,
        repo_id: i64,
        worktree_id: Option<i64>,
        event_type: &str,
        payload: Option<&serde_json::Value>,
    ) -> Result<i64> {
        let created_at = now();
        let payload_str = payload.map(|v| v.to_string());
        self.conn
            .execute(
                "INSERT INTO events (repo_id, worktree_id, event_type, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![repo_id, worktree_id, event_type, payload_str, created_at],
            )
            .context("failed to insert event")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Count events for a worktree, optionally filtered by event type.
    pub fn count_events(
        &self,
        worktree_id: i64,
        event_type: Option<&str>,
    ) -> Result<i64> {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match event_type {
            Some(et) => (
                "SELECT COUNT(*) FROM events WHERE worktree_id = ?1 AND event_type = ?2",
                vec![Box::new(worktree_id), Box::new(et.to_string())],
            ),
            None => (
                "SELECT COUNT(*) FROM events WHERE worktree_id = ?1",
                vec![Box::new(worktree_id)],
            ),
        };
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let count: i64 = self
            .conn
            .query_row(sql, param_refs.as_slice(), |row| row.get(0))
            .context("failed to count events")?;
        Ok(count)
    }
}
