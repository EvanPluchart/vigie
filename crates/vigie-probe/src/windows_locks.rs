//! Who holds a file on Windows (decision 0007): the Restart Manager lists the processes using
//! it. rstrtmgr.dll is loaded on the first request, so it costs nothing until then.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmConsole as RM_CONSOLE, RmCritical as RM_CRITICAL,
    RmExplorer as RM_EXPLORER, RmMainWindow as RM_MAIN_WINDOW, RmOtherWindow as RM_OTHER_WINDOW,
    RmService as RM_SERVICE,
};
use windows_sys::core::{PCWSTR, PWSTR};

use crate::{Locker, LockerKind, ProbeError, Result};

const FILETIME_TO_UNIX_S: u64 = 11_644_473_600;

type StartSession = unsafe extern "system" fn(*mut u32, u32, PWSTR) -> u32;
type RegisterResources =
    unsafe extern "system" fn(u32, u32, *const PCWSTR, u32, *const u8, u32, *const PCWSTR) -> u32;
type GetList =
    unsafe extern "system" fn(u32, *mut u32, *mut u32, *mut RM_PROCESS_INFO, *mut u32) -> u32;
type EndSession = unsafe extern "system" fn(u32) -> u32;

/// The Restart Manager entry points, resolved once.
pub(crate) struct RestartManager {
    start: StartSession,
    register: RegisterResources,
    list: GetList,
    end: EndSession,
}

impl RestartManager {
    pub(crate) fn load() -> Result<Self> {
        let name: Vec<u16> = "rstrtmgr.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated name; System32 only, so no planted DLL is picked up. The
        // module is never freed: the entry points below stay valid for the process lifetime.
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return Err(ProbeError::Unavailable("rstrtmgr.dll".into()));
        }
        let symbol = |name: &[u8]| {
            // SAFETY: `name` is NUL-terminated; `module` is a loaded module.
            unsafe { GetProcAddress(module, name.as_ptr()) }
                .ok_or_else(|| ProbeError::Unavailable("Restart Manager".into()))
        };
        // SAFETY: the signatures match the documented Restart Manager functions.
        unsafe {
            Ok(Self {
                start: std::mem::transmute::<_, StartSession>(symbol(b"RmStartSession\0")?),
                register: std::mem::transmute::<_, RegisterResources>(symbol(
                    b"RmRegisterResources\0",
                )?),
                list: std::mem::transmute::<_, GetList>(symbol(b"RmGetList\0")?),
                end: std::mem::transmute::<_, EndSession>(symbol(b"RmEndSession\0")?),
            })
        }
    }

    /// Processes that have `path` open. Files only: the Restart Manager does not track folders.
    pub(crate) fn lockers(&self, path: &Path, out: &mut Vec<Locker>) -> Result<()> {
        out.clear();
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => return Err(ProbeError::IsFolder),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ProbeError::NotFound),
            // Unreadable metadata (no rights on the folder): the Restart Manager may still know.
            Err(_) => {}
        }
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        let mut session = 0u32;
        // SAFETY: `key` has room for the session key and its NUL.
        let code = unsafe { (self.start)(&mut session, 0, key.as_mut_ptr()) };
        if code != ERROR_SUCCESS {
            return Err(ProbeError::Failed(format!(
                "starting a Restart Manager session: {code}"
            )));
        }
        // Sessions are a small per-user resource: always end this one.
        let _end = Ended(self.end, session);
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let files = [wide.as_ptr()];
        // SAFETY: one valid NUL-terminated path; no applications or services.
        let code = unsafe {
            (self.register)(
                session,
                1,
                files.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        if code != ERROR_SUCCESS {
            return Err(ProbeError::Failed(format!("registering the file: {code}")));
        }
        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        // The list can grow between calls: retry while it does.
        for _ in 0..4 {
            let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
            // SAFETY: `infos` has room for `count` entries.
            let code = unsafe {
                (self.list)(
                    session,
                    &mut needed,
                    &mut count,
                    infos.as_mut_ptr(),
                    &mut reasons,
                )
            };
            match code {
                ERROR_SUCCESS => {
                    out.extend(infos.iter().take(count as usize).map(locker));
                    return Ok(());
                }
                ERROR_MORE_DATA => infos.resize(needed as usize + 4, RM_PROCESS_INFO::default()),
                code => return Err(ProbeError::Failed(format!("listing the processes: {code}"))),
            }
        }
        Err(ProbeError::Failed(
            "the list of processes kept growing".into(),
        ))
    }
}

struct Ended(EndSession, u32);

impl Drop for Ended {
    fn drop(&mut self) {
        // SAFETY: a session started by `lockers`, ended once.
        unsafe { (self.0)(self.1) };
    }
}

fn locker(info: &RM_PROCESS_INFO) -> Locker {
    let text = |chars: &[u16]| {
        let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
        String::from_utf16_lossy(&chars[..end])
    };
    let service = text(&info.strServiceShortName);
    Locker {
        pid: info.Process.dwProcessId,
        start_time: unix_seconds(info.Process.ProcessStartTime),
        name: text(&info.strAppName),
        service: (!service.is_empty()).then_some(service),
        kind: match info.ApplicationType {
            RM_MAIN_WINDOW | RM_OTHER_WINDOW => LockerKind::App,
            RM_SERVICE => LockerKind::Service,
            RM_EXPLORER => LockerKind::Explorer,
            RM_CONSOLE => LockerKind::Console,
            RM_CRITICAL => LockerKind::Critical,
            _ => LockerKind::Other,
        },
    }
}

fn unix_seconds(t: FILETIME) -> u64 {
    let ticks = (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    (ticks / 10_000_000).saturating_sub(FILETIME_TO_UNIX_S)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Probe, Sample};

    #[test]
    fn finds_ourselves_holding_a_file_with_our_start_time() {
        let dir = std::env::temp_dir().join(format!("vigie-locks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("held.txt");
        let file = std::fs::File::create(&path).unwrap();
        let rm = RestartManager::load().unwrap();
        let mut out = Vec::new();
        rm.lockers(&path, &mut out).unwrap();
        let me = std::process::id();
        let ours = out.iter().find(|l| l.pid == me).expect("we hold the file");
        // The identity check of actions needs the same start time as the process probe.
        let mut probe = crate::SystemProbe::new().unwrap();
        let mut sample = Sample::default();
        probe.sample(&mut sample).unwrap();
        let start = sample
            .processes
            .iter()
            .find(|p| p.pid == me)
            .unwrap()
            .start_time;
        assert_eq!(ours.start_time, start);
        assert!(matches!(
            rm.lockers(&dir, &mut out),
            Err(ProbeError::IsFolder)
        ));
        drop(file);
        rm.lockers(&path, &mut out).unwrap();
        assert!(out.iter().all(|l| l.pid != me), "released");
        assert!(matches!(
            rm.lockers(&dir.join("missing.txt"), &mut out),
            Err(ProbeError::NotFound)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
