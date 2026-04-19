use anyhow::{bail, Context, Result};
use rusqlite::OptionalExtension;

use super::{unix_epoch_secs, Database, Event, LogEntry, Repo, Worktree, WorktreeUpdate};

fn now() -> i64 {
    unix_epoch_secs() as i64
}

impl Database {
    /// Insert a new repo and return the populated struct.
    pub fn insert_repo(&self, name: &str, path: &str, default_base: Option<&str>) -> Result<Repo> {
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

    /// Adopt an externally-created worktree by inserting it with `adopted_at` set.
    ///
    /// Like `insert_worktree`, but marks the worktree as adopted (sets
    /// `adopted_at` to the current timestamp). Used for lazy adoption of
    /// unmanaged worktrees on first interaction.
    pub fn adopt_worktree(
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
                "INSERT INTO worktrees (repo_id, name, branch, path, base_branch, managed, adopted_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                rusqlite::params![repo_id, name, branch, path, base_branch, created_at],
            )
            .context("failed to adopt worktree")?;

        let id = self.conn.last_insert_rowid();
        Ok(Worktree {
            id,
            repo_id,
            name: name.to_string(),
            branch: branch.to_string(),
            path: path.to_string(),
            base_branch: base_branch.map(String::from),
            managed: true,
            adopted_at: Some(created_at),
            last_accessed: None,
            removed_at: None,
            created_at,
        })
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
            removed_at: None,
            created_at,
        })
    }

    /// Get a worktree by id. Returns `None` if not found.
    pub fn get_worktree(&self, id: i64) -> Result<Option<Worktree>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_id, name, branch, path, base_branch, managed, adopted_at, last_accessed, removed_at, created_at
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
                    removed_at: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })
            .optional()
            .context("failed to get worktree")?;

        Ok(wt)
    }

    /// List all worktrees belonging to a repo.
    pub fn list_worktrees(&self, repo_id: i64) -> Result<Vec<Worktree>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_id, name, branch, path, base_branch, managed, adopted_at, last_accessed, removed_at, created_at
             FROM worktrees WHERE repo_id = ?1 AND removed_at IS NULL ORDER BY created_at",
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
                    removed_at: row.get(9)?,
                    created_at: row.get(10)?,
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
        if let Some(ref v) = update.removed_at {
            sets.push("removed_at = ?");
            params.push(Box::new(*v));
        }

        if sets.is_empty() {
            return Ok(());
        }

        params.push(Box::new(id));
        let sql = format!("UPDATE worktrees SET {} WHERE id = ?", sets.join(", "));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let affected = self
            .conn
            .execute(&sql, param_refs.as_slice())
            .context("failed to update worktree")?;

        if affected == 0 {
            bail!("worktree with id {id} not found");
        }

        Ok(())
    }

    /// Find an active worktree by its sanitized name or branch name.
    ///
    /// Only returns worktrees that have not been removed (`removed_at IS NULL`).
    /// Checks the `name` column first (sanitized), then `branch` (original).
    pub fn find_worktree_by_identifier(
        &self,
        repo_id: i64,
        identifier: &str,
    ) -> Result<Option<Worktree>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, repo_id, name, branch, path, base_branch, managed, adopted_at, last_accessed, removed_at, created_at
             FROM worktrees
             WHERE repo_id = ?1 AND (name = ?2 OR branch = ?2) AND removed_at IS NULL
             LIMIT 1",
        ).context("failed to prepare find_worktree_by_identifier query")?;

        let wt = stmt
            .query_row(rusqlite::params![repo_id, identifier], |row| {
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
                    removed_at: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })
            .optional()
            .context("failed to find worktree by identifier")?;

        Ok(wt)
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

    /// Set a session key-value pair (upsert).
    pub fn set_session(&self, key: &str, value: &str) -> Result<()> {
        let updated_at = now();
        self.conn
            .execute(
                "INSERT INTO session (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                rusqlite::params![key, value, updated_at],
            )
            .context("failed to set session key")?;
        Ok(())
    }

    /// Get a session value by key. Returns `None` if not found.
    pub fn get_session(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM session WHERE key = ?1")
            .context("failed to prepare get_session query")?;

        let value = stmt
            .query_row(rusqlite::params![key], |row| row.get(0))
            .optional()
            .context("failed to get session key")?;

        Ok(value)
    }

    /// Save TUI list session state for a repo (selected worktree name + scroll position).
    ///
    /// Both fields are written in a single transaction so they stay consistent.
    pub fn save_list_session(
        &self,
        repo_path: &str,
        worktree_name: &str,
        scroll_position: usize,
    ) -> Result<()> {
        let key_name = format!("{repo_path}:selected_worktree");
        let key_scroll = format!("{repo_path}:scroll_position");
        let updated_at = now();
        let sql = "INSERT INTO session (key, value, updated_at) VALUES (?1, ?2, ?3)
                   ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at";
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin session transaction")?;
        tx.execute(sql, rusqlite::params![key_name, worktree_name, updated_at])
            .context("failed to set selected_worktree")?;
        tx.execute(
            sql,
            rusqlite::params![key_scroll, scroll_position.to_string(), updated_at],
        )
        .context("failed to set scroll_position")?;
        tx.commit().context("failed to commit session")?;
        Ok(())
    }

    /// Load TUI list session state for a repo. Returns `(worktree_name, scroll_position)`.
    pub fn load_list_session(&self, repo_path: &str) -> Result<Option<(String, usize)>> {
        let key_name = format!("{repo_path}:selected_worktree");
        let key_scroll = format!("{repo_path}:scroll_position");
        let name = self.get_session(&key_name)?;
        let scroll = self.get_session(&key_scroll)?;
        match (name, scroll) {
            (Some(n), Some(s)) => {
                let pos = s.parse::<usize>().unwrap_or(0);
                Ok(Some((n, pos)))
            }
            _ => Ok(None),
        }
    }

    /// Add a tag to a worktree. Idempotent — duplicate adds are silently ignored.
    pub fn add_tag(&self, worktree_id: i64, name: &str) -> Result<()> {
        let created_at = now();
        self.conn
            .execute(
                "INSERT OR IGNORE INTO tags (worktree_id, name, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![worktree_id, name, created_at],
            )
            .context("failed to add tag")?;
        Ok(())
    }

    /// List all tags for a worktree, sorted alphabetically.
    pub fn list_tags(&self, worktree_id: i64) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM tags WHERE worktree_id = ?1 ORDER BY name")
            .context("failed to prepare list_tags query")?;

        let rows = stmt
            .query_map(rusqlite::params![worktree_id], |row| row.get(0))
            .context("failed to list tags")?;

        let mut tags = Vec::new();
        for row in rows {
            tags.push(row.context("failed to read tag row")?);
        }
        Ok(tags)
    }

    /// List worktrees that have a specific tag, excluding removed worktrees.
    pub fn list_worktrees_by_tag(&self, repo_id: i64, tag: &str) -> Result<Vec<Worktree>> {
        let mut stmt = self.conn.prepare(
            "SELECT w.id, w.repo_id, w.name, w.branch, w.path, w.base_branch, w.managed, w.adopted_at, w.last_accessed, w.removed_at, w.created_at
             FROM worktrees w
             INNER JOIN tags t ON t.worktree_id = w.id
             WHERE w.repo_id = ?1 AND t.name = ?2 AND w.removed_at IS NULL
             ORDER BY w.created_at",
        ).context("failed to prepare list_worktrees_by_tag query")?;

        let rows = stmt
            .query_map(rusqlite::params![repo_id, tag], |row| {
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
                    removed_at: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })
            .context("failed to list worktrees by tag")?;

        let mut worktrees = Vec::new();
        for row in rows {
            worktrees.push(row.context("failed to read worktree row")?);
        }
        Ok(worktrees)
    }

    /// Remove a tag from a worktree. No-op if the tag doesn't exist.
    pub fn remove_tag(&self, worktree_id: i64, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM tags WHERE worktree_id = ?1 AND name = ?2",
                rusqlite::params![worktree_id, name],
            )
            .context("failed to remove tag")?;
        Ok(())
    }

    /// Insert a single log line for an event.
    pub fn insert_log(
        &self,
        event_id: i64,
        stream: &str,
        line: &str,
        line_number: i64,
        step: Option<&str>,
    ) -> Result<()> {
        let created_at = now();
        self.conn
            .execute(
                "INSERT INTO logs (event_id, stream, line, line_number, step, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![event_id, stream, line, line_number, step, created_at],
            )
            .context("failed to insert log line")?;
        Ok(())
    }

    /// Retrieve hook output lines for an event with full metadata (step, timestamps).
    pub fn get_hook_output(&self, event_id: i64) -> Result<Vec<super::HookOutputLine>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT stream, line, step, line_number, created_at FROM logs
                 WHERE event_id = ?1 ORDER BY line_number",
            )
            .context("failed to prepare get_hook_output query")?;

        let rows = stmt
            .query_map(rusqlite::params![event_id], |row| {
                Ok(super::HookOutputLine {
                    stream: row.get(0)?,
                    line: row.get(1)?,
                    step: row.get(2)?,
                    line_number: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .context("failed to get hook output")?;

        let mut lines = Vec::new();
        for row in rows {
            lines.push(row.context("failed to read hook output line")?);
        }
        Ok(lines)
    }

    /// Retrieve log lines for an event, ordered by line number.
    pub fn get_logs(&self, event_id: i64) -> Result<Vec<(String, String, i64)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT stream, line, line_number FROM logs
                 WHERE event_id = ?1 ORDER BY line_number",
            )
            .context("failed to prepare get_logs query")?;

        let rows = stmt
            .query_map(rusqlite::params![event_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .context("failed to get logs")?;

        let mut logs = Vec::new();
        for row in rows {
            logs.push(row.context("failed to read log row")?);
        }
        Ok(logs)
    }

    /// Count events for a worktree, optionally filtered by event type.
    pub fn count_events(&self, worktree_id: i64, event_type: Option<&str>) -> Result<i64> {
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

    /// List events for a repo with optional worktree filter and limit.
    ///
    /// When `worktree_identifier` is `Some`, only events for the matching
    /// worktree (by name or branch) are returned.
    /// When `limit` is `Some`, at most that many events are returned.
    /// Results are ordered most recent first.
    pub fn list_events_filtered(
        &self,
        repo_id: i64,
        worktree_identifier: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<LogEntry>> {
        let mut sql = String::from(
            "SELECT e.id, e.event_type, w.name, e.payload, e.created_at
             FROM events e
             LEFT JOIN worktrees w
               ON e.worktree_id = w.id
              AND e.repo_id = w.repo_id
             WHERE e.repo_id = ?1",
        );

        // Parameter layout:
        //   ?1 = repo_id (always)
        //   ?2 = worktree_identifier (if Some) or limit (if worktree is None)
        //   ?3 = limit (only when worktree_identifier is also Some)
        if worktree_identifier.is_some() {
            sql.push_str(" AND (w.name = ?2 OR w.branch = ?2)");
        }

        sql.push_str(" ORDER BY e.created_at DESC, e.id DESC");

        if limit.is_some() {
            if worktree_identifier.is_some() {
                sql.push_str(" LIMIT ?3");
            } else {
                sql.push_str(" LIMIT ?2");
            }
        }

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("failed to prepare list_events_filtered query")?;

        let params: Vec<Box<dyn rusqlite::types::ToSql>> = {
            let mut p: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(repo_id)];
            if let Some(id) = worktree_identifier {
                p.push(Box::new(id.to_string()));
            }
            if let Some(lim) = limit {
                p.push(Box::new(lim as i64));
            }
            p
        };
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(LogEntry {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    worktree_name: row.get(2)?,
                    payload: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .context("failed to list filtered events")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("failed to read log entry row")?);
        }
        Ok(entries)
    }

    /// Check whether any worktree (active or removed) exists for the given
    /// identifier (name or branch) in a repo.
    pub fn worktree_exists_any(&self, repo_id: i64, identifier: &str) -> Result<bool> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM worktrees WHERE repo_id = ?1 AND (name = ?2 OR branch = ?2))",
                rusqlite::params![repo_id, identifier],
                |row| row.get(0),
            )
            .context("failed to check worktree existence")?;
        Ok(exists)
    }

    /// Find the most recent hook event for a worktree (by name or branch).
    ///
    /// Returns the last event whose `event_type` starts with `hook:` for the
    /// matching worktree, or `None` if no hook events exist.
    pub fn get_last_hook_event_for_worktree(
        &self,
        repo_id: i64,
        identifier: &str,
    ) -> Result<Option<Event>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT e.id, e.event_type, e.payload, e.created_at
             FROM events e
             JOIN worktrees w ON e.worktree_id = w.id AND e.repo_id = w.repo_id
             WHERE e.repo_id = ?1
               AND (w.name = ?2 OR w.branch = ?2)
               AND e.event_type LIKE 'hook:%'
             ORDER BY e.created_at DESC, e.id DESC
             LIMIT 1",
            )
            .context("failed to prepare get_last_hook_event_for_worktree query")?;

        let event = stmt
            .query_row(rusqlite::params![repo_id, identifier], |row| {
                Ok(Event {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    payload: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .optional()
            .context("failed to get last hook event")?;

        Ok(event)
    }

    /// List events for a worktree, most recent first, up to `limit`.
    pub fn list_events(&self, worktree_id: i64, limit: usize) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, event_type, payload, created_at
             FROM events
             WHERE worktree_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
            )
            .context("failed to prepare list_events query")?;

        let rows = stmt
            .query_map(rusqlite::params![worktree_id, limit as i64], |row| {
                Ok(Event {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    payload: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .context("failed to list events")?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row.context("failed to read event row")?);
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Database;

    #[test]
    fn get_last_hook_event_returns_most_recent_hook() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feat", "feature/feat", "/wt/feat", None)
            .unwrap();

        // Insert a non-hook event, then two hook events
        db.insert_event(repo.id, Some(wt.id), "created", None)
            .unwrap();
        let payload1 =
            serde_json::json!({"hook": "post_create", "exit_code": 0, "duration_secs": 1.0});
        db.insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload1))
            .unwrap();
        let payload2 =
            serde_json::json!({"hook": "pre_sync", "exit_code": 0, "duration_secs": 0.5});
        let last_id = db
            .insert_event(repo.id, Some(wt.id), "hook:pre_sync", Some(&payload2))
            .unwrap();

        let event = db
            .get_last_hook_event_for_worktree(repo.id, "feat")
            .unwrap();
        assert!(event.is_some(), "should find a hook event");
        let event = event.unwrap();
        assert_eq!(event.id, last_id);
        assert_eq!(event.event_type, "hook:pre_sync");
    }

    #[test]
    fn get_last_hook_event_returns_none_when_no_hooks() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feat", "feature/feat", "/wt/feat", None)
            .unwrap();

        // Only non-hook events
        db.insert_event(repo.id, Some(wt.id), "created", None)
            .unwrap();

        let event = db
            .get_last_hook_event_for_worktree(repo.id, "feat")
            .unwrap();
        assert!(event.is_none(), "should return None when no hook events");
    }

    #[test]
    fn get_last_hook_event_matches_by_branch_name() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feat", "feature/feat", "/wt/feat", None)
            .unwrap();

        let payload =
            serde_json::json!({"hook": "post_create", "exit_code": 0, "duration_secs": 1.0});
        db.insert_event(repo.id, Some(wt.id), "hook:post_create", Some(&payload))
            .unwrap();

        // Match by branch name (not sanitized name)
        let event = db
            .get_last_hook_event_for_worktree(repo.id, "feature/feat")
            .unwrap();
        assert!(event.is_some(), "should match by branch name");
    }

    #[test]
    fn list_events_filtered_with_limit_returns_n_most_recent() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "feat", "feat", "/wt/feat", None)
            .unwrap();

        // Insert 5 events
        for _ in 0..5 {
            db.insert_event(repo.id, Some(wt.id), "created", None)
                .unwrap();
        }

        // Limit to 3
        let entries = db.list_events_filtered(repo.id, None, Some(3)).unwrap();
        assert_eq!(entries.len(), 3, "should return exactly 3 events");

        // No limit returns all
        let all = db.list_events_filtered(repo.id, None, None).unwrap();
        assert_eq!(all.len(), 5, "no limit should return all 5 events");
    }

    #[test]
    fn list_events_filtered_by_worktree_name() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt_a = db
            .insert_worktree(repo.id, "alpha", "feature/alpha", "/wt/a", None)
            .unwrap();
        let wt_b = db
            .insert_worktree(repo.id, "beta", "feature/beta", "/wt/b", None)
            .unwrap();

        // 3 events for alpha, 2 for beta
        for _ in 0..3 {
            db.insert_event(repo.id, Some(wt_a.id), "created", None)
                .unwrap();
        }
        for _ in 0..2 {
            db.insert_event(repo.id, Some(wt_b.id), "created", None)
                .unwrap();
        }

        // Filter by sanitized name
        let alpha_events = db
            .list_events_filtered(repo.id, Some("alpha"), None)
            .unwrap();
        assert_eq!(alpha_events.len(), 3);

        // Filter by branch name
        let beta_events = db
            .list_events_filtered(repo.id, Some("feature/beta"), None)
            .unwrap();
        assert_eq!(beta_events.len(), 2);

        // Combined: filter + limit
        let limited = db
            .list_events_filtered(repo.id, Some("alpha"), Some(2))
            .unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn save_and_load_list_session_round_trip() {
        let db = Database::open_in_memory().unwrap();

        // Save session for a repo
        db.save_list_session("/repos/my-project", "feat-auth", 3)
            .unwrap();

        // Load it back
        let session = db.load_list_session("/repos/my-project").unwrap();
        assert!(session.is_some(), "should find saved session");
        let (name, pos) = session.unwrap();
        assert_eq!(name, "feat-auth");
        assert_eq!(pos, 3);
    }

    #[test]
    fn load_list_session_returns_none_when_no_session() {
        let db = Database::open_in_memory().unwrap();

        let session = db.load_list_session("/repos/no-such-repo").unwrap();
        assert!(session.is_none(), "should return None for unknown repo");
    }

    #[test]
    fn save_list_session_overwrites_previous() {
        let db = Database::open_in_memory().unwrap();

        db.save_list_session("/repos/r", "feat-a", 1).unwrap();
        db.save_list_session("/repos/r", "feat-b", 5).unwrap();

        let (name, pos) = db.load_list_session("/repos/r").unwrap().unwrap();
        assert_eq!(name, "feat-b");
        assert_eq!(pos, 5);
    }

    #[test]
    fn save_list_session_isolates_per_repo() {
        let db = Database::open_in_memory().unwrap();

        db.save_list_session("/repos/alpha", "wt-a", 2).unwrap();
        db.save_list_session("/repos/beta", "wt-b", 7).unwrap();

        let (name_a, pos_a) = db.load_list_session("/repos/alpha").unwrap().unwrap();
        assert_eq!(name_a, "wt-a");
        assert_eq!(pos_a, 2);

        let (name_b, pos_b) = db.load_list_session("/repos/beta").unwrap().unwrap();
        assert_eq!(name_b, "wt-b");
        assert_eq!(pos_b, 7);
    }

    #[test]
    fn save_list_session_writes_both_keys_atomically() {
        let db = Database::open_in_memory().unwrap();
        db.save_list_session("/repos/atomic", "wt-x", 42).unwrap();

        // Verify both keys individually via raw get_session
        let name = db.get_session("/repos/atomic:selected_worktree").unwrap();
        let scroll = db.get_session("/repos/atomic:scroll_position").unwrap();
        assert_eq!(name.as_deref(), Some("wt-x"), "worktree name should be set");
        assert_eq!(
            scroll.as_deref(),
            Some("42"),
            "scroll position should be set"
        );

        // Connection should be in autocommit mode (no dangling transaction)
        assert!(
            db.conn_for_test().is_autocommit(),
            "connection should be in autocommit after save (no dangling tx)"
        );
    }

    #[test]
    fn worktree_exists_any_includes_removed() {
        let db = Database::open_in_memory().unwrap();
        let repo = db.insert_repo("r", "/r", None).unwrap();
        let wt = db
            .insert_worktree(repo.id, "gone", "feature/gone", "/wt/gone", None)
            .unwrap();

        assert!(db.worktree_exists_any(repo.id, "gone").unwrap());
        assert!(db.worktree_exists_any(repo.id, "feature/gone").unwrap());
        assert!(!db.worktree_exists_any(repo.id, "nonexistent").unwrap());

        // Mark as removed — should still exist
        db.update_worktree(
            wt.id,
            &crate::state::WorktreeUpdate {
                removed_at: Some(Some(1000)),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            db.worktree_exists_any(repo.id, "gone").unwrap(),
            "removed worktree should still be found"
        );
    }
}
