//! SQLite-backed episodic memory store for the FSM agent.
//!
//! Uses WAL journal mode for concurrent readers without blocking writers.
//! The connection is wrapped in `Arc<Mutex<Connection>>` so it can be cloned
//! across threads (UniFFI / tokio spawn_blocking).

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Errors produced by the state-database layer.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("lock poisoned")]
    LockPoisoned,
}

/// A single fact retrieved from episodic memory.
#[derive(Debug, Clone)]
pub struct EpisodicFact {
    pub id: i64,
    pub query: String,
    pub observation: String,
    pub timestamp: String,
}

/// Thread-safe handle to the SQLite state database.
#[derive(Clone)]
pub struct StateDb {
    conn: Arc<Mutex<Connection>>,
}

impl StateDb {
    /// Open (or create) a SQLite database at `db_path` with WAL mode enabled.
    pub fn open(db_path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(db_path)?;
        Self::setup(conn)
    }

    /// Create an in-memory database — useful for unit tests.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        Self::setup(conn)
    }

    fn setup(conn: Connection) -> Result<Self, DbError> {
        // WAL mode: concurrent readers do not block writers.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS episodic_memory (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 query       TEXT    NOT NULL,
                 observation TEXT    NOT NULL,
                 timestamp   TEXT    NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Append a query→observation pair to episodic memory.
    pub fn insert_fact(&self, query: &str, observation: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO episodic_memory (query, observation) VALUES (?1, ?2)",
            params![query, observation],
        )?;
        Ok(())
    }

    /// Return the `limit` most-recently inserted facts (newest first).
    pub fn get_recent_context(&self, limit: usize) -> Result<Vec<EpisodicFact>, DbError> {
        let conn = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, query, observation, timestamp
             FROM episodic_memory
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let facts = stmt
            .query_map(params![limit as i64], |row| {
                Ok(EpisodicFact {
                    id: row.get(0)?,
                    query: row.get(1)?,
                    observation: row.get(2)?,
                    timestamp: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(facts)
    }

    /// Total number of facts stored.
    pub fn fact_count(&self) -> Result<usize, DbError> {
        let conn = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM episodic_memory", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> StateDb {
        StateDb::open_in_memory().expect("in-memory db")
    }

    #[test]
    fn empty_db_has_zero_facts() {
        let db = db();
        assert_eq!(db.fact_count().unwrap(), 0);
    }

    #[test]
    fn insert_and_retrieve_single_fact() {
        let db = db();
        db.insert_fact("what is 2+2?", "4").unwrap();
        let facts = db.get_recent_context(10).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].query, "what is 2+2?");
        assert_eq!(facts[0].observation, "4");
    }

    #[test]
    fn get_recent_context_returns_newest_first() {
        let db = db();
        db.insert_fact("q1", "obs1").unwrap();
        db.insert_fact("q2", "obs2").unwrap();
        db.insert_fact("q3", "obs3").unwrap();
        let facts = db.get_recent_context(10).unwrap();
        // Newest (id=3) should be first.
        assert_eq!(facts[0].query, "q3");
        assert_eq!(facts[2].query, "q1");
    }

    #[test]
    fn get_recent_context_respects_limit() {
        let db = db();
        for i in 0..5 {
            db.insert_fact(&format!("q{i}"), &format!("obs{i}"))
                .unwrap();
        }
        let facts = db.get_recent_context(3).unwrap();
        assert_eq!(facts.len(), 3);
    }

    #[test]
    fn fact_count_increments_correctly() {
        let db = db();
        assert_eq!(db.fact_count().unwrap(), 0);
        db.insert_fact("a", "b").unwrap();
        assert_eq!(db.fact_count().unwrap(), 1);
        db.insert_fact("c", "d").unwrap();
        assert_eq!(db.fact_count().unwrap(), 2);
    }

    #[test]
    fn clone_shares_state() {
        let db1 = db();
        let db2 = db1.clone();
        db1.insert_fact("shared", "fact").unwrap();
        // Both handles see the same underlying connection.
        assert_eq!(db2.fact_count().unwrap(), 1);
    }
}
