use crate::crypto::{decrypt_database, read_key_from_reader, DecryptOptions};
use crate::discovery;
use crate::export::export_messages;
use crate::index::build_library;
use crate::key_capture::capture_database_key;
use crate::key_scan::inspect_wrapper;
use crate::key_store::{load_key, save_key};
use crate::query::{self, MessageFilter};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "qqnt-export-core",
    version,
    about = "qqnt-export 的本地 Rust 核心"
)]
struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    data_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Doctor,
    Accounts,
    Inspect {
        db: PathBuf,
        #[arg(long)]
        show_key_meta: bool,
    },
    Decrypt {
        db: PathBuf,
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        account_key: Option<String>,
    },
    Auth {
        account: String,
        db: PathBuf,
        #[arg(long)]
        wrapper: Option<PathBuf>,
    },
    Index {
        source: PathBuf,
        library: PathBuf,
        #[arg(long)]
        replace: bool,
    },
    Chats {
        library: PathBuf,
    },
    History {
        library: PathBuf,
        chat: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        before: Option<i64>,
        #[arg(long)]
        after: Option<i64>,
        #[arg(long)]
        direction: Option<i64>,
    },
    Search {
        library: PathBuf,
        query: String,
        #[arg(long)]
        chat: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        before: Option<i64>,
        #[arg(long)]
        after: Option<i64>,
        #[arg(long)]
        direction: Option<i64>,
    },
    Export {
        library: PathBuf,
        chat: String,
        output: PathBuf,
        #[arg(long, default_value = "markdown")]
        format: String,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        before: Option<i64>,
        #[arg(long)]
        after: Option<i64>,
        #[arg(long)]
        direction: Option<i64>,
    },
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
}

#[derive(Debug, Subcommand)]
enum KeyCommand {
    Inspect {
        #[arg(long)]
        wrapper: Option<PathBuf>,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => print_json(&discovery::doctor(cli.data_root.as_deref())?),
        Command::Accounts => {
            let accounts = discovery::discover_accounts(cli.data_root.as_deref())?;
            print_json(&serde_json::json!({ "accounts": accounts }))
        }
        Command::Inspect { db, show_key_meta } => {
            let mut inspection = discovery::inspect_database(&db)?;
            if !show_key_meta {
                inspection.key_meta = None;
            }
            print_json(&inspection)
        }
        Command::Decrypt {
            db,
            out,
            force,
            account_key,
        } => {
            let key = if let Some(account) = account_key {
                eprintln!("正在读取保存的数据库密钥。");
                load_key(&account)?
            } else {
                eprintln!("正在创建只读快照并解密；密钥只从标准输入读取。");
                read_key_from_reader(io::stdin().lock())?
            };
            let report = decrypt_database(DecryptOptions {
                source: &db,
                output: &out,
                key,
                force,
            })?;
            print_json(&report)
        }
        Command::Auth {
            account,
            db,
            wrapper,
        } => {
            let captured = capture_database_key(&db, wrapper.as_deref())?;
            let saved_to = save_key(&account, &captured.key)?;
            print_json(&serde_json::json!({
                "account": account,
                "savedTo": saved_to.to_string_lossy(),
                "functionRva": captured.function_rva,
                "version": captured.version,
            }))
        }
        Command::Index {
            source,
            library,
            replace,
        } => print_json(&build_library(&source, &library, replace)?),
        Command::Chats { library } => {
            let chats = query::list_chats(&library)?;
            print_json(&serde_json::json!({ "chats": chats }))
        }
        Command::History {
            library,
            chat,
            limit,
            before,
            after,
            direction,
        } => {
            let messages = query::history(
                &library,
                MessageFilter {
                    chat: Some(chat.clone()),
                    before,
                    after,
                    direction,
                    limit: Some(limit),
                },
            )?;
            print_json(&serde_json::json!({
                "chat": chat,
                "count": messages.len(),
                "messages": messages,
            }))
        }
        Command::Search {
            library,
            query: query_text,
            chat,
            limit,
            before,
            after,
            direction,
        } => {
            let messages = query::search(
                &library,
                &query_text,
                MessageFilter {
                    chat,
                    before,
                    after,
                    direction,
                    limit: Some(limit),
                },
            )?;
            print_json(&serde_json::json!({
                "query": query_text,
                "count": messages.len(),
                "messages": messages,
            }))
        }
        Command::Export {
            library,
            chat,
            output,
            format,
            limit,
            before,
            after,
            direction,
        } => {
            let summary = query::resolve_chat_address(&library, &chat)?;
            let messages = query::history(
                &library,
                MessageFilter {
                    chat: Some(chat.clone()),
                    before,
                    after,
                    direction,
                    limit,
                },
            )?;
            export_messages(&summary.address, &format, &output, &messages)?;
            print_json(&serde_json::json!({
                "output": output.to_string_lossy(),
                "format": format,
                "chat": summary,
                "messageCount": messages.len(),
            }))
        }
        Command::Key {
            command: KeyCommand::Inspect { wrapper },
        } => print_json(&inspect_wrapper(wrapper.as_deref())?),
    }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, value)?;
    use std::io::Write;
    lock.write_all(b"\n")?;
    Ok(())
}

#[allow(dead_code)]
fn parse_timestamp(value: &str) -> Result<i64> {
    if let Ok(timestamp) = value.parse::<i64>() {
        return Ok(timestamp);
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| anyhow::anyhow!("不是有效的 Unix 时间戳或 RFC3339 时间: {value}"))?;
    Ok(parsed.timestamp())
}

#[allow(dead_code)]
fn require_limit(value: usize) -> Result<usize> {
    if value == 0 {
        bail!("limit 必须大于 0");
    }
    Ok(value)
}
