#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
use anyhow::bail;
use anyhow::Result;
use std::path::Path;
use zeroize::Zeroizing;

pub struct CapturedKey {
    pub key: Zeroizing<String>,
    pub function_rva: u64,
    pub version: Option<String>,
}

pub fn capture_database_key(database: &Path, wrapper: Option<&Path>) -> Result<CapturedKey> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        windows_capture::capture(database, wrapper)
    }
    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        let _ = (database, wrapper);
        bail!("调试器密钥截获目前只支持 Windows x86_64")
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
mod windows_capture {
    use super::CapturedKey;
    use crate::crypto::KeyValidator;
    use crate::key_scan::inspect_wrapper;
    use anyhow::{bail, Context, Result};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{
        CloseHandle, DBG_CONTINUE, ERROR_SEM_TIMEOUT, EXCEPTION_BREAKPOINT, EXCEPTION_SINGLE_STEP,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        ContinueDebugEvent, FlushInstructionCache, GetThreadContext, ReadProcessMemory,
        SetThreadContext, WaitForDebugEvent, WriteProcessMemory, CONTEXT, CONTEXT_ALL_AMD64,
        CREATE_PROCESS_DEBUG_EVENT, DEBUG_EVENT, EXCEPTION_DEBUG_EVENT, EXIT_PROCESS_DEBUG_EVENT,
        LOAD_DLL_DEBUG_EVENT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
        MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, OpenThread, TerminateProcess, DEBUG_ONLY_THIS_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOW, THREAD_ALL_ACCESS,
    };
    use zeroize::Zeroizing;

    const DBG_EXCEPTION_NOT_HANDLED: i32 = 0x8001_0001_u32 as i32;
    const TIMEOUT_MS: u32 = 10_000;
    const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

    pub fn capture(database: &Path, wrapper: Option<&Path>) -> Result<CapturedKey> {
        if qq_is_running()? {
            bail!("检测到 QQ.exe 正在运行，请先完全退出 QQ，再执行 auth");
        }

        let inspection = inspect_wrapper(wrapper)?;
        let qq_exe = inspection
            .qq_exe
            .as_deref()
            .context("找到 wrapper.node，但没有定位到 QQ.exe")?;
        let validator = KeyValidator::new(database)?;
        let child = DebugChild::launch(Path::new(qq_exe))?;

        eprintln!("正在以调试模式启动 QQ。登录目标账号后会自动截获并验证密钥。");
        let mut breakpoint_address = 0usize;
        let mut original_byte = 0u8;
        let mut breakpoint_active = false;
        let mut stepping_thread = None;
        let mut captured = None;
        let started = std::time::Instant::now();

        while started.elapsed() < MAX_WAIT {
            let mut event = DEBUG_EVENT::default();
            if unsafe { WaitForDebugEvent(&mut event, TIMEOUT_MS) } == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_SEM_TIMEOUT as i32) {
                    bail!("等待 QQ 调试事件失败: {error}");
                }
                continue;
            }
            let mut continue_status = DBG_CONTINUE;

            match event.dwDebugEventCode {
                CREATE_PROCESS_DEBUG_EVENT => {
                    let info = unsafe { event.u.CreateProcessInfo };
                    if !info.hFile.is_null() {
                        unsafe { CloseHandle(info.hFile) };
                    }
                }
                LOAD_DLL_DEBUG_EVENT => {
                    let info = unsafe { event.u.LoadDll };
                    if !info.hFile.is_null() {
                        unsafe { CloseHandle(info.hFile) };
                    }
                    if breakpoint_address == 0 {
                        if let Some(base) = module_base(event.dwProcessId, "wrapper.node")? {
                            breakpoint_address = base + inspection.function_rva as usize;
                            original_byte = read_byte(child.process, breakpoint_address)?;
                            write_byte(child.process, breakpoint_address, 0xcc)?;
                            breakpoint_active = true;
                            eprintln!("wrapper.node 已加载，密钥断点已设置。");
                        }
                    }
                }
                EXCEPTION_DEBUG_EVENT => {
                    let info = unsafe { event.u.Exception };
                    let code = info.ExceptionRecord.ExceptionCode;
                    let address = info.ExceptionRecord.ExceptionAddress as usize;
                    if code == EXCEPTION_BREAKPOINT
                        && breakpoint_active
                        && address == breakpoint_address
                    {
                        write_byte(child.process, breakpoint_address, original_byte)?;
                        let context = thread_context(event.dwThreadId)?;
                        if let Some(key) = read_key_at(child.process, context.R8 as usize) {
                            let validated = validator.validate(&key)?;
                            if validated {
                                write_byte(child.process, breakpoint_address, 0xcc)?;
                                unsafe { TerminateProcess(child.process, 0) };
                                captured = Some(Zeroizing::new(key));
                                eprintln!("数据库密钥已截获并验证。");
                            } else {
                                let mut context = context;
                                context.Rip = context.Rip.saturating_sub(1);
                                context.EFlags |= 0x100;
                                set_thread_context(event.dwThreadId, &context)?;
                                breakpoint_active = false;
                                stepping_thread = Some(event.dwThreadId);
                            }
                        } else {
                            let mut context = context;
                            context.Rip = context.Rip.saturating_sub(1);
                            context.EFlags |= 0x100;
                            set_thread_context(event.dwThreadId, &context)?;
                            breakpoint_active = false;
                            stepping_thread = Some(event.dwThreadId);
                        }
                    } else if code == EXCEPTION_SINGLE_STEP
                        && stepping_thread == Some(event.dwThreadId)
                    {
                        clear_single_step(event.dwThreadId)?;
                        if captured.is_none() {
                            write_byte(child.process, breakpoint_address, 0xcc)?;
                            breakpoint_active = true;
                        }
                        stepping_thread = None;
                    } else {
                        continue_status = DBG_EXCEPTION_NOT_HANDLED;
                    }
                }
                EXIT_PROCESS_DEBUG_EVENT => {
                    unsafe {
                        ContinueDebugEvent(event.dwProcessId, event.dwThreadId, continue_status);
                    }
                    break;
                }
                _ => {}
            }

            unsafe {
                ContinueDebugEvent(event.dwProcessId, event.dwThreadId, continue_status);
            }
        }

        let key = captured.context("等待超时，没有截获到可用的数据库密钥")?;
        Ok(CapturedKey {
            key,
            function_rva: inspection.function_rva,
            version: inspection.version,
        })
    }

    struct DebugChild {
        process: *mut c_void,
        thread: *mut c_void,
    }

    impl DebugChild {
        fn launch(path: &Path) -> Result<Self> {
            let application = wide_null(path.as_os_str().encode_wide());
            let directory = path
                .parent()
                .context("QQ.exe 没有父目录")?
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let startup = STARTUPINFOW {
                cb: size_of::<STARTUPINFOW>() as u32,
                ..Default::default()
            };
            let mut information = PROCESS_INFORMATION::default();
            let created = unsafe {
                CreateProcessW(
                    application.as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    DEBUG_ONLY_THIS_PROCESS,
                    std::ptr::null(),
                    directory.as_ptr(),
                    &startup,
                    &mut information,
                )
            };
            if created == 0 {
                return Err(anyhow::anyhow!(
                    "无法启动 QQ 调试实例: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self {
                process: information.hProcess,
                thread: information.hThread,
            })
        }
    }

    impl Drop for DebugChild {
        fn drop(&mut self) {
            unsafe {
                if !self.process.is_null() {
                    TerminateProcess(self.process, 0);
                    CloseHandle(self.process);
                }
                if !self.thread.is_null() {
                    CloseHandle(self.thread);
                }
            }
        }
    }

    fn wide_null(value: impl Iterator<Item = u16>) -> Vec<u16> {
        value.chain(std::iter::once(0)).collect()
    }

    fn qq_is_running() -> Result<bool> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot.is_null() || snapshot == (-1isize as *mut c_void) {
            bail!("无法枚举进程: {}", std::io::Error::last_os_error());
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
        while ok {
            let length = entry
                .szExeFile
                .iter()
                .position(|item| *item == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
            if name.eq_ignore_ascii_case("QQ.exe") {
                found = true;
                break;
            }
            ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        Ok(found)
    }

    fn module_base(process_id: u32, target: &str) -> Result<Option<usize>> {
        let snapshot = unsafe {
            CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id)
        };
        if snapshot.is_null() || snapshot == (-1isize as *mut c_void) {
            return Ok(None);
        }
        let mut entry = MODULEENTRY32W {
            dwSize: size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        let mut result = None;
        let mut ok = unsafe { Module32FirstW(snapshot, &mut entry) } != 0;
        while ok {
            let length = entry
                .szModule
                .iter()
                .position(|item| *item == 0)
                .unwrap_or(entry.szModule.len());
            let name = String::from_utf16_lossy(&entry.szModule[..length]);
            if name.eq_ignore_ascii_case(target) {
                result = Some(entry.modBaseAddr as usize);
                break;
            }
            ok = unsafe { Module32NextW(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        Ok(result)
    }

    fn read_byte(process: *mut c_void, address: usize) -> Result<u8> {
        let mut value = 0u8;
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                process,
                address as *const c_void,
                &mut value as *mut u8 as *mut c_void,
                1,
                &mut read,
            )
        };
        if ok == 0 || read != 1 {
            bail!("无法读取断点地址 0x{address:X}");
        }
        Ok(value)
    }

    fn write_byte(process: *mut c_void, address: usize, value: u8) -> Result<()> {
        let mut written = 0usize;
        let ok = unsafe {
            WriteProcessMemory(
                process,
                address as *const c_void,
                &value as *const u8 as *const c_void,
                1,
                &mut written,
            )
        };
        if ok == 0 || written != 1 {
            bail!("无法写入断点地址 0x{address:X}");
        }
        unsafe {
            FlushInstructionCache(process, address as *const c_void, 1);
        }
        Ok(())
    }

    fn thread_context(thread_id: u32) -> Result<CONTEXT> {
        let thread = unsafe { OpenThread(THREAD_ALL_ACCESS, 0, thread_id) };
        if thread.is_null() {
            bail!("无法打开 QQ 线程 {thread_id}");
        }
        let mut context = CONTEXT {
            ContextFlags: CONTEXT_ALL_AMD64,
            ..Default::default()
        };
        let ok = unsafe { GetThreadContext(thread, &mut context) };
        unsafe { CloseHandle(thread) };
        if ok == 0 {
            bail!("无法读取 QQ 线程上下文");
        }
        Ok(context)
    }

    fn set_thread_context(thread_id: u32, context: &CONTEXT) -> Result<()> {
        let thread = unsafe { OpenThread(THREAD_ALL_ACCESS, 0, thread_id) };
        if thread.is_null() {
            bail!("无法打开 QQ 线程 {thread_id}");
        }
        let ok = unsafe { SetThreadContext(thread, context) };
        unsafe { CloseHandle(thread) };
        if ok == 0 {
            bail!("无法更新 QQ 线程上下文");
        }
        Ok(())
    }

    fn clear_single_step(thread_id: u32) -> Result<()> {
        let mut context = thread_context(thread_id)?;
        context.EFlags &= !0x100;
        set_thread_context(thread_id, &context)
    }

    fn read_key_at(process: *mut c_void, address: usize) -> Option<String> {
        if address == 0 {
            return None;
        }
        let mut buffer = [0u8; 256];
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                process,
                address as *const c_void,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len(),
                &mut read,
            )
        };
        if ok == 0 || read == 0 {
            return None;
        }
        let end = buffer[..read]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(read);
        if end != 16 || !buffer[..16].iter().all(|byte| (32..=126).contains(byte)) {
            return None;
        }
        String::from_utf8(buffer[..16].to_vec()).ok()
    }
}
