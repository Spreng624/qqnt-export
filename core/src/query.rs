use crate::model::{ChatSummary, MessageRecord};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, Row};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct MessageFilter {
    pub chat: Option<String>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub direction: Option<i64>,
    pub limit: Option<usize>,
}

enum SearchMode<'a> {
    Fts(&'a str),
    Like(Vec<&'a str>),
}

pub fn open_library(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("无法打开索引库: {}", path.display()))?;
    conn.query_row(
        "SELECT value FROM meta WHERE key='schema_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .context("文件不是有效的 qqnt-export 索引库")?;
    Ok(conn)
}

pub fn list_chats(library: &Path) -> Result<Vec<ChatSummary>> {
    let conn = open_library(library)?;
    let mut statement = conn.prepare(
        "SELECT id, kind, remote_id, kind || ':' || remote_id AS address, title,
                first_timestamp, last_timestamp, message_count
         FROM chats
         WHERE message_count > 0
         ORDER BY last_timestamp DESC, id DESC",
    )?;
    let rows = statement.query_map([], chat_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn history(library: &Path, filter: MessageFilter) -> Result<Vec<MessageRecord>> {
    let conn = open_library(library)?;
    let chat_id = filter
        .chat
        .as_deref()
        .map(|value| resolve_chat(&conn, value))
        .transpose()?;
    query_messages(&conn, None, chat_id, &filter, true)
}

pub fn search(library: &Path, query: &str, filter: MessageFilter) -> Result<Vec<MessageRecord>> {
    let query = query.trim();
    if query.is_empty() {
        bail!("搜索关键词不能为空");
    }
    let conn = open_library(library)?;
    let chat_id = filter
        .chat
        .as_deref()
        .map(|value| resolve_chat(&conn, value))
        .transpose()?;
    let mode = if query.is_ascii() && query.chars().any(|character| character.is_alphanumeric()) {
        SearchMode::Fts(query)
    } else {
        SearchMode::Like(query.split_whitespace().collect())
    };
    query_messages(&conn, Some(mode), chat_id, &filter, false)
}

pub fn resolve_chat_address(library: &Path, chat: &str) -> Result<ChatSummary> {
    let conn = open_library(library)?;
    let id = resolve_chat(&conn, chat)?;
    conn.query_row(
        "SELECT id, kind, remote_id, kind || ':' || remote_id AS address, title,
                first_timestamp, last_timestamp, message_count
         FROM chats WHERE id=?1",
        [id],
        chat_from_row,
    )
    .context("无法读取会话")
}

fn query_messages(
    conn: &Connection,
    search_mode: Option<SearchMode<'_>>,
    chat_id: Option<i64>,
    filter: &MessageFilter,
    chronological: bool,
) -> Result<Vec<MessageRecord>> {
    let mut conditions = Vec::new();
    let mut values = Vec::<Box<dyn rusqlite::ToSql>>::new();
    if let Some(chat_id) = chat_id {
        conditions.push("m.chat_id = ?".to_string());
        values.push(Box::new(chat_id));
    }
    if let Some(before) = filter.before {
        conditions.push("m.timestamp < ?".to_string());
        values.push(Box::new(before));
    }
    if let Some(after) = filter.after {
        conditions.push("m.timestamp > ?".to_string());
        values.push(Box::new(after));
    }
    if let Some(direction) = filter.direction {
        conditions.push("m.direction = ?".to_string());
        values.push(Box::new(direction));
    }
    match &search_mode {
        Some(SearchMode::Fts(query)) => {
            conditions.push("messages_fts MATCH ?".to_string());
            values.push(Box::new(fts_expression(query)));
        }
        Some(SearchMode::Like(terms)) => {
            for term in terms {
                conditions.push("m.text LIKE ? ESCAPE '\\'".to_string());
                values.push(Box::new(format!("%{}%", escape_like(term))));
            }
        }
        None => {}
    }
    let where_sql = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    let join_sql = if matches!(search_mode, Some(SearchMode::Fts(_))) {
        "JOIN messages_fts ON messages_fts.rowid = m.id"
    } else {
        ""
    };
    let limit_sql = filter
        .limit
        .map(|limit| format!("LIMIT {}", limit.clamp(1, 10_000_000)))
        .unwrap_or_default();
    let sql = format!(
        "SELECT m.id, m.msg_id, c.kind, c.remote_id, m.timestamp, m.direction,
                m.sender_uid, m.sender_qq, m.sender_name, m.msg_type, m.content_type,
                m.subtype, m.text, m.content, m.parse_status
         FROM messages m
         JOIN chats c ON c.id = m.chat_id
         {join_sql}
         {where_sql}
         ORDER BY m.timestamp DESC, m.msg_id DESC
         {limit_sql}"
    );
    let mut statement = conn.prepare(&sql)?;
    let parameters = values
        .iter()
        .map(|value| value.as_ref() as &dyn rusqlite::ToSql)
        .collect::<Vec<_>>();
    let mut rows = statement.query(parameters.as_slice())?;
    let mut messages = Vec::new();
    while let Some(row) = rows.next()? {
        messages.push(message_from_row(row)?);
    }
    if chronological {
        messages.reverse();
    }
    Ok(messages)
}

fn resolve_chat(conn: &Connection, value: &str) -> Result<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("会话标识不能为空");
    }

    if let Some((kind, remote)) = trimmed.split_once(':') {
        let normalized = match kind.to_ascii_lowercase().as_str() {
            "group" | "g" | "群" => "group",
            "c2c" | "friend" | "private" | "私聊" => "c2c",
            _ => bail!("未知会话类型 {kind}，可用 group:ID 或 c2c:ID"),
        };
        return conn
            .query_row(
                "SELECT id FROM chats WHERE kind=?1 AND remote_id=?2",
                params![normalized, remote],
                |row| row.get(0),
            )
            .with_context(|| format!("找不到会话 {normalized}:{remote}"));
    }

    if trimmed.chars().all(|character| character.is_ascii_digit()) {
        let matches = conn
            .prepare("SELECT id, kind FROM chats WHERE remote_id=?1 ORDER BY kind")?
            .query_map([trimmed], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if matches.is_empty() {
            bail!("找不到号码为 {trimmed} 的会话");
        }
        if matches.len() > 1 {
            let options = matches
                .iter()
                .map(|(_, kind)| format!("{kind}:{trimmed}"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("会话号同时匹配群聊和私聊，请明确指定: {options}");
        }
        return Ok(matches[0].0);
    }

    let pattern = format!("%{}%", escape_like(trimmed));
    let matches = conn
        .prepare(
            "SELECT id, kind, remote_id FROM chats
             WHERE title LIKE ?1 ESCAPE '\\' OR remote_id LIKE ?1 ESCAPE '\\'
             ORDER BY last_timestamp DESC LIMIT 20",
        )?
        .query_map([&pattern], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    match matches.as_slice() {
        [] => bail!("找不到会话或标题: {trimmed}"),
        [(id, _, _)] => Ok(*id),
        _ => {
            let options = matches
                .iter()
                .map(|(_, kind, remote)| format!("{kind}:{remote}"))
                .take(8)
                .collect::<Vec<_>>()
                .join(", ");
            bail!("标题匹配到多个会话，请使用号码指定: {options}")
        }
    }
}

fn fts_expression(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn chat_from_row(row: &Row<'_>) -> rusqlite::Result<ChatSummary> {
    Ok(ChatSummary {
        id: row.get(0)?,
        kind: row.get(1)?,
        remote_id: row.get(2)?,
        address: row.get(3)?,
        title: row.get(4)?,
        first_timestamp: row.get(5)?,
        last_timestamp: row.get(6)?,
        message_count: row.get(7)?,
    })
}

fn message_from_row(row: &Row<'_>) -> Result<MessageRecord> {
    let timestamp: i64 = row.get(4)?;
    let content_text: String = row.get(13)?;
    Ok(MessageRecord {
        id: row.get(0)?,
        msg_id: row.get(1)?,
        kind: row.get(2)?,
        chat_id: row.get(3)?,
        timestamp,
        timestamp_iso: chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
            .map(|value| value.to_rfc3339())
            .unwrap_or_default(),
        direction: row.get(5)?,
        sender_uid: row.get(6)?,
        sender_qq: row.get(7)?,
        sender_name: row.get(8)?,
        msg_type: row.get(9)?,
        content_type: row.get(10)?,
        subtype: row.get(11)?,
        text: row.get(12)?,
        content: serde_json::from_str::<Value>(&content_text)
            .unwrap_or_else(|_| serde_json::json!({"raw": content_text})),
        parse_status: row.get(14)?,
    })
}
