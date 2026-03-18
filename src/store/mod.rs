//! Session and message persistence backed by SQLite.
//!
//! Ported from Go: internal/store/store.go + sqlite.go

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::types::Message;

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Store trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait Store: Send + Sync {
    async fn create_session(&self, session: &Session) -> anyhow::Result<()>;
    async fn get_session(&self, id: &str) -> anyhow::Result<Option<Session>>;
    async fn list_sessions(&self) -> anyhow::Result<Vec<Session>>;
    async fn delete_session(&self, id: &str) -> anyhow::Result<()>;
    async fn update_title(&self, id: &str, title: &str) -> anyhow::Result<()>;
    async fn append_messages(&self, session_id: &str, msgs: &[Message]) -> anyhow::Result<()>;
    async fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>>;
}

// ---------------------------------------------------------------------------
// SQLite implementation
// ---------------------------------------------------------------------------

pub struct SqliteStore {
    db: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    /// Opens or creates a SQLite database at ~/.marsclaw/marsclaw.db.
    pub fn new() -> anyhow::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home dir"))?;
        let dir = home.join(".marsclaw");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("marsclaw.db");
        Self::open(path)
    }

    /// Opens or creates a SQLite database at the given path.
    pub fn open(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id         TEXT PRIMARY KEY,
                title      TEXT NOT NULL DEFAULT '',
                source     TEXT NOT NULL DEFAULT 'cli',
                metadata   TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS messages (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role        TEXT NOT NULL,
                content     TEXT,
                tool_calls  TEXT,
                tool_result TEXT,
                timestamp   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, id);",
        )?;

        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
        })
    }
}

/// Helper: run a blocking closure on the SQLite connection via spawn_blocking.
macro_rules! with_db {
    ($self:expr, $closure:expr) => {{
        let db = Arc::clone(&$self.db);
        tokio::task::spawn_blocking(move || {
            let conn = db.blocking_lock();
            $closure(&conn)
        })
        .await?
    }};
}

#[async_trait::async_trait]
impl Store for SqliteStore {
    async fn create_session(&self, session: &Session) -> anyhow::Result<()> {
        let session = session.clone();
        with_db!(self, move |conn: &Connection| -> anyhow::Result<()> {
            let meta = session
                .metadata
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_default());
            conn.execute(
                "INSERT INTO sessions (id, title, source, metadata, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session.id,
                    session.title,
                    session.source,
                    meta,
                    session.created_at.to_rfc3339(),
                    session.updated_at.to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    async fn get_session(&self, id: &str) -> anyhow::Result<Option<Session>> {
        let id = id.to_owned();
        with_db!(self, move |conn: &Connection| -> anyhow::Result<Option<Session>> {
            let mut stmt = conn.prepare(
                "SELECT id, title, source, metadata, created_at, updated_at FROM sessions WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![id])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            Ok(Some(row_to_session(row)?))
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        with_db!(self, move |conn: &Connection| -> anyhow::Result<Vec<Session>> {
            let mut stmt = conn.prepare(
                "SELECT id, title, source, metadata, created_at, updated_at FROM sessions ORDER BY updated_at DESC",
            )?;
            let mut rows = stmt.query([])?;
            let mut sessions = Vec::new();
            while let Some(row) = rows.next()? {
                sessions.push(row_to_session(row)?);
            }
            Ok(sessions)
        })
    }

    async fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        let id = id.to_owned();
        with_db!(self, move |conn: &Connection| -> anyhow::Result<()> {
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    async fn update_title(&self, id: &str, title: &str) -> anyhow::Result<()> {
        let id = id.to_owned();
        let title = title.to_owned();
        with_db!(self, move |conn: &Connection| -> anyhow::Result<()> {
            conn.execute(
                "UPDATE sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title, Utc::now().to_rfc3339(), id],
            )?;
            Ok(())
        })
    }

    async fn append_messages(&self, session_id: &str, msgs: &[Message]) -> anyhow::Result<()> {
        let session_id = session_id.to_owned();
        let msgs = msgs.to_vec();
        with_db!(self, move |conn: &Connection| -> anyhow::Result<()> {
            let tx = conn.unchecked_transaction()?;

            {
                let mut stmt = tx.prepare(
                    "INSERT INTO messages (session_id, role, content, tool_calls, tool_result, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                for m in &msgs {
                    let role = serde_json::to_value(&m.role)?;
                    let role_str = role.as_str().unwrap_or("user");

                    let tool_calls: Option<String> = if m.tool_calls.is_empty() {
                        None
                    } else {
                        Some(serde_json::to_string(&m.tool_calls)?)
                    };

                    let tool_result: Option<String> = m
                        .tool_result
                        .as_ref()
                        .map(|tr| serde_json::to_string(tr))
                        .transpose()?;

                    stmt.execute(params![
                        session_id,
                        role_str,
                        m.content,
                        tool_calls,
                        tool_result,
                        m.timestamp.to_rfc3339(),
                    ])?;
                }
            }

            tx.execute(
                "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                params![Utc::now().to_rfc3339(), session_id],
            )?;

            tx.commit()?;
            Ok(())
        })
    }

    async fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        let session_id = session_id.to_owned();
        with_db!(self, move |conn: &Connection| -> anyhow::Result<Vec<Message>> {
            let mut stmt = conn.prepare(
                "SELECT role, content, tool_calls, tool_result, timestamp FROM messages WHERE session_id = ?1 ORDER BY id",
            )?;
            let mut rows = stmt.query(params![session_id])?;
            let mut messages = Vec::new();
            while let Some(row) = rows.next()? {
                messages.push(row_to_message(row)?);
            }
            Ok(messages)
        })
    }
}

// ---------------------------------------------------------------------------
// Row converters
// ---------------------------------------------------------------------------

fn row_to_session(row: &rusqlite::Row<'_>) -> anyhow::Result<Session> {
    let meta_str: Option<String> = row.get(3)?;
    let metadata = meta_str
        .and_then(|s| serde_json::from_str(&s).ok());

    let created_str: String = row.get(4)?;
    let updated_str: String = row.get(5)?;

    Ok(Session {
        id: row.get(0)?,
        title: row.get(1)?,
        source: row.get(2)?,
        metadata,
        created_at: parse_datetime(&created_str),
        updated_at: parse_datetime(&updated_str),
    })
}

fn row_to_message(row: &rusqlite::Row<'_>) -> anyhow::Result<Message> {
    let role_str: String = row.get(0)?;
    let content: Option<String> = row.get(1)?;
    let tool_calls_str: Option<String> = row.get(2)?;
    let tool_result_str: Option<String> = row.get(3)?;
    let ts_str: String = row.get(4)?;

    let role = serde_json::from_value(serde_json::Value::String(role_str))?;

    let tool_calls = tool_calls_str
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();

    let tool_result = tool_result_str
        .map(|s| serde_json::from_str(&s))
        .transpose()?;

    Ok(Message {
        role,
        content: content.unwrap_or_default(),
        tool_calls,
        tool_result,
        token_count: 0,
        timestamp: parse_datetime(&ts_str),
    })
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
