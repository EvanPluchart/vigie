//! Replacing Task Manager (decision 0009): the `Debugger` value of taskmgr.exe under Image File
//! Execution Options. Reading needs no rights; writing runs in an elevated copy of Vigie,
//! started with `runas`.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_64KEY,
    REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};

use crate::{ReplaceError, ReplaceState};

const KEY: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";
const VALUE: &str = "Debugger";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key opened here, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// What the `Debugger` value says now.
pub(crate) fn state() -> ReplaceState {
    let mut key = std::ptr::null_mut();
    let path = wide(KEY);
    // SAFETY: read-only open of a machine key; any user may read it.
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if code != ERROR_SUCCESS {
        return ReplaceState::Off;
    }
    let key = Key(key);
    let name = wide(VALUE);
    let mut data = vec![0u16; 1024];
    let mut len = (data.len() * 2) as u32;
    let mut kind = 0u32;
    // SAFETY: buffer sized as passed.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if code != ERROR_SUCCESS || kind != REG_SZ {
        return ReplaceState::Off;
    }
    let units = &data[..(len as usize / 2)];
    let text = String::from_utf16_lossy(units.split(|&u| u == 0).next().unwrap_or(units));
    crate::state_of(&text)
}

/// Writes (`Some`) or deletes (`None`) the value. Needs administrator rights.
pub(crate) fn write(exe: Option<&Path>) -> Result<(), ReplaceError> {
    let mut key = std::ptr::null_mut();
    let path = wide(KEY);
    // SAFETY: opens (or creates) the key for writing.
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    match code {
        ERROR_SUCCESS => {}
        ERROR_ACCESS_DENIED => return Err(ReplaceError::Denied),
        code => return Err(ReplaceError::Failed(code)),
    }
    let key = Key(key);
    let name = wide(VALUE);
    let code = match exe {
        Some(exe) => {
            let value = wide(&crate::debugger_value(exe));
            // SAFETY: a REG_SZ value with its terminator.
            unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            }
        }
        // SAFETY: deletes our value; missing is fine.
        None => match unsafe { RegDeleteValueW(key.0, name.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
            code => code,
        },
    };
    match code {
        ERROR_SUCCESS => Ok(()),
        ERROR_ACCESS_DENIED => Err(ReplaceError::Denied),
        code => Err(ReplaceError::Failed(code)),
    }
}

/// Runs `exe` with `args` elevated (one UAC prompt) and waits for it. Its exit code 0 means
/// done; the user declining the prompt gives `Cancelled`.
pub(crate) fn run_elevated(exe: &Path, args: &str) -> Result<(), ReplaceError> {
    let verb = wide("runas");
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let params = wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: params.as_ptr(),
        nShow: 0,
        ..SHELLEXECUTEINFOW::default()
    };
    // SAFETY: a filled structure; the process handle is closed below.
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            // The usual reason: the user said no to the prompt.
            return Err(ReplaceError::Cancelled);
        }
        let process = info.hProcess;
        if process.is_null() {
            return Err(ReplaceError::Cancelled);
        }
        let mut code = 1u32;
        if WaitForSingleObject(process, INFINITE) == WAIT_OBJECT_0 {
            GetExitCodeProcess(process, &mut code);
        }
        CloseHandle(process);
        if code == 0 {
            Ok(())
        } else {
            Err(ReplaceError::Failed(code))
        }
    }
}

/// Where the installer puts Vigie: `%LOCALAPPDATA%\Programs\Vigie\vigie.exe`.
pub(crate) fn installed_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Programs\Vigie\vigie.exe"))
}
