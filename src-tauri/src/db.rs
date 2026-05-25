use rusqlite::{params, Connection, OptionalExtension, Result, Row};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub sync_status: String,
    pub retry_count: i64,
    pub target_doc_id: Option<String>,
    pub metadata: String,
    pub synced_at: Option<String>,
}

pub fn init_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            text TEXT NOT NULL,
            created_at TEXT NOT NULL,
            sync_status TEXT NOT NULL DEFAULT 'queued',
            retry_count INTEGER NOT NULL DEFAULT 0,
            target_doc_id TEXT,
            metadata TEXT NOT NULL DEFAULT '{}',
            synced_at TEXT
        );
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )?;
    Ok(conn)
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query_map([key], |row| row.get(0))?;
    rows.next().transpose()
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_last_synced_at(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT created_at FROM messages WHERE sync_status = 'synced' ORDER BY created_at DESC LIMIT 1",
        [],
        |row| row.get(0),
    ).optional()
}

pub fn update_target_doc_id(conn: &Connection, id: &str, doc_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE messages SET target_doc_id = ?1 WHERE id = ?2",
        params![doc_id, id],
    )?;
    Ok(())
}

pub fn insert_message(
    conn: &Connection,
    id: &str,
    text: &str,
    created_at: &str,
    target_doc_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "
        INSERT INTO messages (id, text, created_at, sync_status, retry_count, target_doc_id, metadata)
        VALUES (?1, ?2, ?3, 'queued', 0, ?4, '{}')
        ",
        params![id, text, created_at, target_doc_id],
    )?;
    Ok(())
}

pub fn get_messages(conn: &Connection, limit: i64) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "
        SELECT id, text, created_at, sync_status, retry_count, target_doc_id, metadata, synced_at
        FROM messages
        ORDER BY created_at DESC
        LIMIT ?1
        ",
    )?;
    let mut messages = stmt
        .query_map([limit], row_to_message)?
        .collect::<Result<Vec<_>>>()?;
    messages.reverse();
    Ok(messages)
}

pub fn get_queued_messages(conn: &Connection) -> Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "
        SELECT id, text, created_at, sync_status, retry_count, target_doc_id, metadata, synced_at
        FROM messages
        WHERE sync_status = 'queued'
        ORDER BY created_at ASC
        ",
    )?;
    let messages = stmt.query_map([], row_to_message)?.collect();
    messages
}

pub fn update_sync_status(
    conn: &Connection,
    id: &str,
    status: &str,
    synced_at: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE messages SET sync_status = ?1, synced_at = ?2 WHERE id = ?3",
        params![status, synced_at, id],
    )?;
    Ok(())
}

pub fn increment_retry(conn: &Connection, id: &str) -> Result<i64> {
    conn.execute(
        "UPDATE messages SET retry_count = retry_count + 1 WHERE id = ?1",
        [id],
    )?;
    conn.query_row(
        "SELECT retry_count FROM messages WHERE id = ?1",
        [id],
        |row| row.get(0),
    )
}

pub fn reset_for_retry(conn: &Connection, id: &str) -> Result<Option<Message>> {
    conn.execute(
        "UPDATE messages SET sync_status = 'queued', retry_count = 0, synced_at = NULL WHERE id = ?1",
        [id],
    )?;
    get_message(conn, id)
}

pub fn message_text_exists(conn: &Connection, text: &str, doc_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE text = ?1 AND target_doc_id = ?2",
        params![text, doc_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn insert_remote_message(
    conn: &Connection,
    id: &str,
    text: &str,
    created_at: &str,
    doc_id: &str,
    synced_at: &str,
) -> Result<bool> {
    if message_text_exists(conn, text, doc_id)? {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO messages (id, text, created_at, sync_status, retry_count, target_doc_id, metadata, synced_at)
         VALUES (?1, ?2, ?3, 'synced', 0, ?4, '{\"source\":\"remote\"}', ?5)",
        params![id, text, created_at, doc_id, synced_at],
    )?;
    Ok(true)
}

pub fn get_message(conn: &Connection, id: &str) -> Result<Option<Message>> {
    let mut stmt = conn.prepare(
        "
        SELECT id, text, created_at, sync_status, retry_count, target_doc_id, metadata, synced_at
        FROM messages
        WHERE id = ?1
        ",
    )?;
    let mut rows = stmt.query_map([id], row_to_message)?;
    rows.next().transpose()
}

fn row_to_message(row: &Row<'_>) -> Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        text: row.get(1)?,
        created_at: row.get(2)?,
        sync_status: row.get(3)?,
        retry_count: row.get(4)?,
        target_doc_id: row.get(5)?,
        metadata: row.get(6)?,
        synced_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        init_db(":memory:").expect("init db")
    }

    #[test]
    fn test_init_db() {
        let conn = memory_db();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'messages'",
                [],
                |row| row.get(0),
            )
            .expect("query sqlite schema");

        assert_eq!(exists, 1);
    }

    #[test]
    fn test_insert_and_get() {
        let conn = memory_db();

        insert_message(
            &conn,
            "message-1",
            "hello flash-idea",
            "2026-05-18T10:00:00Z",
            Some("doc-1"),
        )
        .expect("insert message");

        let messages = get_messages(&conn, 10).expect("get messages");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "message-1");
        assert_eq!(messages[0].text, "hello flash-idea");
        assert_eq!(messages[0].created_at, "2026-05-18T10:00:00Z");
        assert_eq!(messages[0].sync_status, "queued");
        assert_eq!(messages[0].retry_count, 0);
        assert_eq!(messages[0].target_doc_id.as_deref(), Some("doc-1"));
        assert_eq!(messages[0].metadata, "{}");
        assert_eq!(messages[0].synced_at, None);
    }

    #[test]
    fn test_update_sync_status() {
        let conn = memory_db();
        insert_message(
            &conn,
            "message-1",
            "hello flash-idea",
            "2026-05-18T10:00:00Z",
            Some("doc-1"),
        )
        .expect("insert message");

        update_sync_status(
            &conn,
            "message-1",
            "synced",
            Some("2026-05-18T10:00:10Z"),
        )
        .expect("update sync status");

        let messages = get_messages(&conn, 10).expect("get messages");
        assert_eq!(messages[0].sync_status, "synced");
        assert_eq!(messages[0].synced_at.as_deref(), Some("2026-05-18T10:00:10Z"));
    }

    #[test]
    fn test_insert_remote_message_dedup() {
        let conn = memory_db();
        insert_message(
            &conn,
            "local-1",
            "hello flash-idea",
            "2026-05-19T10:00:00Z",
            Some("doc-1"),
        )
        .expect("insert local message");

        let inserted = insert_remote_message(
            &conn,
            "remote-1",
            "hello flash-idea",
            "2026-05-19T10:00:01Z",
            "doc-1",
            "2026-05-19T10:01:00Z",
        )
        .expect("insert remote duplicate");
        assert!(!inserted);

        let inserted = insert_remote_message(
            &conn,
            "remote-2",
            "new remote message",
            "2026-05-19T10:02:00Z",
            "doc-1",
            "2026-05-19T10:03:00Z",
        )
        .expect("insert remote new");
        assert!(inserted);

        let messages = get_messages(&conn, 10).expect("get messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].metadata, "{\"source\":\"remote\"}");
        assert_eq!(messages[1].sync_status, "synced");
    }

    #[test]
    fn test_get_queued() {
        let conn = memory_db();
        insert_message(
            &conn,
            "queued-1",
            "queued message",
            "2026-05-18T10:00:00Z",
            Some("doc-1"),
        )
        .expect("insert queued message");
        insert_message(
            &conn,
            "synced-1",
            "synced message",
            "2026-05-18T10:00:01Z",
            Some("doc-1"),
        )
        .expect("insert synced message");
        update_sync_status(&conn, "synced-1", "synced", Some("2026-05-18T10:00:10Z"))
            .expect("mark synced");

        let messages = get_queued_messages(&conn).expect("get queued messages");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "queued-1");
        assert_eq!(messages[0].sync_status, "queued");
    }
}
