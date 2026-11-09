use crate::model::MessageRecord;
use anyhow::{bail, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

pub fn export_messages(
    chat: &str,
    format: &str,
    output: &Path,
    messages: &[MessageRecord],
) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(output)
        .with_context(|| format!("无法写入导出文件: {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    match format.to_ascii_lowercase().as_str() {
        "json" => {
            serde_json::to_writer_pretty(
                &mut writer,
                &serde_json::json!({
                    "format": "qqnt-export/v1",
                    "chat": chat,
                    "exportedAt": chrono::Utc::now().to_rfc3339(),
                    "messageCount": messages.len(),
                    "messages": messages,
                }),
            )?;
            writer.write_all(b"\n")?;
        }
        "jsonl" | "ndjson" => {
            for message in messages {
                serde_json::to_writer(&mut writer, message)?;
                writer.write_all(b"\n")?;
            }
        }
        "csv" => write_csv(&mut writer, messages)?,
        "md" | "markdown" => write_markdown(&mut writer, chat, messages)?,
        "html" => write_html(&mut writer, chat, messages)?,
        other => bail!("不支持的导出格式 {other}，可用 json/jsonl/csv/markdown/html"),
    }
    writer.flush()?;
    restrict_file(output)?;
    Ok(())
}

fn write_csv(writer: &mut impl Write, messages: &[MessageRecord]) -> Result<()> {
    writer.write_all(
        "timestamp,isoTime,direction,senderQq,senderName,chatType,remoteId,msgType,contentType,text\r\n"
            .as_bytes(),
    )?;
    for message in messages {
        let fields = [
            message.timestamp.to_string(),
            message.timestamp_iso.clone(),
            message.direction.to_string(),
            message
                .sender_qq
                .map(|value| value.to_string())
                .unwrap_or_default(),
            message.sender_name.clone(),
            message.kind.clone(),
            message.chat_id.clone(),
            message.msg_type.to_string(),
            message
                .content_type
                .map(|value| value.to_string())
                .unwrap_or_default(),
            message.text.clone(),
        ];
        let line = fields
            .iter()
            .map(|value| csv_escape(value))
            .collect::<Vec<_>>()
            .join(",");
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\r\n")?;
    }
    Ok(())
}

fn write_markdown(writer: &mut impl Write, chat: &str, messages: &[MessageRecord]) -> Result<()> {
    writeln!(writer, "# QQ 聊天记录")?;
    writeln!(writer)?;
    writeln!(writer, "- 会话: `{chat}`")?;
    writeln!(writer, "- 消息数: {}", messages.len())?;
    writeln!(
        writer,
        "- 导出时间: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )?;
    writeln!(writer)?;
    for message in messages {
        let sender = if message.direction == 1 || message.direction == 2 {
            "我".to_string()
        } else if !message.sender_name.is_empty() {
            message.sender_name.clone()
        } else {
            message
                .sender_qq
                .map(|value| value.to_string())
                .unwrap_or_else(|| "未知发送者".into())
        };
        writeln!(
            writer,
            "### {} · {}",
            escape_html(&sender).replace('\n', " "),
            escape_html(&display_time(message))
        )?;
        writeln!(writer)?;
        writeln!(
            writer,
            "{}",
            escape_html(&message.text).replace('\n', "  \n")
        )?;
        writeln!(writer)?;
    }
    Ok(())
}

fn write_html(writer: &mut impl Write, chat: &str, messages: &[MessageRecord]) -> Result<()> {
    writeln!(
        writer,
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>QQ 聊天记录</title><style>\
         :root{{color-scheme:light dark}}body{{font-family:system-ui,sans-serif;max-width:860px;margin:2rem auto;padding:0 1rem;line-height:1.6}}\
         header{{border-bottom:1px solid #8884;margin-bottom:2rem}}.msg{{margin:1.25rem 0;padding:.8rem 1rem;background:#8881;border-radius:.6rem}}\
         .meta{{font-size:.82rem;opacity:.65;margin-bottom:.4rem}}.text{{white-space:pre-wrap;overflow-wrap:anywhere}}\
         </style></head><body><header><h1>QQ 聊天记录</h1><p>会话：{} · {} 条消息</p></header>",
        escape_html(chat),
        messages.len()
    )?;
    for message in messages {
        let sender = if message.direction == 1 || message.direction == 2 {
            "我".to_string()
        } else if !message.sender_name.is_empty() {
            message.sender_name.clone()
        } else {
            message
                .sender_qq
                .map(|value| value.to_string())
                .unwrap_or_else(|| "未知发送者".into())
        };
        writeln!(
            writer,
            "<article class=\"msg\"><div class=\"meta\">{} · {}</div><div class=\"text\">{}</div></article>",
            escape_html(&sender),
            escape_html(&display_time(message)),
            escape_html(&message.text)
        )?;
    }
    writeln!(writer, "</body></html>")?;
    Ok(())
}

fn display_time(message: &MessageRecord) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(message.timestamp, 0)
        .map(|value| {
            value
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "时间未知".into())
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\r') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}
