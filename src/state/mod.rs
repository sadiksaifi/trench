mod queries;

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

/// Core database handle wrapping a SQLite connection with migrations applied.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the database at the given file path.
    ///
    /// Applies pragmas (WAL, FK, synchronous NORMAL) and runs all pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open database at {}", path.display()))?;
        Self::init(conn)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("failed to open in-memory database")?;
        Self::init(conn)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )
        .context("failed to set database pragmas")?;

        Self::migrations()
            .to_latest(&mut conn)
            .context("failed to run database migrations")?;

        Ok(Self { conn })
    }

    fn migrations() -> Migrations<'static> {
        Migrations::new(vec![M::up(include_str!("sql/001_initial_schema.sql"))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_applies_pragmas_and_creates_tables() {
        let db = Database::open_in_memory().expect("should open in-memory database");

        // Verify pragmas
        let fk: i64 = db
            .conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys should be ON");

        let sync: i64 = db
            .conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        assert_eq!(sync, 1, "synchronous should be NORMAL (1)");

        // Verify all 6 tables exist
        let tables = vec!["repos", "worktrees", "events", "logs", "tags", "session"];
        for table in &tables {
            let exists: bool = db
                .conn
                .prepare(&format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{}'",
                    table
                ))
                .unwrap()
                .query_row([], |row| row.get::<_, i64>(0))
                .map(|count| count > 0)
                .unwrap();
            assert!(exists, "table '{}' should exist", table);
        }
    }
}
