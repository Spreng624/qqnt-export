use crate::model::IndexReport;
use crate::proto::parse_message_body;
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy)]
struct TableSpec {
    name: &'static str,
    kind: &'static str,
    message_type_col: &'static str,
    subtype_col: Option<&'static str>,
    address_col: &'static str,
    uid_col: &'static str,
    name_cols: &'static [&'static str],
}

const GROUP_TABLE: TableSpec = TableSpec {
    name: "group_msg_table",
    kind: "group",
    message_type_col: "40011",
    subtype_col: Some("40012"),
    address_col: "40030",
    uid_col: "40021",
    name_cols: &["40090", "40093"],
};

const C2C_TABLE: TableSpec = TableSpec {
    name: "c2c_msg_table",
    kind: "c2c",
    message_type_col: "40011",
    subtype_col: Some("40012"),
    address_col: "40030",
    uid_col: "40021",
    name_cols: &["40093"],
};

pub fn build_library(source: &Path, library: &Path, replace: bool) -> Result<IndexReport> {
    let started = Instant::now();
    let source = fs::canonicalize(source)
        .with_context(|| format!("找不到明文数据库: {}", source.display()))?;
    let library = if library.is_absolute() {
        library.to_path_buf()
    } else {
        std::env::current_dir()?.join(library)
    };
    if source == library {
        bail!("明文数据库和索引库不能是同一个文件");
    }
    if library.exists() {
        if !replace {
            bail!("索引库已存在: {}（使用 --replace 重建）", library.display());
        }
        fs::remove_file(&library)?;
        for suffix in ["-wal", "-shm"] {
            let _ = fs::remove_file(sidecar(&library, suffix));
        }
    }
    if let Some(parent) = library.parent() {
        fs::create_dir_all(parent)?;
    }

    let source_conn = Connection::open_with_flags(
        &source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("无法打开明文数据库: {}", source.display()))?;
    source_conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })?;

    let mut target = Connection::open(&library)
        .with_context(|| format!("无法创建索引库: {}", library.display()))?;
    target
        .execute_batch(
            "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA temp_store = MEMORY;
         PRAGMA foreign_keys = ON;
         CREATE TABLE meta (
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         );
         CREATE TABLE chats (
           id INTEGER PRIMARY KEY,
           kind TEXT NOT NULL,
           remote_id TEXT NOT NULL,
           title TEXT NOT NULL,
           first_timestamp INTEGER NOT NULL DEFAULT 0,
           last_timestamp INTEGER NOT NULL DEFAULT 0,
           message_count INTEGER NOT NULL DEFAULT 0,
           UNIQUE(kind, remote_id)
         );
         CREATE TABLE messages (
           id INTEGER PRIMARY KEY,
           source_table TEXT NOT NULL,
           source_rowid INTEGER NOT NULL,
           msg_id INTEGER NOT NULL,
           chat_id INTEGER NOT NULL REFERENCES chats(id),
           timestamp INTEGER NOT NULL,
           direction INTEGER NOT NULL,
           sender_uid TEXT NOT NULL,
           sender_qq INTEGER,
           sender_name TEXT NOT NULL,
           msg_type INTEGER NOT NULL,
           content_type INTEGER,
           subtype INTEGER,
           text TEXT NOT NULL,
           content TEXT NOT NULL,
           parse_status TEXT NOT NULL,
           UNIQUE(source_table, msg_id)
         );
         CREATE INDEX idx_messages_chat_time ON messages(chat_id, timestamp, msg_id);
         CREATE INDEX idx_messages_time ON messages(timestamp);
         CREATE VIRTUAL TABLE messages_fts USING fts5(
           text,
           content='messages',
           content_rowid='id',
           tokenize='unicode61'
         );
         CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
           INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
         END;
         CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
           INSERT INTO messages_fts(messages_fts, rowid, text)
           VALUES ('delete', old.id, old.text);
         END;
         CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
           INSERT INTO messages_fts(messages_fts, rowid, text)
           VALUES ('delete', old.id, old.text);
           INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
         END;",
        )
        .context("无法创建索引库；当前 SQLite 构建可能缺少 FTS5")?;
    target.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', ?1), ('source', ?2), ('created_at', ?3)",
        params![
            SCHEMA_VERSION.to_string(),
            source.to_string_lossy(),
            chrono::Utc::now().to_rfc3339()
        ],
    )?;

    let transaction = target.transaction()?;
    let mut messages = 0usize;
    let mut skipped = 0usize;
    let mut chat_ids = std::collections::BTreeSet::new();

    for spec in [GROUP_TABLE, C2C_TABLE] {
        if !source_table_exists(&source_conn, spec.name)? {
            eprintln!("跳过不存在的表: {}", spec.name);
            continue;
        }
        let columns = table_columns(&source_conn, spec.name)?;
        let mut compatible = true;
        for required in ["40001", "40050", "40013", "40020", "40033", "40800"] {
            if !columns.iter().any(|column| column == required) {
                eprintln!("跳过 {}：缺少字段 {}", spec.name, required);
                compatible = false;
                break;
            }
        }
        if !columns.iter().any(|column| column == spec.address_col) {
            eprintln!("跳过 {}：缺少会话字段 {}", spec.name, spec.address_col);
            compatible = false;
        }
        if !columns.iter().any(|column| column == spec.uid_col) {
            eprintln!("跳过 {}：缺少会话 UID 字段 {}", spec.name, spec.uid_col);
            compatible = false;
        }
        if !compatible {
            continue;
        }

        let name_expr = spec
            .name_cols
            .iter()
            .filter(|column| columns.iter().any(|item| item == *column))
            .map(|column| format!("NULLIF({}, '')", quote_identifier(column)))
            .collect::<Vec<_>>();
        let name_expr = if name_expr.is_empty() {
            "NULL".to_string()
        } else if name_expr.len() == 1 {
            name_expr[0].clone()
        } else {
            format!("COALESCE({})", name_expr.join(", "))
        };
        let subtype_expr = spec
            .subtype_col
            .filter(|column| columns.iter().any(|item| item == column))
            .map(quote_identifier)
            .unwrap_or_else(|| "NULL".into());
        let sql = format!(
            "SELECT rowid,
                    {msg_id}, {timestamp}, {direction}, {sender_uid}, {sender_qq},
                    {address}, {peer_uid}, {msg_type}, {subtype}, {name_expr}, {body}
             FROM {table}
             ORDER BY rowid",
            msg_id = quote_identifier("40001"),
            timestamp = quote_identifier("40050"),
            direction = quote_identifier("40013"),
            sender_uid = quote_identifier("40020"),
            sender_qq = quote_identifier("40033"),
            address = quote_identifier(spec.address_col),
            peer_uid = quote_identifier(spec.uid_col),
            msg_type = quote_identifier(spec.message_type_col),
            subtype = subtype_expr,
            body = quote_identifier("40800"),
            table = quote_identifier(spec.name),
        );
        let mut statement = source_conn
            .prepare(&sql)
            .with_context(|| format!("无法查询 {}", spec.name))?;
        let mut rows = statement.query([])?;
        let mut table_messages = 0usize;
        while let Some(row) = rows.next()? {
            let source_rowid = row.get::<_, i64>(0)?;
            let msg_id = row.get::<_, Option<i64>>(1)?.unwrap_or(source_rowid);
            let timestamp = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let direction = row.get::<_, Option<i64>>(3)?.unwrap_or(0);
            let sender_uid = row.get::<_, Option<String>>(4)?.unwrap_or_default();
            let sender_qq = row.get::<_, Option<i64>>(5)?;
            let address = sql_value_string(row, 6);
            let peer_uid = sql_value_string(row, 7);
            let msg_type = row.get::<_, Option<i64>>(8)?.unwrap_or(0);
            let subtype = row.get::<_, Option<i64>>(9)?;
            let sender_name = row.get::<_, Option<String>>(10)?.unwrap_or_default();
            let body = row.get::<_, Option<Vec<u8>>>(11)?;
            let remote_id = if !address.is_empty() && address != "0" {
                address
            } else if !peer_uid.is_empty() {
                peer_uid
            } else {
                skipped += 1;
                continue;
            };

            let parsed = parse_message_body(body.as_deref(), msg_type);
            let content = json!({
                "status": parsed.status,
                "segments": parsed.segments,
                "wireFields": parsed.wire_fields,
                "error": parsed.error,
            });
            let text = message_text(&parsed, &content);
            let chat_id = ensure_chat(&transaction, spec.kind, &remote_id, timestamp)?;
            chat_ids.insert(chat_id);
            transaction.execute(
                "INSERT OR IGNORE INTO messages
                 (source_table, source_rowid, msg_id, chat_id, timestamp, direction,
                  sender_uid, sender_qq, sender_name, msg_type, content_type, subtype,
                  text, content, parse_status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    spec.name,
                    source_rowid,
                    msg_id,
                    chat_id,
                    timestamp,
                    direction,
                    sender_uid,
                    sender_qq,
                    sender_name,
                    msg_type,
                    parsed.content_type,
                    subtype,
                    text,
                    content.to_string(),
                    parsed.status,
                ],
            )?;

            messages += 1;
            table_messages += 1;
            if messages % 5000 == 0 {
                eprintln!("已索引 {messages} 条消息...");
            }
        }
        eprintln!("{}: {table_messages} 条", spec.name);
    }

    transaction.execute_batch(
        "UPDATE chats SET
           first_timestamp = COALESCE((SELECT min(timestamp) FROM messages WHERE chat_id = chats.id), 0),
           last_timestamp = COALESCE((SELECT max(timestamp) FROM messages WHERE chat_id = chats.id), 0),
           message_count = (SELECT count(*) FROM messages WHERE chat_id = chats.id);
         INSERT INTO messages_fts(messages_fts) VALUES ('rebuild');",
    )?;
    transaction.commit()?;
    target.execute_batch("PRAGMA optimize;")?;

    Ok(IndexReport {
        library: library.to_string_lossy().into_owned(),
        source: source.to_string_lossy().into_owned(),
        chats: chat_ids.len(),
        messages,
        skipped,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn ensure_chat(
    connection: &rusqlite::Transaction<'_>,
    kind: &str,
    remote_id: &str,
    timestamp: i64,
) -> Result<i64> {
    connection.execute(
        "INSERT INTO chats(kind, remote_id, title, first_timestamp, last_timestamp)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(kind, remote_id) DO UPDATE SET
           first_timestamp = CASE
             WHEN chats.first_timestamp = 0 THEN excluded.first_timestamp
             ELSE min(chats.first_timestamp, excluded.first_timestamp)
           END,
           last_timestamp = max(chats.last_timestamp, excluded.last_timestamp)",
        params![kind, remote_id, remote_id, timestamp],
    )?;
    connection
        .query_row(
            "SELECT id FROM chats WHERE kind=?1 AND remote_id=?2",
            params![kind, remote_id],
            |row| row.get(0),
        )
        .context("无法读取会话 ID")
}

fn message_text(parsed: &crate::proto::ParsedMessageBody, content: &serde_json::Value) -> String {
    if let Some(text) = &parsed.text {
        if !text.is_empty() {
            return text.clone();
        }
    }
    for segment in &parsed.segments {
        if let Some(filename) = &segment.filename {
            if !filename.is_empty() {
                return filename.clone();
            }
        }
    }
    let kind = content
        .get("segments")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("content"))
        .and_then(|item| item.get("type"))
        .and_then(|value| value.as_str());
    match kind {
        Some("image") => "[图片]".into(),
        Some("video") => "[视频]".into(),
        Some("file") => "[文件]".into(),
        Some("sticker") | Some("face") => "[表情]".into(),
        Some("reply") => "[引用消息]".into(),
        Some("forward") | Some("legacyForward") => "[合并转发]".into(),
        Some("call") => "[通话记录]".into(),
        Some("system") => "[系统消息]".into(),
        Some(_) => "[非文本消息]".into(),
        None if parsed.status == "wire_fallback" => "[未知消息结构]".into(),
        None => String::new(),
    }
}

fn source_table_exists(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement =
        connection.prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn sql_value_string(row: &rusqlite::Row<'_>, index: usize) -> String {
    row.get::<_, Option<String>>(index)
        .ok()
        .flatten()
        .or_else(|| {
            row.get::<_, Option<i64>>(index)
                .ok()
                .flatten()
                .map(|value| value.to_string())
        })
        .unwrap_or_default()
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}
