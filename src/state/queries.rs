use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use super::{Database, Repo};

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

    /// Get a repo by id, returning `None` if not found.
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
}
