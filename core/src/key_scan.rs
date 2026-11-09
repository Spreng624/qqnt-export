use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const KEY_LOG_PATTERN: &[u8] = b"nt_sqlite3_key_v2: db=%p zDb=%s";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInspection {
    pub wrapper_node: String,
    pub qq_exe: Option<String>,
    pub version: Option<String>,
    pub sha256: String,
    pub function_rva: u64,
    pub lea_instruction_rva: u64,
    pub extraction_supported: bool,
    pub warning: String,
}

#[derive(Debug, Clone)]
struct Section {
    name: String,
    virtual_address: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

pub fn inspect_wrapper(path: Option<&Path>) -> Result<KeyInspection> {
    let wrapper = match path {
        Some(path) => resolve_wrapper(path)?,
        None => find_wrapper()?,
    };
    let bytes = fs::read(&wrapper)
        .with_context(|| format!("无法读取 wrapper.node: {}", wrapper.display()))?;
    let analysis = analyze_pe(&bytes)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let install = if wrapper
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .is_some()
    {
        wrapper
            .ancestors()
            .find(|path| path.join("QQ.exe").is_file())
            .map(Path::to_path_buf)
    } else {
        None
    };
    let qq_exe = install.as_ref().map(|path| path.join("QQ.exe"));
    let version = wrapper
        .ancestors()
        .filter_map(Path::file_name)
        .filter_map(|name| name.to_str())
        .find(|name| {
            name.chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
                && name.contains('-')
        })
        .map(ToOwned::to_owned);

    Ok(KeyInspection {
        wrapper_node: wrapper.to_string_lossy().into_owned(),
        qq_exe: qq_exe.map(|path| path.to_string_lossy().into_owned()),
        version,
        sha256,
        function_rva: analysis.function_rva,
        lea_instruction_rva: analysis.lea_rva,
        extraction_supported: cfg!(all(target_os = "windows", target_arch = "x86_64")),
        warning: "调试器密钥截获依赖当前 QQ 版本，并可能触发 QQ 稳定性或账号安全策略。".into(),
    })
}

struct PeAnalysis {
    function_rva: u64,
    lea_rva: u64,
}

fn analyze_pe(bytes: &[u8]) -> Result<PeAnalysis> {
    if read_u16(bytes, 0)? != 0x5a4d {
        bail!("wrapper.node 不是有效 PE 文件");
    }
    let pe_offset = read_u32(bytes, 0x3c)? as usize;
    if read_u32(bytes, pe_offset)? != 0x0000_4550 {
        bail!("wrapper.node 的 PE 签名无效");
    }
    let coff = pe_offset + 4;
    let section_count = read_u16(bytes, coff + 2)? as usize;
    let optional_size = read_u16(bytes, coff + 16)? as usize;
    let optional = coff + 20;
    if read_u16(bytes, optional)? != 0x20b {
        bail!("当前仅支持 PE32+ 的 wrapper.node");
    }
    let directory = optional + 112;
    let exception_rva = read_u32(bytes, directory + 3 * 8)?;
    let exception_size = read_u32(bytes, directory + 3 * 8 + 4)?;
    let section_table = optional + optional_size;

    let mut sections = Vec::new();
    for index in 0..section_count {
        let offset = section_table + index * 40;
        let name_bytes = bytes
            .get(offset..offset + 8)
            .ok_or_else(|| anyhow::anyhow!("PE 节表被截断"))?;
        let name = String::from_utf8_lossy(name_bytes)
            .trim_end_matches('\0')
            .to_owned();
        sections.push(Section {
            name,
            virtual_size: read_u32(bytes, offset + 8)?,
            virtual_address: read_u32(bytes, offset + 12)?,
            raw_size: read_u32(bytes, offset + 16)?,
            raw_offset: read_u32(bytes, offset + 20)?,
        });
    }

    let rdata = sections
        .iter()
        .find(|section| section.name == ".rdata")
        .context("wrapper.node 中缺少 .rdata")?;
    let rdata_start = rdata.raw_offset as usize;
    let rdata_end = rdata_start
        .checked_add(rdata.raw_size as usize)
        .filter(|end| *end <= bytes.len())
        .context(".rdata 范围无效")?;
    let pattern_offset = find_bytes(&bytes[rdata_start..rdata_end], KEY_LOG_PATTERN)
        .map(|offset| rdata_start + offset)
        .context("未找到 nt_sqlite3_key_v2 日志字符串，QQ 版本可能已变化")?;
    let string_rva = rdata.virtual_address as u64 + (pattern_offset - rdata_start) as u64;

    let text = sections
        .iter()
        .find(|section| section.name == ".text")
        .context("wrapper.node 中缺少 .text")?;
    let text_start = text.raw_offset as usize;
    let text_end = text_start
        .checked_add(text.raw_size as usize)
        .filter(|end| *end <= bytes.len())
        .context(".text 范围无效")?;
    let mut lea_rva = None;
    let text_bytes = &bytes[text_start..text_end];
    for index in 1..text_bytes.len().saturating_sub(6) {
        if text_bytes[index] != 0x8d {
            continue;
        }
        let rex = text_bytes[index - 1];
        if rex & 0xf8 != 0x48 {
            continue;
        }
        let modrm = text_bytes[index + 1];
        if modrm & 0xc7 != 0x05 {
            continue;
        }
        let displacement = i32::from_le_bytes(
            text_bytes[index + 2..index + 6]
                .try_into()
                .expect("slice length is fixed"),
        );
        let instruction_rva = text.virtual_address as u64 + (index - 1) as u64;
        let target = (instruction_rva as i64 + 7 + displacement as i64) as u64;
        if target == string_rva {
            lea_rva = Some(instruction_rva);
            break;
        }
    }
    let lea_rva = lea_rva.context("找到日志字符串，但未定位到引用它的 LEA 指令")?;

    let exception_section = sections
        .iter()
        .find(|section| {
            exception_rva >= section.virtual_address
                && exception_rva < section.virtual_address.saturating_add(section.virtual_size)
        })
        .context("找不到异常目录所在节")?;
    let exception_offset = exception_section.raw_offset as usize
        + (exception_rva - exception_section.virtual_address) as usize;
    let entry_count = exception_size as usize / 12;
    let mut left = 0usize;
    let mut right = entry_count;
    let mut function_rva = None;
    while left < right {
        let middle = left + (right - left) / 2;
        let offset = exception_offset + middle * 12;
        let begin = read_u32(bytes, offset)?;
        let end = read_u32(bytes, offset + 4)?;
        if lea_rva < begin as u64 {
            right = middle;
        } else if lea_rva >= end as u64 {
            left = middle + 1;
        } else {
            function_rva = Some(begin as u64);
            break;
        }
    }

    Ok(PeAnalysis {
        function_rva: function_rva.context("异常目录中找不到目标函数")?,
        lea_rva,
    })
}

fn resolve_wrapper(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(fs::canonicalize(path)?);
    }
    if path.is_dir() {
        return find_wrapper_in(path)
            .with_context(|| format!("目录中没有找到 wrapper.node: {}", path.display()));
    }
    bail!("路径不存在: {}", path.display())
}

fn find_wrapper() -> Result<PathBuf> {
    let mut roots = Vec::new();
    roots.extend(registry_install_dirs());
    if let Some(value) = env::var_os("ProgramFiles").map(PathBuf::from) {
        roots.push(value.join("Tencent").join("QQNT"));
    }
    if let Some(value) = env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
        roots.push(value.join("Tencent").join("QQNT"));
    }
    if let Some(value) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        roots.push(value.join("Programs").join("Tencent").join("QQNT"));
    }
    for root in roots {
        if let Ok(wrapper) = find_wrapper_in(&root) {
            return Ok(wrapper);
        }
    }
    bail!("未自动找到 QQNT。请使用 --wrapper 指定 wrapper.node 或其安装目录")
}

#[cfg(target_os = "windows")]
fn registry_install_dirs() -> Vec<PathBuf> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ,
    };

    let keys = [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\QQ",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\NTQQ",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\NTQQ",
        ),
    ];
    let mut output = Vec::new();
    for (root, subkey) in keys {
        let subkey = subkey
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut key = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        let value_name = "UninstallString"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut buffer = vec![0u16; 4096];
        let mut size = (buffer.len() * 2) as u32;
        let result = unsafe {
            RegQueryValueExW(
                key,
                value_name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buffer.as_mut_ptr() as *mut u8,
                &mut size,
            )
        };
        unsafe { RegCloseKey(key) };
        if result != ERROR_SUCCESS {
            continue;
        }
        let length = buffer
            .iter()
            .position(|item| *item == 0)
            .unwrap_or(size as usize / 2);
        let uninstall = String::from_utf16_lossy(&buffer[..length]);
        let executable = if let Some(rest) = uninstall.strip_prefix('"') {
            rest.split('"').next().unwrap_or(rest)
        } else {
            uninstall
                .to_ascii_lowercase()
                .find(".exe")
                .map(|index| &uninstall[..index + 4])
                .unwrap_or_else(|| uninstall.split_whitespace().next().unwrap_or(""))
        };
        if let Some(parent) = Path::new(executable).parent() {
            if parent.is_dir() {
                output.push(parent.to_path_buf());
            }
        }
    }
    output
}

#[cfg(not(target_os = "windows"))]
fn registry_install_dirs() -> Vec<PathBuf> {
    Vec::new()
}

fn find_wrapper_in(root: &Path) -> Result<PathBuf> {
    let versions = root.join("versions");
    if versions.is_dir() {
        let mut candidates = fs::read_dir(&versions)?
            .filter_map(Result::ok)
            .map(|entry| {
                entry
                    .path()
                    .join("resources")
                    .join("app")
                    .join("wrapper.node")
            })
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(path) = candidates.pop() {
            return Ok(path);
        }
    }
    let direct = root.join("resources").join("app").join("wrapper.node");
    if direct.is_file() {
        return Ok(direct);
    }
    for entry in walkdir::WalkDir::new(root)
        .max_depth(6)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("wrapper.node"))
        {
            return Ok(entry.path().to_path_buf());
        }
    }
    bail!("未找到 wrapper.node")
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .context("PE 文件被截断")?
        .try_into()
        .unwrap();
    Ok(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .context("PE 文件被截断")?
        .try_into()
        .unwrap();
    Ok(u32::from_le_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::find_bytes;

    #[test]
    fn finds_pattern() {
        assert_eq!(find_bytes(b"abc needle xyz", b"needle"), Some(4));
    }
}
