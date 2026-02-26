pub mod queries;

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

/// A repository tracked by trench.
#[derive(Debug, Clone)]
pub struct Repo {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub default_base: Option<String>,
    pub created_at: i64,
}

/// A worktree tracked by trench.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub id: i64,
    pub repo_id: i64,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub base_branch: Option<String>,
    pub managed: bool,
    pub adopted_at: Option<i64>,
    pub last_accessed: Option<i64>,
    pub created_at: i64,
}

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

    #[test]
    fn insert_and_get_repo_round_trip() {
        let db = Database::open_in_memory().unwrap();

        let repo = db
            .insert_repo("my-project", "/home/user/my-project", Some("main"))
            .expect("insert_repo should succeed");

        assert_eq!(repo.name, "my-project");
        assert_eq!(repo.path, "/home/user/my-project");
        assert_eq!(repo.default_base.as_deref(), Some("main"));
        assert!(repo.id > 0);
        assert!(repo.created_at > 0);

        let fetched = db
            .get_repo(repo.id)
            .expect("get_repo should succeed")
            .expect("repo should exist");

        assert_eq!(fetched.id, repo.id);
        assert_eq!(fetched.name, repo.name);
        assert_eq!(fetched.path, repo.path);
        assert_eq!(fetched.default_base, repo.default_base);
        assert_eq!(fetched.created_at, repo.created_at);
    }

    #[test]
    fn insert_and_get_worktree_round_trip() {
        let db = Database::open_in_memory().unwrap();
        let repo = db
            .insert_repo("my-project", "/home/user/my-project", Some("main"))
            .unwrap();

        let wt = db
            .insert_worktree(
                repo.id,
                "feature-auth",
                "feature/auth",
                "/home/user/.worktrees/my-project/feature-auth",
                Some("main"),
            )
            .expect("insert_worktree should succeed");

        assert_eq!(wt.repo_id, repo.id);
        assert_eq!(wt.name, "feature-auth");
        assert_eq!(wt.branch, "feature/auth");
        assert_eq!(wt.path, "/home/user/.worktrees/my-project/feature-auth");
        assert_eq!(wt.base_branch.as_deref(), Some("main"));
        assert!(wt.managed);
        assert!(wt.adopted_at.is_none());
        assert!(wt.created_at > 0);

        let fetched = db
            .get_worktree(wt.id)
            .expect("get_worktree should succeed")
            .expect("worktree should exist");

        assert_eq!(fetched.id, wt.id);
        assert_eq!(fetched.name, wt.name);
        assert_eq!(fetched.branch, wt.branch);
        assert_eq!(fetched.path, wt.path);
        assert_eq!(fetched.base_branch, wt.base_branch);
        assert_eq!(fetched.managed, wt.managed);
    }
}
