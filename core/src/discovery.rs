use crate::model::{AccountInfo, DoctorReport, RootInfo};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const HEADER_SIZE: u64 = 1024;

pub fn data_roots(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path.to_path_buf()];
    }

    let mut roots = Vec::new();
    if let Some(profile) = env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.push(profile.join("Documents").join("Tencent Files"));
    }
    if let Some(appdata) = env::var_os("APPDATA").map(PathBuf::from) {
        roots.push(appdata.join("Tencent").join("QQ"));
    }
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        roots.push(local.join("Tencent"));
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join("Documents").join("Tencent Files"));
    }

    let mut unique = Vec::new();
    for root in roots {
        if !unique.iter().any(|item: &PathBuf| item == &root) {
            unique.push(root);
        }
    }
    unique
}

pub fn discover_accounts(explicit: Option<&Path>) -> Result<Vec<AccountInfo>> {
    let mut databases = Vec::new();

    if let Some(path) = explicit {
        collect_databases(path, &mut databases);
    } else {
        for root in data_roots(None) {
            collect_databases(&root, &mut databases);
        }
    }

    let mut accounts = Vec::new();
    let mut seen = Vec::new();
    for db_path in databases {
        let canonical = fs::canonicalize(&db_path).unwrap_or_else(|_| db_path.clone());
        if seen.iter().any(|item: &PathBuf| item == &canonical) {
            continue;
        }
        seen.push(canonical);
        let inspection = inspect_database(&db_path).ok();
        let metadata = fs::metadata(&db_path)
            .with_context(|| format!("无法读取数据库信息: {}", db_path.display()))?;
        let account = account_from_db_path(&db_path).unwrap_or_else(|| "unknown".into());
        let nt_dir = db_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned();
        accounts.push(AccountInfo {
            account,
            db_path: db_path.to_string_lossy().into_owned(),
            db_size: metadata.len(),
            nt_dir,
            wal_present: sidecar(&db_path, "-wal").is_file(),
            header_size: inspection
                .as_ref()
                .map(|item| item.header_size)
                .unwrap_or(0),
            key_meta_sha256: inspection.and_then(|item| item.key_meta_sha256),
        });
    }

    accounts.sort_by(|left, right| left.account.cmp(&right.account));
    Ok(accounts)
}

fn collect_databases(path: &Path, output: &mut Vec<PathBuf>) {
    if path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("nt_msg.db"))
    {
        output.push(path.to_path_buf());
        return;
    }
    if !path.is_dir() {
        return;
    }

    for entry in WalkDir::new(path)
        .max_depth(7)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("nt_msg.db"))
        {
            output.push(entry.path().to_path_buf());
        }
    }
}

fn account_from_db_path(path: &Path) -> Option<String> {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent.file_name().and_then(|name| name.to_str()) == Some("nt_qq") {
            return parent
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned);
        }
        current = parent.parent();
    }
    path.parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
}

pub fn inspect_database(path: &Path) -> Result<crate::model::Inspection> {
    let mut file =
        File::open(path).with_context(|| format!("无法打开数据库: {}", path.display()))?;
    let metadata = file.metadata()?;
    let size = metadata.len();
    let mut first = [0u8; 16];
    let read = file.read(&mut first)?;
    let (header_size, payload_size) = choose_header(size, &first[..read])?;
    let page_count = if payload_size > 0 {
        payload_size.div_ceil(4096)
    } else {
        0
    };

    let mut prefix = vec![0u8; HEADER_SIZE as usize];
    file.seek(SeekFrom::Start(0))?;
    let prefix_len = file.read(&mut prefix)?;
    prefix.truncate(prefix_len);
    let key_meta = find_key_meta(&prefix);

    Ok(crate::model::Inspection {
        path: path.to_string_lossy().into_owned(),
        size,
        header_size,
        payload_size,
        page_count,
        key_meta_sha256: key_meta.as_ref().map(|value| {
            let mut hasher = Sha256::new();
            hasher.update(value.as_bytes());
            hex::encode(hasher.finalize())
        }),
        key_meta,
        wal_present: sidecar(path, "-wal").is_file(),
        shm_present: sidecar(path, "-shm").is_file(),
        looks_like_qqnt: size > HEADER_SIZE && (size - HEADER_SIZE) % 4096 == 0,
    })
}

pub fn choose_header(size: u64, first: &[u8]) -> Result<(u64, u64)> {
    if first.starts_with(b"SQLite format 3\0") {
        return Ok((0, size));
    }
    if size > HEADER_SIZE && (size - HEADER_SIZE) % 4096 == 0 {
        return Ok((HEADER_SIZE, size - HEADER_SIZE));
    }
    if size > 0 && size % 4096 == 0 {
        return Ok((0, size));
    }
    anyhow::bail!(
        "数据库大小不符合 QQ NT 格式（文件 {} 字节，无法识别 1024 字节扩展头）",
        size
    )
}

pub fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn find_key_meta(prefix: &[u8]) -> Option<String> {
    let mut start = None;
    let mut length = 0usize;
    for (index, byte) in prefix.iter().copied().enumerate() {
        if byte.is_ascii_hexdigit() {
            if start.is_none() {
                start = Some(index);
            }
            length += 1;
        } else {
            if length == 128 {
                return start.map(|offset| {
                    String::from_utf8_lossy(&prefix[offset..offset + length]).into_owned()
                });
            }
            start = None;
            length = 0;
        }
    }
    if length == 128 {
        start.map(|offset| String::from_utf8_lossy(&prefix[offset..offset + length]).into_owned())
    } else {
        None
    }
}

pub fn doctor(explicit: Option<&Path>) -> Result<DoctorReport> {
    let accounts = discover_accounts(explicit)?;
    let roots = data_roots(explicit)
        .into_iter()
        .map(|path| RootInfo {
            exists: path.exists(),
            path: path.to_string_lossy().into_owned(),
        })
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    if accounts.is_empty() {
        warnings.push("未找到 nt_msg.db；可用 --data-root 指定 Tencent Files 目录".into());
    }
    let sqlcipher = crate::crypto::find_sqlcipher().is_ok();
    if !sqlcipher {
        warnings.push("未找到 SQLCipher 引擎；请运行 npm run build 准备依赖".into());
    }
    if accounts.iter().any(|account| account.wal_present) {
        warnings.push("检测到 WAL 文件；建议退出 QQ 后再解密，或确认快照一致".into());
    }

    Ok(DoctorReport {
        platform: format!("{}-{}", env::consts::OS, env::consts::ARCH),
        sqlcipher,
        data_roots: roots,
        accounts,
        warnings,
    })
}
