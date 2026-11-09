use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
enum WireValue {
    Varint(u64),
    Bytes(Vec<u8>),
    Fixed64([u8; 8]),
    Fixed32([u8; 4]),
}

#[derive(Debug, Clone)]
struct WireField {
    number: u32,
    wire_type: u8,
    value: WireValue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSegment {
    pub content_type: Option<i64>,
    pub segment_id: Option<i64>,
    pub media_subtype: Option<i64>,
    pub sender_uid: Option<String>,
    pub text: Option<String>,
    pub filename: Option<String>,
    pub file_size: Option<i64>,
    pub md5: Option<String>,
    pub cdn_url: Option<String>,
    pub local_path: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration: Option<i64>,
    pub content: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMessageBody {
    pub status: String,
    pub content_type: Option<i64>,
    pub text: Option<String>,
    pub segments: Vec<MessageSegment>,
    pub wire_fields: Vec<Value>,
    pub error: Option<String>,
}

pub fn parse_message_body(blob: Option<&[u8]>, table_msg_type: i64) -> ParsedMessageBody {
    let Some(blob) = blob.filter(|value| !value.is_empty()) else {
        return ParsedMessageBody {
            status: "null".into(),
            content_type: None,
            text: None,
            segments: Vec::new(),
            wire_fields: Vec::new(),
            error: None,
        };
    };

    let outer = match parse_wire(blob) {
        Ok(fields) => fields,
        Err(error) => {
            return ParsedMessageBody {
                status: "invalid".into(),
                content_type: None,
                text: None,
                segments: Vec::new(),
                wire_fields: Vec::new(),
                error: Some(error),
            };
        }
    };

    let mut segment_blobs = Vec::new();
    let mut canonical = !outer.is_empty();
    for field in &outer {
        if field.number == 40800 && field.wire_type == 2 {
            if let WireValue::Bytes(value) = &field.value {
                segment_blobs.push(value.clone());
            } else {
                canonical = false;
            }
        } else {
            canonical = false;
        }
    }

    if !canonical || segment_blobs.is_empty() {
        return ParsedMessageBody {
            status: "wire_fallback".into(),
            content_type: None,
            text: None,
            segments: Vec::new(),
            wire_fields: outer.iter().map(wire_field_json).collect(),
            error: if outer.is_empty() {
                Some("消息体为空".into())
            } else {
                Some("消息体不符合当前 MsgBody schema，已保留 wire 字段".into())
            },
        };
    }

    let mut segments = Vec::new();
    let mut typed = true;
    let mut errors = Vec::new();
    for blob in segment_blobs {
        match parse_segment(&blob, table_msg_type) {
            Ok(segment) => segments.push(segment),
            Err(error) => {
                typed = false;
                errors.push(error);
            }
        }
    }

    let text = collect_text(&segments);
    ParsedMessageBody {
        status: if typed { "typed" } else { "wire_fallback" }.into(),
        content_type: segments.first().and_then(|segment| segment.content_type),
        text,
        segments: if typed { segments } else { Vec::new() },
        wire_fields: if typed {
            Vec::new()
        } else {
            outer.iter().map(wire_field_json).collect()
        },
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    }
}

fn parse_segment(blob: &[u8], table_msg_type: i64) -> Result<MessageSegment, String> {
    let fields = parse_wire(blob)?;
    let content_type = int_field(&fields, 45002);
    let segment_id = int_field(&fields, 45001);
    let media_subtype = int_field(&fields, 45003);
    let sender_uid = string_field(&fields, 40020);
    let text = string_field(&fields, 45101);
    let filename = string_field(&fields, 45402);
    let file_size = int_field(&fields, 45405);
    let md5_raw = bytes_field(&fields, 45406);
    let md5 = md5_raw.map(|bytes| {
        if bytes.len() == 16 {
            hex::encode(bytes)
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        }
    });
    let cdn_url = string_field(&fields, 45802)
        .or_else(|| string_field(&fields, 45803))
        .or_else(|| string_field(&fields, 45804));
    let local_path = string_field(&fields, 45812);
    let width = int_field(&fields, 45411).or_else(|| int_field(&fields, 45413));
    let height = int_field(&fields, 45412).or_else(|| int_field(&fields, 45414));
    let duration = int_field(&fields, 45410).or_else(|| int_field(&fields, 45415));
    let content = classify_segment(
        &fields,
        table_msg_type,
        content_type,
        text.as_deref(),
        filename.as_deref(),
        file_size,
        md5.as_deref(),
        cdn_url.as_deref(),
        local_path.as_deref(),
        width,
        height,
        duration,
    );

    Ok(MessageSegment {
        content_type,
        segment_id,
        media_subtype,
        sender_uid,
        text,
        filename,
        file_size,
        md5,
        cdn_url,
        local_path,
        width,
        height,
        duration,
        content,
    })
}

#[allow(clippy::too_many_arguments)]
fn classify_segment(
    fields: &[WireField],
    table_msg_type: i64,
    content_type: Option<i64>,
    text: Option<&str>,
    filename: Option<&str>,
    file_size: Option<i64>,
    md5: Option<&str>,
    cdn_url: Option<&str>,
    local_path: Option<&str>,
    width: Option<i64>,
    height: Option<i64>,
    duration: Option<i64>,
) -> Value {
    let face_text = string_field(fields, 47602);
    let sys_text = string_field(fields, 80900);
    let reply_summary = string_field(fields, 47713);
    let call_desc = string_field(fields, 48153);
    let legacy_xml = string_field(fields, 48602);
    let forward_meta = string_field(fields, 47901);

    let value = match table_msg_type {
        3 => media_json("file", filename, file_size, md5, cdn_url, local_path),
        5 if bytes_field(fields, 47710).is_some()
            || int_field(fields, 47703).is_some()
            || string_field(fields, 47703).is_some() =>
        {
            json!({
                "type": "reply",
                "uid": string_field(fields, 47703),
                "nickname": string_field(fields, 47705),
                "summary": reply_summary,
                "text": text,
            })
        }
        5 => json!({
            "type": "contact",
            "uid": string_field(fields, 47703),
            "nickname": string_field(fields, 47705),
            "remark": string_field(fields, 47706),
        }),
        6 => json!({
            "type": "contact",
            "uid": string_field(fields, 47703),
            "nickname": string_field(fields, 47705),
            "remark": string_field(fields, 47706),
        }),
        7 | 19 => {
            let call_type = int_field(fields, 48151);
            let call_duration = int_field(fields, 48152);
            if table_msg_type == 19 {
                json!({
                    "type": "call",
                    "callType": call_type,
                    "durationMs": call_duration,
                    "description": call_desc,
                })
            } else {
                media_json("video", filename, file_size, md5, cdn_url, local_path)
            }
        }
        8 | 16 => json!({
            "type": "legacyForward",
            "resid": string_field(fields, 48601),
            "uuid": string_field(fields, 48603),
            "xml": legacy_xml,
        }),
        11 => json!({
            "type": "forward",
            "meta": parse_json_or_string(forward_meta),
            "token": string_field(fields, 47902),
            "uuid": string_field(fields, 47904),
        }),
        17 => json!({
            "type": "system",
            "subType": int_field(fields, 80810),
            "text": sys_text.clone().or_else(|| text.map(ToOwned::to_owned)),
        }),
        _ => match content_type {
            Some(1) | Some(14) => json!({
                "type": "text",
                "text": text
                    .map(ToOwned::to_owned)
                    .or_else(|| face_text.clone())
                    .or_else(|| sys_text.clone()),
            }),
            Some(2) => {
                if width.unwrap_or(0) > 0 || height.unwrap_or(0) > 0 {
                    media_json("image", filename, file_size, md5, cdn_url, local_path)
                } else if duration.unwrap_or(0) > 0 || int_field(fields, 47601).is_some() {
                    media_json("video", filename, file_size, md5, cdn_url, local_path)
                } else {
                    json!({"type": "text", "text": text})
                }
            }
            Some(3) => media_json("file", filename, file_size, md5, cdn_url, local_path),
            Some(4) | Some(5) | Some(11) => json!({
                "type": "sticker",
                "md5": bytes_field(fields, 45600).map(hex::encode),
                "fallbackText": first_text_fallback(fields).or(face_text),
            }),
            Some(6) => json!({
                "type": "face",
                "faceId": int_field(fields, 47601),
                "text": face_text.clone().or_else(|| text.map(ToOwned::to_owned)),
            }),
            Some(7) => json!({
                "type": "reply",
                "summary": reply_summary,
                "text": text,
            }),
            Some(8) => json!({
                "type": "system",
                "text": sys_text.clone().or_else(|| text.map(ToOwned::to_owned)),
            }),
            Some(9) => media_json("video", filename, file_size, md5, cdn_url, local_path),
            Some(10) | Some(17) => json!({
                "type": "ark",
                "text": text,
                "raw": string_field(fields, 47901),
            }),
            Some(16) => json!({
                "type": "legacyForward",
                "xml": legacy_xml,
            }),
            Some(21) => json!({
                "type": "call",
                "callType": int_field(fields, 48151),
                "durationMs": int_field(fields, 48152),
                "description": call_desc,
            }),
            _ => json!({
                "type": "unknown",
                "contentType": content_type,
                "text": text
                    .map(ToOwned::to_owned)
                    .or_else(|| face_text.clone())
                    .or_else(|| sys_text.clone()),
            }),
        },
    };

    let mut object = value.as_object().cloned().unwrap_or_default();
    let unknown = unknown_fields(fields);
    if !unknown.is_empty() {
        object.insert("unknownFields".into(), Value::Array(unknown));
    }
    Value::Object(object)
}

fn media_json(
    kind: &str,
    filename: Option<&str>,
    file_size: Option<i64>,
    md5: Option<&str>,
    cdn_url: Option<&str>,
    local_path: Option<&str>,
) -> Value {
    json!({
        "type": kind,
        "filename": filename,
        "fileSize": file_size,
        "md5": md5,
        "cdnUrl": cdn_url,
        "localPath": local_path,
    })
}

fn collect_text(segments: &[MessageSegment]) -> Option<String> {
    let parts = segments
        .iter()
        .filter_map(|segment| {
            segment
                .text
                .clone()
                .or_else(|| segment.content.get("text")?.as_str().map(ToOwned::to_owned))
                .or_else(|| {
                    segment
                        .content
                        .get("fallbackText")?
                        .as_str()
                        .map(ToOwned::to_owned)
                })
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn parse_json_or_string(value: Option<String>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    serde_json::from_str(&value).unwrap_or(Value::String(value))
}

fn first_text_fallback(fields: &[WireField]) -> Option<String> {
    fields.iter().find_map(|field| {
        if field.number == 45815 {
            if let WireValue::Bytes(value) = &field.value {
                return Some(String::from_utf8_lossy(value).into_owned());
            }
        }
        None
    })
}

fn int_field(fields: &[WireField], number: u32) -> Option<i64> {
    fields.iter().find_map(|field| {
        if field.number == number {
            if let WireValue::Varint(value) = &field.value {
                return Some(*value as i64);
            }
        }
        None
    })
}

fn bytes_field(fields: &[WireField], number: u32) -> Option<&[u8]> {
    fields.iter().find_map(|field| {
        if field.number == number {
            if let WireValue::Bytes(value) = &field.value {
                return Some(value.as_slice());
            }
        }
        None
    })
}

fn string_field(fields: &[WireField], number: u32) -> Option<String> {
    let value = bytes_field(fields, number)?;
    if value.iter().all(|byte| *byte == 0) {
        return None;
    }
    let text = String::from_utf8_lossy(value)
        .trim_end_matches('\0')
        .to_owned();
    (!text.is_empty()).then_some(text)
}

fn known_field(number: u32) -> bool {
    matches!(
        number,
        5 | 40010
            | 40020
            | 45001..=45004
            | 45101..=45112
            | 45402..=45424
            | 45501..=45550
            | 45600
            | 45801..=45862
            | 45906..=45954
            | 47401..=47422
            | 47501..=47715
            | 47901..=47904
            | 48151..=48157
            | 48210..=48275
            | 48601..=48603
            | 49154..=49155
            | 80810
            | 80900..=80995
            | 95654
    )
}

fn unknown_fields(fields: &[WireField]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for field in fields {
        if known_field(field.number) || !seen.insert(field.number) {
            continue;
        }
        output.push(wire_field_json(field));
    }
    output
}

fn wire_field_json(field: &WireField) -> Value {
    let mut object = Map::new();
    object.insert("number".into(), json!(field.number));
    object.insert("wireType".into(), json!(field.wire_type));
    match &field.value {
        WireValue::Varint(value) => {
            object.insert("value".into(), json!(value));
        }
        WireValue::Bytes(value) => {
            object.insert("rawValueHex".into(), json!(hex::encode(value)));
            if let Ok(text) = std::str::from_utf8(value) {
                if text
                    .chars()
                    .all(|character| !character.is_control() || character == '\n')
                {
                    object.insert("text".into(), json!(text));
                }
            }
        }
        WireValue::Fixed64(value) => {
            object.insert("rawValueHex".into(), json!(hex::encode(value)));
        }
        WireValue::Fixed32(value) => {
            object.insert("rawValueHex".into(), json!(hex::encode(value)));
        }
    }
    Value::Object(object)
}

fn parse_wire(data: &[u8]) -> Result<Vec<WireField>, String> {
    let mut fields = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let key_start = offset;
        let (key, next) =
            read_varint(data, offset).ok_or_else(|| format!("字段 key 损坏，偏移 {key_start}"))?;
        offset = next;
        let number = (key >> 3) as u32;
        let wire_type = (key & 7) as u8;
        if number == 0 {
            return Err(format!("字段编号为 0，偏移 {key_start}"));
        }
        let value = match wire_type {
            0 => {
                let (value, next) = read_varint(data, offset)
                    .ok_or_else(|| format!("varint 损坏，偏移 {offset}"))?;
                offset = next;
                WireValue::Varint(value)
            }
            1 => {
                let end = offset + 8;
                if end > data.len() {
                    return Err(format!("fixed64 被截断，偏移 {offset}"));
                }
                let bytes: [u8; 8] = data[offset..end].try_into().unwrap();
                offset = end;
                WireValue::Fixed64(bytes)
            }
            2 => {
                let (length, next) = read_varint(data, offset)
                    .ok_or_else(|| format!("bytes 长度损坏，偏移 {offset}"))?;
                offset = next;
                let end = offset
                    .checked_add(length as usize)
                    .ok_or_else(|| format!("bytes 长度溢出，偏移 {offset}"))?;
                if end > data.len() {
                    return Err(format!("bytes 被截断，偏移 {offset}，长度 {length}"));
                }
                let bytes = data[offset..end].to_vec();
                offset = end;
                WireValue::Bytes(bytes)
            }
            5 => {
                let end = offset + 4;
                if end > data.len() {
                    return Err(format!("fixed32 被截断，偏移 {offset}"));
                }
                let bytes: [u8; 4] = data[offset..end].try_into().unwrap();
                offset = end;
                WireValue::Fixed32(bytes)
            }
            other => {
                return Err(format!("不支持 wire type {other}，偏移 {offset}"));
            }
        };
        fields.push(WireField {
            number,
            wire_type,
            value,
        });
    }
    Ok(fields)
}

fn read_varint(data: &[u8], mut offset: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while offset < data.len() && shift <= 63 {
        let byte = data[offset];
        offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some((value, offset));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut value: u64, output: &mut Vec<u8>) {
        while value >= 0x80 {
            output.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        output.push(value as u8);
    }

    fn field(number: u32, wire_type: u8, payload: &[u8], output: &mut Vec<u8>) {
        varint(((number as u64) << 3) | wire_type as u64, output);
        if wire_type == 2 {
            varint(payload.len() as u64, output);
        }
        output.extend_from_slice(payload);
    }

    fn segment(content_type: u64, text: &str) -> Vec<u8> {
        let mut value = Vec::new();
        let mut kind = Vec::new();
        varint(content_type, &mut kind);
        field(45002, 0, &kind, &mut value);
        field(45101, 2, text.as_bytes(), &mut value);
        let mut outer = Vec::new();
        field(40800, 2, &value, &mut outer);
        outer
    }

    #[test]
    fn parses_text_message() {
        let parsed = parse_message_body(Some(&segment(1, "hello")), 2);
        assert_eq!(parsed.status, "typed");
        assert_eq!(parsed.content_type, Some(1));
        assert_eq!(parsed.text.as_deref(), Some("hello"));
        assert_eq!(parsed.segments[0].content["type"], "text");
    }

    #[test]
    fn preserves_unknown_outer_wire_data() {
        let mut value = Vec::new();
        field(12345, 2, b"mystery", &mut value);
        let parsed = parse_message_body(Some(&value), 2);
        assert_eq!(parsed.status, "wire_fallback");
        assert_eq!(parsed.wire_fields[0]["number"], 12345);
    }

    #[test]
    fn rejects_truncated_payload_without_panicking() {
        let parsed = parse_message_body(Some(&[0x82, 0xff, 0xff, 0xff, 0x0f]), 2);
        assert_eq!(parsed.status, "invalid");
    }
}
