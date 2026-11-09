use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub fn save_key(account: &str, key: &str) -> Result<PathBuf> {
    if key.len() != 16 {
        bail!("数据库密钥长度应为 16 字节");
    }
    let path = key_path(account)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        restrict_directory(parent)?;
    }
    let encrypted = protect(key.as_bytes())?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, encrypted)?;
    restrict_file(&temporary)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(&temporary, &path)?;
    Ok(path)
}

pub fn load_key(account: &str) -> Result<Zeroizing<String>> {
    let path = key_path(account)?;
    let encrypted = match fs::read(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!("没有保存的密钥，请先运行 qqnt-export auth -a {account}")
        }
        Err(error) => return Err(error).context(format!("无法读取保存的密钥: {}", path.display())),
    };
    let plain = Zeroizing::new(unprotect(&encrypted)?);
    let key = String::from_utf8(plain.to_vec()).context("保存的数据库密钥格式损坏")?;
    if key.len() != 16 {
        bail!("保存的数据库密钥长度应为 16 字节");
    }
    Ok(Zeroizing::new(key))
}

fn key_path(account: &str) -> Result<PathBuf> {
    if !account.chars().all(|character| character.is_ascii_digit()) {
        bail!("QQ 账号格式无效");
    }
    let root = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .context("无法确定本地应用数据目录")?;
    Ok(root
        .join("qqnt-export")
        .join("keys")
        .join(format!("{account}.bin")))
}

#[cfg(target_os = "windows")]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = data.to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptProtectData(
            &input_blob,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    input.fill(0);
    if result == 0 {
        bail!(
            "Windows DPAPI 加密失败: {}",
            std::io::Error::last_os_error()
        );
    }
    let encrypted =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(encrypted)
}

#[cfg(target_os = "windows")]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut input = data.to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptUnprotectData(
            &input_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if result == 0 {
        bail!(
            "Windows DPAPI 解密失败: {}",
            std::io::Error::last_os_error()
        );
    }
    let plain =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut std::ffi::c_void);
    }
    Ok(plain)
}

#[cfg(not(target_os = "windows"))]
fn protect(_data: &[u8]) -> Result<Vec<u8>> {
    bail!("安全保存密钥目前只支持 Windows")
}

#[cfg(not(target_os = "windows"))]
fn unprotect(_data: &[u8]) -> Result<Vec<u8>> {
    bail!("读取保存的密钥目前只支持 Windows")
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
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{protect, unprotect};

    #[test]
    fn dpapi_round_trip_does_not_store_plaintext() {
        let key = b"0123456789abcdef";
        let encrypted = protect(key).unwrap();
        assert_ne!(encrypted, key);
        assert_eq!(unprotect(&encrypted).unwrap(), key);
    }
}
