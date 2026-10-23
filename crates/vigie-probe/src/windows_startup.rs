//! Programs that start with the session on Windows (decision 0007): the Run keys and the
//! startup folders, with the enabled state Explorer keeps under `StartupApproved`.
//!
//! An approval value is 12 bytes: the first is even when the item is enabled (2, or 6 for some
//! system items) and odd when it is disabled (3), followed by the time it was disabled. An item
//! without a value is enabled. Read on demand, when the Startup view opens.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SUCCESS, FILETIME};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY,
    REG_BINARY, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

use crate::{ProbeError, Result, StartupItem, StartupLocation};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_WOW64: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Key {
    fn open(root: HKEY, path: &str, access: u32) -> Option<Key> {
        let mut key = std::ptr::null_mut();
        let path = wide(path);
        // SAFETY: valid root and NUL-terminated path; `key` receives the handle.
        let code =
            unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, access | KEY_WOW64_64KEY, &mut key) };
        (code == ERROR_SUCCESS).then_some(Key(key))
    }

    /// Values as (name, type, data), in registry order.
    fn values(&self) -> Vec<(String, u32, Vec<u8>)> {
        let mut out = Vec::new();
        let mut name = vec![0u16; 16_384];
        let mut data = vec![0u8; 4096];
        for index in 0.. {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut kind = 0u32;
            // SAFETY: buffers sized as passed.
            let code = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if code != ERROR_SUCCESS {
                // ERROR_NO_MORE_ITEMS, or a value too large to be a command: stop or skip.
                if code == windows_sys::Win32::Foundation::ERROR_MORE_DATA {
                    continue;
                }
                break;
            }
            out.push((
                String::from_utf16_lossy(&name[..name_len as usize]),
                kind,
                data[..data_len as usize].to_vec(),
            ));
        }
        out
    }

    fn binary(&self, name: &str) -> Option<Vec<u8>> {
        let name = wide(name);
        let mut data = [0u8; 64];
        let mut len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: buffers sized as passed.
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr(),
                &mut len,
            )
        };
        (code == ERROR_SUCCESS && kind == REG_BINARY).then(|| data[..len as usize].to_vec())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key opened by `Key::open` or `approval_key`, closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// Where Explorer keeps the enabled state of items from `location`.
fn approval(location: StartupLocation) -> (HKEY, String) {
    match location {
        StartupLocation::UserRun => (HKEY_CURRENT_USER, format!(r"{APPROVED}\Run")),
        StartupLocation::MachineRun => (HKEY_LOCAL_MACHINE, format!(r"{APPROVED}\Run")),
        StartupLocation::MachineRun32 => (HKEY_LOCAL_MACHINE, format!(r"{APPROVED}\Run32")),
        StartupLocation::UserFolder => (HKEY_CURRENT_USER, format!(r"{APPROVED}\StartupFolder")),
        StartupLocation::CommonFolder => (HKEY_LOCAL_MACHINE, format!(r"{APPROVED}\StartupFolder")),
    }
}

/// Enabled unless the approval value says disabled (odd first byte).
fn enabled(approved: Option<&Key>, name: &str) -> bool {
    approved
        .and_then(|k| k.binary(name))
        .and_then(|v| v.first().copied())
        .is_none_or(|b| b % 2 == 0)
}

fn utf16_string(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Expands `%NAME%` from the environment; unknown names stay as they are.
fn expand(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) if !name.is_empty() => out.push_str(&value),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The program a command line starts: the quoted part, or up to ".exe", or the first word.
pub(crate) fn command_exe(command: &str) -> Option<PathBuf> {
    let command = expand(command.trim());
    let exe = if let Some(rest) = command.strip_prefix('"') {
        rest.split('"').next()?.to_owned()
    } else if let Some(at) = command.to_ascii_lowercase().find(".exe") {
        command[..at + 4].to_owned()
    } else {
        command.split_whitespace().next()?.to_owned()
    };
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

fn startup_folder(location: StartupLocation) -> Option<PathBuf> {
    let (var, tail) = match location {
        StartupLocation::UserFolder => {
            ("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup")
        }
        StartupLocation::CommonFolder => (
            "ProgramData",
            r"Microsoft\Windows\Start Menu\Programs\StartUp",
        ),
        _ => return None,
    };
    Some(Path::new(&std::env::var_os(var)?).join(tail))
}

pub(crate) fn items(out: &mut Vec<StartupItem>) -> Result<()> {
    out.clear();
    for (location, root, path) in [
        (StartupLocation::UserRun, HKEY_CURRENT_USER, RUN),
        (StartupLocation::MachineRun, HKEY_LOCAL_MACHINE, RUN),
        (StartupLocation::MachineRun32, HKEY_LOCAL_MACHINE, RUN_WOW64),
    ] {
        let Some(run) = Key::open(root, path, KEY_READ) else {
            continue;
        };
        let (approved_root, approved_path) = approval(location);
        let approved = Key::open(approved_root, &approved_path, KEY_READ);
        for (name, kind, data) in run.values() {
            if kind != REG_SZ && kind != REG_EXPAND_SZ {
                continue;
            }
            let command = utf16_string(&data);
            out.push(StartupItem {
                enabled: enabled(approved.as_ref(), &name),
                exe: command_exe(&command),
                name,
                command,
                location,
            });
        }
    }
    for location in [StartupLocation::UserFolder, StartupLocation::CommonFolder] {
        let Some(folder) = startup_folder(location) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let (approved_root, approved_path) = approval(location);
        let approved = Key::open(approved_root, &approved_path, KEY_READ);
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case("desktop.ini") || entry.path().is_dir() {
                continue;
            }
            let path = entry.path();
            let is_exe = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
            out.push(StartupItem {
                enabled: enabled(approved.as_ref(), &name),
                command: path.to_string_lossy().into_owned(),
                // Shortcuts would need the shell to resolve; their own icon is enough.
                exe: is_exe.then_some(path),
                name,
                location,
            });
        }
    }
    Ok(())
}

/// Enables or disables an item the way Explorer and Task Manager do: through its approval
/// value. The Run entry or the shortcut itself is never touched, so this is reversible.
pub(crate) fn set_enabled(location: StartupLocation, name: &str, enabled: bool) -> Result<()> {
    let (root, path) = approval(location);
    let mut value = [0u8; 12];
    if enabled {
        value[0] = 2;
    } else {
        value[0] = 3;
        let mut now = FILETIME::default();
        // SAFETY: writes the current time into `now`.
        unsafe { GetSystemTimeAsFileTime(&mut now) };
        value[4..8].copy_from_slice(&now.dwLowDateTime.to_le_bytes());
        value[8..12].copy_from_slice(&now.dwHighDateTime.to_le_bytes());
    }
    let mut key = std::ptr::null_mut();
    let wide_path = wide(&path);
    // SAFETY: valid root and NUL-terminated path; creates the approval key if it is missing.
    let code = unsafe {
        RegCreateKeyExW(
            root,
            wide_path.as_ptr(),
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
        ERROR_ACCESS_DENIED => return Err(ProbeError::Denied),
        code => {
            return Err(ProbeError::Failed(format!(
                "opening the approval key: {code}"
            )));
        }
    }
    let key = Key(key);
    let name = wide(name);
    // SAFETY: a 12-byte binary value.
    let code = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_BINARY,
            value.as_ptr(),
            value.len() as u32,
        )
    };
    match code {
        ERROR_SUCCESS => Ok(()),
        ERROR_ACCESS_DENIED => Err(ProbeError::Denied),
        code => Err(ProbeError::Failed(format!(
            "writing the approval value: {code}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::RegDeleteValueW;

    #[test]
    fn commands_name_their_program() {
        let exe = |c: &str| command_exe(c).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(
            exe(r#""C:\Program Files\App\app.exe" --hidden"#).as_deref(),
            Some(r"C:\Program Files\App\app.exe")
        );
        assert_eq!(
            exe(r"C:\Tools\tool.EXE /min").as_deref(),
            Some(r"C:\Tools\tool.EXE")
        );
        assert_eq!(exe("rundll32 shell32.dll").as_deref(), Some("rundll32"));
        assert_eq!(exe("  ").as_deref(), None);
        // SAFETY of the test: reads an environment variable that always exists on Windows.
        let windir = std::env::var("SystemRoot").unwrap();
        assert_eq!(
            exe(r"%SystemRoot%\system32\cmd.exe /c").unwrap(),
            format!(r"{windir}\system32\cmd.exe")
        );
    }

    #[test]
    fn lists_the_run_keys_without_failing() {
        let mut out = Vec::new();
        items(&mut out).unwrap();
        // Every listed item has a name; nothing more can be assumed about this machine.
        assert!(out.iter().all(|i| !i.name.is_empty()));
    }

    /// Deletes the test's own Run value and approval value, even when the test fails.
    struct Cleanup(&'static str);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let name = wide(self.0);
            for path in [RUN.to_owned(), format!(r"{APPROVED}\Run")] {
                if let Some(key) = Key::open(HKEY_CURRENT_USER, &path, KEY_SET_VALUE) {
                    // SAFETY: deletes a value this test created.
                    unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
                }
            }
        }
    }

    /// Writes to the user's registry: a Run value of its own pointing to a harmless command,
    /// removed afterwards. Run by hand: `cargo test -p vigie-probe -- --ignored toggles`.
    #[test]
    #[ignore]
    fn toggles_an_item_of_its_own() {
        const NAME: &str = "VigieProbeTest";
        let _cleanup = Cleanup(NAME);
        let run = {
            let mut key = std::ptr::null_mut();
            let path = wide(RUN);
            // SAFETY: opens the user's Run key for writing.
            let code = unsafe {
                RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_SET_VALUE, &mut key)
            };
            assert_eq!(code, ERROR_SUCCESS);
            Key(key)
        };
        let command = wide(r#"cmd.exe /c exit"#);
        let name = wide(NAME);
        // SAFETY: a REG_SZ value of the test's own.
        let code = unsafe {
            RegSetValueExW(
                run.0,
                name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        };
        assert_eq!(code, ERROR_SUCCESS);
        let find = || {
            let mut out = Vec::new();
            items(&mut out).unwrap();
            out.into_iter().find(|i| i.name == NAME).expect("listed")
        };
        let item = find();
        assert!(item.enabled && item.location == StartupLocation::UserRun);
        set_enabled(StartupLocation::UserRun, NAME, false).unwrap();
        assert!(!find().enabled);
        set_enabled(StartupLocation::UserRun, NAME, true).unwrap();
        assert!(find().enabled);
    }
}
