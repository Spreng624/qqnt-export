use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub account: String,
    pub db_path: String,
    pub db_size: u64,
    pub nt_dir: String,
    pub wal_present: bool,
    pub header_size: u64,
    pub key_meta_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub platform: String,
    pub sqlcipher: bool,
    pub data_roots: Vec<RootInfo>,
    pub accounts: Vec<AccountInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub path: String,
    pub size: u64,
    pub header_size: u64,
    pub payload_size: u64,
    pub page_count: u64,
    pub key_meta_sha256: Option<String>,
    pub key_meta: Option<String>,
    pub wal_present: bool,
    pub shm_present: bool,
    pub looks_like_qqnt: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub id: i64,
    pub kind: String,
    pub remote_id: String,
    pub address: String,
    pub title: String,
    pub first_timestamp: i64,
    pub last_timestamp: i64,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub id: i64,
    pub msg_id: i64,
    pub kind: String,
    pub chat_id: String,
    pub timestamp: i64,
    pub timestamp_iso: String,
    pub direction: i64,
    pub sender_uid: String,
    pub sender_qq: Option<i64>,
    pub sender_name: String,
    pub msg_type: i64,
    pub content_type: Option<i64>,
    pub subtype: Option<i64>,
    pub text: String,
    pub content: serde_json::Value,
    pub parse_status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReport {
    pub library: String,
    pub source: String,
    pub chats: usize,
    pub messages: usize,
    pub skipped: usize,
    pub elapsed_ms: u128,
}
