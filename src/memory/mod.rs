//! Persistent cross-session memory system backed by SQLite.
//!
//! Ported from Go: internal/memory/memory.go + internal/memory/sqlite.go
//!
//! Three memory kinds:
//! - **Episodic**: conversation summaries, past events
//! - **Semantic**: facts, knowledge, user preferences
//! - **Procedural**: how-to, workflows, learned patterns
//!
//! Each kind has a character budget that caps how much gets injected into prompts.

use std::fmt;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Classifies a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Episodic,
    Semantic,
    Procedural,
}

impl fmt::Display for MemoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Episodic => write!(f, "episodic"),
            Self::Semantic => write!(f, "semantic"),
            Self::Procedural => write!(f, "procedural"),
        }
    }
}

impl MemoryKind {
    fn from_str_lossy(s: &str) -> Self {
        match s {
            "semantic" => Self::Semantic,
            "procedural" => Self::Procedural,
            _ => Self::Episodic,
        }
    }
}

/// A single piece of persistent knowledge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub kind: MemoryKind,
    pub content: String,
    pub tags: Vec<String>,
    pub score: f64,
    pub access_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Manages cross-session memory with bounded storage.
pub struct MemoryManager {
    db: Mutex<Connection>,
    max_episodic: usize,
    max_semantic: usize,
    max_procedural: usize,
}

impl MemoryManager {
    /// Open (or create) the memory database at `~/.marsclaw/memory.db`.
    pub fn new() -> anyhow::Result<Self> {
        let dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".marsclaw");
        std::fs::create_dir_all(&dir)?;

        let db_path = dir.join("memory.db");
        let conn = Connection::open(db_path)?;
        migrate(&conn)?;

        Ok(Self {
            db: Mutex::new(conn),
            max_episodic: 4000,
            max_semantic: 4000,
            max_procedural: 2000,
        })
    }

    /// Open a memory database at a custom path (useful for testing).
    pub fn with_path(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        migrate(&conn)?;

        Ok(Self {
            db: Mutex::new(conn),
            max_episodic: 4000,
            max_semantic: 4000,
            max_procedural: 2000,
        })
    }

    /// Store a new memory entry. Returns the generated ID.
    pub fn remember(
        &self,
        kind: MemoryKind,
        content: &str,
        tags: &[&str],
    ) -> anyhow::Result<String> {
        let now = Utc::now();
        let id = format!("mem_{}", now.timestamp_nanos_opt().unwrap_or(0));
        let tags_json = serde_json::to_string(&tags)?;

        let db = self.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        db.execute(
            "INSERT INTO memory_entries (id, kind, content, tags, score, access_cnt, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1.0, 0, ?5, ?6)",
            params![
                id,
                kind.to_string(),
                content,
                tags_json,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )?;

        Ok(id)
    }

    /// Search memory for entries matching `query` keywords. Bumps access counts.
    pub fn recall(&self, query: &str, limit: usize) -> anyhow::Result<Vec<Memory>> {
        let limit = if limit == 0 { 10 } else { limit };
        let db = self.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut entries = search(&db, query, limit)?;

        for entry in &mut entries {
            entry.access_count += 1;
            update_access(&db, &entry.id, entry.access_count)?;
        }

        Ok(entries)
    }

    /// Remove a memory entry by ID.
    pub fn forget(&self, id: &str) -> anyhow::Result<()> {
        let db = self.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        db.execute("DELETE FROM memory_entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Build a `<memory>` block for agent prompt injection.
    ///
    /// Each memory kind has a character budget. Entries are added in relevance
    /// order until the budget for that kind is exhausted.
    pub fn inject(&self, query: &str) -> String {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(_) => return String::new(),
        };

        let entries = match search(&db, query, 20) {
            Ok(e) if !e.is_empty() => e,
            _ => return String::new(),
        };

        let mut budget_episodic = self.max_episodic as i64;
        let mut budget_semantic = self.max_semantic as i64;
        let mut budget_procedural = self.max_procedural as i64;

        let mut out = String::from("<memory>\n");

        for entry in &entries {
            let remaining = match entry.kind {
                MemoryKind::Episodic => &mut budget_episodic,
                MemoryKind::Semantic => &mut budget_semantic,
                MemoryKind::Procedural => &mut budget_procedural,
            };

            if *remaining <= 0 {
                continue;
            }

            let mut line = format!("[{}] {}\n", entry.kind, entry.content);
            let line_len = line.len() as i64;

            if line_len > *remaining {
                line.truncate(*remaining as usize);
                line.push_str("...\n");
            }

            *remaining -= line.len() as i64;
            out.push_str(&line);
        }

        out.push_str("</memory>");
        out
    }
}

// ---------------------------------------------------------------------------
// SQLite helpers
// ---------------------------------------------------------------------------

fn migrate(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_entries (
            id         TEXT PRIMARY KEY,
            kind       TEXT NOT NULL,
            content    TEXT NOT NULL,
            tags       TEXT DEFAULT '[]',
            score      REAL DEFAULT 1.0,
            access_cnt INTEGER DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_memory_kind ON memory_entries(kind);",
    )?;
    Ok(())
}

fn search(conn: &Connection, query: &str, limit: usize) -> anyhow::Result<Vec<Memory>> {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return list_all(conn, limit);
    }

    let conditions: Vec<String> = words
        .iter()
        .map(|_| "LOWER(content) LIKE ?".to_string())
        .collect();
    let where_clause = conditions.join(" OR ");

    let sql = format!(
        "SELECT id, kind, content, tags, score, access_cnt, created_at, updated_at
         FROM memory_entries
         WHERE {where_clause}
         ORDER BY score DESC, updated_at DESC
         LIMIT ?",
    );

    let mut stmt = conn.prepare(&sql)?;
    let param_count = words.len() + 1;
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(param_count);
    for w in &words {
        params_vec.push(Box::new(format!("%{}%", w.to_lowercase())));
    }
    params_vec.push(Box::new(limit as i64));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(params_refs.as_slice(), scan_row)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

fn list_all(conn: &Connection, limit: usize) -> anyhow::Result<Vec<Memory>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, content, tags, score, access_cnt, created_at, updated_at
         FROM memory_entries ORDER BY updated_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], scan_row)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

fn scan_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let kind_str: String = row.get(1)?;
    let tags_str: String = row.get(3)?;
    let created_str: String = row.get(6)?;
    let updated_str: String = row.get(7)?;

    let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
    let created_at = DateTime::parse_from_rfc3339(&created_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_str)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(Memory {
        id: row.get(0)?,
        kind: MemoryKind::from_str_lossy(&kind_str),
        content: row.get(2)?,
        tags,
        score: row.get(4)?,
        access_count: row.get(5)?,
        created_at,
        updated_at,
    })
}

fn update_access(conn: &Connection, id: &str, count: i32) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE memory_entries SET access_cnt = ?1, updated_at = ?2 WHERE id = ?3",
        params![count, Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> MemoryManager {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        MemoryManager {
            db: Mutex::new(conn),
            max_episodic: 4000,
            max_semantic: 4000,
            max_procedural: 2000,
        }
    }

    #[test]
    fn remember_and_recall() {
        let mgr = test_manager();
        let id = mgr.remember(MemoryKind::Semantic, "Rust is fast", &["lang"]).unwrap();
        assert!(id.starts_with("mem_"));

        let results = mgr.recall("rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Rust is fast");
        assert_eq!(results[0].access_count, 1);
    }

    #[test]
    fn forget_removes_entry() {
        let mgr = test_manager();
        let id = mgr.remember(MemoryKind::Episodic, "temp note", &[]).unwrap();
        mgr.forget(&id).unwrap();

        let results = mgr.recall("temp", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn inject_formats_memory_block() {
        let mgr = test_manager();
        mgr.remember(MemoryKind::Semantic, "user prefers dark mode", &[]).unwrap();
        mgr.remember(MemoryKind::Episodic, "discussed dark mode yesterday", &[]).unwrap();

        let block = mgr.inject("dark mode");
        assert!(block.starts_with("<memory>"));
        assert!(block.ends_with("</memory>"));
        assert!(block.contains("[semantic]"));
        assert!(block.contains("[episodic]"));
    }

    #[test]
    fn inject_empty_when_no_matches() {
        let mgr = test_manager();
        let block = mgr.inject("nonexistent query");
        assert!(block.is_empty());
    }

    #[test]
    fn recall_bumps_access_count() {
        let mgr = test_manager();
        mgr.remember(MemoryKind::Procedural, "always run tests", &[]).unwrap();

        mgr.recall("tests", 10).unwrap();
        let results = mgr.recall("tests", 10).unwrap();
        assert_eq!(results[0].access_count, 2);
    }

    #[test]
    fn memory_kind_display() {
        assert_eq!(MemoryKind::Episodic.to_string(), "episodic");
        assert_eq!(MemoryKind::Semantic.to_string(), "semantic");
        assert_eq!(MemoryKind::Procedural.to_string(), "procedural");
    }
}
