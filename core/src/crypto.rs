use crate::discovery::{choose_header, inspect_database, sidecar};
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct DecryptOptions<'a> {
    pub source: &'a Path,
    pub output: &'a Path,
    pub key: Zeroizing<String>,
    pub force: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecryptReport {
    pub output: String,
    pub source: String,
    pub header_size: u64,
    pub output_bytes: u64,
    pub tables: usize,
    pub integrity: String,
}

pub fn decrypt_database(options: DecryptOptions<'_>) -> Result<DecryptReport> {
    let source = fs::canonicalize(options.source)
        .with_context(|| format!("找不到数据库: {}", options.source.display()))?;
    let output = absolute_path(options.output)?;
    if source == output {
        bail!("输入数据库和输出路径不能相同");
    }
    if output.exists() {
        if !options.force {
            bail!("输出文件已存在: {}（使用 --force 覆盖）", output.display());
        }
        fs::remove_file(&output)
            .with_context(|| format!("无法覆盖输出文件: {}", output.display()))?;
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建输出目录: {}", parent.display()))?;
        restrict_directory(parent)?;
    }

    let inspection = inspect_database(&source)?;
    let work_dir = tempfile::Builder::new()
        .prefix(".qqnt-export-")
        .tempdir_in(output.parent().unwrap_or_else(|| Path::new(".")))
        .context("无法创建安全临时目录")?;
    restrict_directory(work_dir.path())?;

    let snapshot = work_dir.path().join("snapshot.db");
    copy_snapshot(&source, &snapshot)?;

    let key = options.key;
    let tables = if key.trim().is_empty() {
        copy_plain_snapshot(&snapshot, &output)?
    } else {
        decrypt_sqlcipher_snapshot(&snapshot, &output, &key, inspection.header_size)?
    };
    drop(key);

    verify_plain_database(&output)?;
    restrict_file(&output)?;
    let output_bytes = fs::metadata(&output)?.len();
    cleanup_snapshot_sidecars(&work_dir, &snapshot);

    Ok(DecryptReport {
        output: output.to_string_lossy().into_owned(),
        source: source.to_string_lossy().into_owned(),
        header_size: inspection.header_size,
        output_bytes,
        tables,
        integrity: "ok".into(),
    })
}

pub(crate) struct KeyValidator {
    _work_dir: TempDir,
    clear: PathBuf,
    executable: PathBuf,
}

impl KeyValidator {
    pub(crate) fn new(source: &Path) -> Result<Self> {
        let inspection = inspect_database(source)?;
        let work_dir = tempfile::Builder::new()
            .prefix(".qqnt-export-key-")
            .tempdir()
            .context("无法创建密钥验证临时目录")?;
        let snapshot = work_dir.path().join("snapshot.db");
        copy_snapshot(source, &snapshot)?;
        let clear = work_dir.path().join("clear.db");
        strip_header(&snapshot, &clear, inspection.header_size)?;
        let executable = find_sqlcipher()?;
        Ok(Self {
            _work_dir: work_dir,
            clear,
            executable,
        })
    }

    pub(crate) fn validate(&self, key: &str) -> Result<bool> {
        if key.len() != 16 {
            return Ok(false);
        }
        let safe_key = key.replace('\'', "''");
        let sql = Zeroizing::new(format!(
            "PRAGMA cipher_page_size = 4096;\n\
             PRAGMA key = '{safe_key}';\n\
             PRAGMA kdf_iter = 4000;\n\
             PRAGMA cipher_hmac_algorithm = HMAC_SHA1;\n\
             PRAGMA cipher_kdf_algorithm = PBKDF2_HMAC_SHA512;\n\
             SELECT count(*) FROM sqlite_master;\n"
        ));
        let mut child = Command::new(&self.executable)
            .arg(&self.clear)
            .current_dir(self.executable.parent().unwrap_or_else(|| Path::new(".")))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("无法启动 SQLCipher: {}", self.executable.display()))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(sql.as_bytes())?;
        }
        drop(child.stdin.take());
        Ok(child.wait()?.success())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn copy_snapshot(source: &Path, destination: &Path) -> Result<()> {
    copy_file(source, destination)?;
    for suffix in ["-wal", "-shm"] {
        let from = sidecar(source, suffix);
        if from.exists() {
            copy_file(&from, &sidecar(destination, suffix))?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    let input =
        File::open(source).with_context(|| format!("无法读取数据库快照: {}", source.display()))?;
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .with_context(|| format!("无法创建临时快照: {}", destination.display()))?;
    restrict_file(destination)?;
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, input);
    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, output);
    std::io::copy(&mut reader, &mut writer)?;
    writer.flush()?;
    Ok(())
}

fn decrypt_sqlcipher_snapshot(
    snapshot: &Path,
    output: &Path,
    key: &str,
    header_size: u64,
) -> Result<usize> {
    let clear = snapshot.with_file_name("clear.db");
    strip_header(snapshot, &clear, header_size)?;

    for suffix in ["-wal", "-shm"] {
        let from = sidecar(snapshot, suffix);
        if from.exists() {
            let to = sidecar(&clear, suffix);
            if to.exists() {
                fs::remove_file(&to)?;
            }
            fs::rename(&from, &to)?;
        }
    }

    let executable = find_sqlcipher()?;
    let safe_key = key.replace('\'', "''");
    let output_sql = quote_sql_string(&output.to_string_lossy());
    let sql = Zeroizing::new(format!(
        "PRAGMA cipher_page_size = 4096;\n\
         PRAGMA key = '{safe_key}';\n\
         PRAGMA kdf_iter = 4000;\n\
         PRAGMA cipher_hmac_algorithm = HMAC_SHA1;\n\
         PRAGMA cipher_kdf_algorithm = PBKDF2_HMAC_SHA512;\n\
         PRAGMA temp_store = MEMORY;\n\
         SELECT count(*) FROM sqlite_master;\n\
         ATTACH DATABASE {output_sql} AS plaintext KEY '';\n\
         SELECT sqlcipher_export('plaintext');\n\
         DETACH DATABASE plaintext;\n"
    ));

    let mut child = Command::new(&executable)
        .arg(&clear)
        .current_dir(executable.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "无法启动 SQLCipher 引擎: {}。请运行 npm run build 准备依赖",
                executable.display()
            )
        })?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(sql.as_bytes())
            .context("无法向 SQLCipher 传递解密指令")?;
    }
    drop(child.stdin.take());
    let output_process = child
        .wait_with_output()
        .context("等待 SQLCipher 解密进程失败")?;
    drop(sql);
    if !output_process.status.success() {
        let stderr = String::from_utf8_lossy(&output_process.stderr);
        let stdout = String::from_utf8_lossy(&output_process.stdout);
        anyhow::bail!(
            "SQLCipher 解密失败，请检查密钥和数据库版本。\n{}{}",
            stdout.trim(),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!("\n{}", stderr.trim())
            }
        );
    }

    let conn = Connection::open_with_flags(
        output,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let table_count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table'",
        [],
        |row| row.get(0),
    )?;
    drop(conn);
    restrict_file(output)?;
    Ok(table_count as usize)
}

pub(crate) fn find_sqlcipher() -> Result<PathBuf> {
    if let Some(path) = env::var_os("QQNT_EXPORT_SQLCIPHER").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        bail!("QQNT_EXPORT_SQLCIPHER 指向的文件不存在: {}", path.display());
    }

    let mut candidates = Vec::new();
    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            candidates.push(dir.join("sqlcipher.exe"));
            candidates.push(dir.join("sqlcipher").join("sqlcipher.exe"));
            candidates.push(dir.join("vendor").join("sqlcipher").join("sqlcipher.exe"));
        }
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("vendor")
            .join("sqlcipher")
            .join("sqlcipher.exe"),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .context("找不到 SQLCipher 引擎。请运行 npm run build")
}

fn copy_plain_snapshot(snapshot: &Path, output: &Path) -> Result<usize> {
    let header_size = {
        let mut file = File::open(snapshot)?;
        let mut first = [0u8; 16];
        let count = file.read(&mut first)?;
        let size = file.metadata()?.len();
        choose_header(size, &first[..count])?.0
    };
    if header_size == 0 {
        fs::copy(snapshot, output)
            .with_context(|| format!("无法写出明文数据库: {}", output.display()))?;
    } else {
        strip_header(snapshot, output, header_size)?;
    }
    let conn = Connection::open_with_flags(
        output,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("输入不是可识别的明文 SQLite 数据库")?;
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table'",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

fn strip_header(source: &Path, destination: &Path, header_size: u64) -> Result<()> {
    let mut input = BufReader::with_capacity(
        8 * 1024 * 1024,
        File::open(source).with_context(|| format!("无法读取数据库: {}", source.display()))?,
    );
    if header_size > 0 {
        std::io::copy(&mut input.by_ref().take(header_size), &mut std::io::sink())?;
    }
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .with_context(|| format!("无法创建剥离头部后的数据库: {}", destination.display()))?;
    restrict_file(destination)?;
    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, output);
    std::io::copy(&mut input, &mut writer)?;
    writer.flush()?;
    Ok(())
}

fn verify_plain_database(output: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        output,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("无法打开解密后的数据库: {}", output.display()))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .context("解密输出不是有效 SQLite 数据库")?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        bail!("解密数据库完整性检查失败: {integrity}");
    }
    Ok(())
}

fn cleanup_snapshot_sidecars(work_dir: &TempDir, snapshot: &Path) {
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(sidecar(snapshot, suffix));
    }
    let _ = work_dir.path().join("clear.db");
}

fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn read_key_from_reader(reader: impl Read) -> Result<Zeroizing<String>> {
    let mut key = String::new();
    reader
        .take(4097)
        .read_to_string(&mut key)
        .context("无法从标准输入读取密钥")?;
    let key = key.trim_end_matches(['\r', '\n']);
    if key.is_empty() {
        bail!("标准输入中没有密钥");
    }
    if key.len() != 16 {
        bail!("QQ NT 数据库密钥应为 16 字节，实际为 {} 字节", key.len());
    }
    Ok(Zeroizing::new(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::build_library;
    use crate::query;
    use std::io::Write;

    const TEST_KEY: &str = "0123456789abcdef";

    fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
        while value >= 0x80 {
            output.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        output.push(value as u8);
    }

    fn field(number: u32, wire_type: u8, payload: &[u8], output: &mut Vec<u8>) {
        encode_varint(((number as u64) << 3) | wire_type as u64, output);
        if wire_type == 2 {
            encode_varint(payload.len() as u64, output);
        }
        output.extend_from_slice(payload);
    }

    fn text_body(text: &str) -> Vec<u8> {
        let mut segment = Vec::new();
        let mut content_type = Vec::new();
        encode_varint(1, &mut content_type);
        field(45002, 0, &content_type, &mut segment);
        field(45101, 2, text.as_bytes(), &mut segment);
        let mut body = Vec::new();
        field(40800, 2, &segment, &mut body);
        body
    }

    fn encrypted_fixture(root: &Path) -> std::path::PathBuf {
        let encrypted = root.join("encrypted.db");
        let body_hex = hex::encode(text_body("hello from fixture 共同作业"));
        let sql = format!(
            "PRAGMA cipher_page_size = 4096;
             PRAGMA key = '{TEST_KEY}';
             PRAGMA kdf_iter = 4000;
             PRAGMA cipher_hmac_algorithm = HMAC_SHA1;
             PRAGMA cipher_kdf_algorithm = PBKDF2_HMAC_SHA512;
             CREATE TABLE group_msg_table(
               \"40001\" INTEGER PRIMARY KEY,
               \"40050\" INTEGER,
               \"40013\" INTEGER,
               \"40020\" TEXT,
               \"40033\" INTEGER,
               \"40021\" TEXT,
               \"40030\" INTEGER,
               \"40011\" INTEGER,
               \"40012\" INTEGER,
               \"40090\" TEXT,
               \"40093\" TEXT,
               \"40800\" BLOB
             );
             INSERT INTO group_msg_table
             VALUES (1, 1760000000, 0, 'u_peer', 10001, '20002', 20002, 2, 0,
                     'Peer', 'Peer', X'{body_hex}');
             CREATE TABLE c2c_msg_table(
               \"40001\" INTEGER PRIMARY KEY,
               \"40050\" INTEGER,
               \"40013\" INTEGER,
               \"40020\" TEXT,
               \"40033\" INTEGER,
               \"40021\" TEXT,
               \"40030\" INTEGER,
               \"40011\" INTEGER,
               \"40012\" INTEGER,
               \"40093\" TEXT,
               \"40800\" BLOB
             );
             INSERT INTO c2c_msg_table
             VALUES (2, 1760000001, 1, 'u_self', 10001, '30003', 30003, 2, 0,
                     'Self', X'{body_hex}');"
        );
        let executable = find_sqlcipher().unwrap();
        let mut child = Command::new(executable)
            .arg(&encrypted)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(sql.as_bytes())
            .unwrap();
        drop(child.stdin.take());
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );

        let raw = root.join("nt_msg.db");
        let mut output = File::create(&raw).unwrap();
        let mut prefix = vec![b'a'; 128];
        prefix.resize(crate::discovery::HEADER_SIZE as usize, 0);
        output.write_all(&prefix).unwrap();
        std::io::copy(&mut File::open(&encrypted).unwrap(), &mut output).unwrap();
        output.flush().unwrap();
        raw
    }

    #[test]
    fn decrypts_qqnt_header_and_builds_searchable_library() {
        let root = tempfile::tempdir().unwrap();
        let source = encrypted_fixture(root.path());
        let plain = root.path().join("plain.db");
        let library = root.path().join("library.db");

        let validator = KeyValidator::new(&source).unwrap();
        assert!(validator.validate(TEST_KEY).unwrap());
        assert!(!validator.validate("fedcba9876543210").unwrap());
        drop(validator);

        let report = decrypt_database(DecryptOptions {
            source: &source,
            output: &plain,
            key: Zeroizing::new(TEST_KEY.into()),
            force: false,
        })
        .unwrap();
        assert_eq!(report.header_size, crate::discovery::HEADER_SIZE);
        assert!(plain.is_file());

        let index = build_library(&plain, &library, false).unwrap();
        assert_eq!(index.messages, 2);
        let chats = query::list_chats(&library).unwrap();
        assert_eq!(chats.len(), 2);
        assert!(chats.iter().any(|chat| chat.address == "group:20002"));
        assert!(chats.iter().any(|chat| chat.address == "c2c:30003"));
        let found = query::search(
            &library,
            "fixture",
            crate::query::MessageFilter {
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "hello from fixture 共同作业");

        let chinese = query::search(
            &library,
            "作业",
            crate::query::MessageFilter {
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(chinese.len(), 2);
    }
}
