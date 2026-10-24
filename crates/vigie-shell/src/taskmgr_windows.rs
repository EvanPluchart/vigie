//! Replacing Task Manager (decisions 0009 and 0011): the `Debugger` value of taskmgr.exe under
//! Image File Execution Options. Reading needs no rights; writing runs in an elevated copy of
//! Vigie, started with `runas`.
//!
//! The value points at a copy of Vigie in Program Files, which only administrators can change:
//! Windows starts it with administrator rights and without a prompt, so it must not be a file
//! any program of the user could replace (the per-user install is).

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
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHGetKnownFolderPath,
    ShellExecuteExW,
};

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

/// The copy Ctrl+Shift+Esc starts: `Program Files\Vigie\vigie.exe`. The folder comes from
/// Windows itself, not from `%ProgramFiles%`, which a user can redefine for their account.
pub(crate) fn machine_path() -> Option<PathBuf> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: a known folder id; the string Windows allocates is freed below.
    let found = unsafe {
        SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, std::ptr::null_mut(), &mut raw)
    } == 0;
    let folder = if found && !raw.is_null() {
        // SAFETY: a NUL-terminated string from Windows.
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        // SAFETY: `len` units are readable at `raw`.
        let units = unsafe { std::slice::from_raw_parts(raw, len) };
        Some(PathBuf::from(String::from_utf16_lossy(units)))
    } else {
        None
    };
    // SAFETY: freeing what SHGetKnownFolderPath allocated (null is allowed).
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(folder?.join(r"Vigie\vigie.exe"))
}

/// In the elevated helper: puts `source` at `target`. A copy that is running (a Vigie opened
/// with Ctrl+Shift+Esc) cannot be overwritten but can be renamed: it is moved aside, and
/// deleted on a later run.
pub(crate) fn install_copy(source: &Path, target: &Path) -> Result<(), ReplaceError> {
    let failed = |e: std::io::Error| ReplaceError::Failed(e.raw_os_error().unwrap_or(0) as u32);
    let folder = target.parent().ok_or(ReplaceError::Failed(0))?;
    std::fs::create_dir_all(folder).map_err(failed)?;
    remove_set_aside(folder);
    let fresh = folder.join("vigie.exe.new");
    std::fs::copy(source, &fresh).map_err(failed)?;
    let mut aside = None;
    if target.exists() && std::fs::remove_file(target).is_err() {
        let path = folder.join(format!("vigie.old-{}.exe", std::process::id()));
        std::fs::rename(target, &path).map_err(failed)?;
        aside = Some(path);
    }
    std::fs::rename(&fresh, target).map_err(|e| {
        // The value may already name `target`: put the previous copy back rather than leave
        // Ctrl+Shift+Esc pointing at nothing.
        if let Some(aside) = &aside {
            let _ = std::fs::rename(aside, target);
        }
        let _ = std::fs::remove_file(&fresh);
        failed(e)
    })
}

/// In the elevated helper: removes the copy, and its folder once empty. A running copy is
/// moved aside and goes on a later run, or stays harmless: nothing points at it.
pub(crate) fn remove_copy(target: &Path) {
    let Some(folder) = target.parent() else {
        return;
    };
    if std::fs::remove_file(target).is_err() && target.exists() {
        let _ = std::fs::rename(target, folder.join(format!("vigie.old-{}.exe", std::process::id())));
    }
    remove_set_aside(folder);
    let _ = std::fs::remove_dir(folder);
}

/// Deletes copies set aside earlier, those no longer running.
fn remove_set_aside(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if (name.starts_with("vigie.old-") && name.ends_with(".exe")) || name == "vigie.exe.new" {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Where the installer puts Vigie: `%LOCALAPPDATA%\Programs\Vigie\vigie.exe`.
pub(crate) fn installed_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Programs\Vigie\vigie.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_lives_in_program_files() {
        let copy = machine_path().expect("Program Files is known");
        let text = copy.to_string_lossy().to_lowercase();
        assert!(text.ends_with(r"\vigie\vigie.exe"), "{text}");
        assert!(!text.contains("appdata"), "{text}");
    }

    /// In a temporary folder, not Program Files: the copy is made, replaced while "running"
    /// (open), and removed with its folder.
    #[test]
    fn copies_are_made_replaced_and_removed() {
        let base = std::env::temp_dir().join(format!("vigie-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let source = base.join("source.exe");
        std::fs::write(&source, b"first").unwrap();
        let target = base.join(r"Vigie\vigie.exe");
        install_copy(&source, &target).expect("copied");
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        std::fs::write(&source, b"second").unwrap();
        install_copy(&source, &target).expect("replaced");
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        remove_copy(&target);
        assert!(!target.exists());
        assert!(!target.parent().unwrap().exists(), "empty folder removed");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The copy keeps its source's size and modification time: `copy_outdated` relies on it.
    #[test]
    fn a_copy_keeps_size_and_time() {
        let base = std::env::temp_dir().join(format!("vigie-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let source = base.join("source.exe");
        std::fs::write(&source, vec![7u8; 4096]).unwrap();
        let target = base.join(r"Vigie\vigie.exe");
        install_copy(&source, &target).expect("copied");
        let stamp = |p: &Path| {
            let m = std::fs::metadata(p).unwrap();
            (m.len(), m.modified().unwrap())
        };
        assert_eq!(stamp(&source), stamp(&target));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A copy that is running (a Vigie opened with Ctrl+Shift+Esc) is replaced all the same.
    /// The running program is a copy of ping this test starts and ends itself.
    #[test]
    fn a_running_copy_is_replaced() {
        let base = std::env::temp_dir().join(format!("vigie-running-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join(r"Vigie\vigie.exe");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let windows = std::env::var_os("SystemRoot").unwrap();
        std::fs::copy(Path::new(&windows).join(r"System32\PING.EXE"), &target).unwrap();
        let mut running = std::process::Command::new(&target)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("the copy runs");
        let source = base.join("new.exe");
        std::fs::write(&source, b"new version").unwrap();
        install_copy(&source, &target).expect("replaced while running");
        assert_eq!(std::fs::read(&target).unwrap(), b"new version");
        let _ = running.kill();
        let _ = running.wait();
        remove_copy(&target);
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
