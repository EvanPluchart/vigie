//! Actions on processes (Windows). Every action reopens the process with the rights it needs and
//! checks its creation time first, so a PID reused since the user clicked is never touched.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, FILETIME, GetLastError, GlobalFree, HANDLE,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetProcessTimes, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess, PROCESS_ACCESS_RIGHTS,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_SUSPEND_RESUME,
    PROCESS_TERMINATE, SetPriorityClass, TerminateProcess,
};

use crate::windows_exe;
use crate::{Action, Priority, ProbeError, Result, Target};

// Not in windows-sys: the documented-by-usage NT calls behind every "suspend process" tool.
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

/// FILETIME epoch (1601-01-01) to Unix epoch, in seconds.
const FILETIME_TO_UNIX_S: u64 = 11_644_473_600;

pub(crate) fn act(target: Target, action: Action) -> Result<()> {
    match action {
        Action::End => {
            let handle = open_verified(target, PROCESS_TERMINATE)?;
            // SAFETY: live handle with terminate rights.
            check(unsafe { TerminateProcess(handle.as_raw_handle(), 1) } != 0)
        }
        Action::Suspend => {
            let handle = open_verified(target, PROCESS_SUSPEND_RESUME)?;
            // SAFETY: live handle with suspend rights.
            nt(unsafe { NtSuspendProcess(handle.as_raw_handle()) })
        }
        Action::Resume => {
            let handle = open_verified(target, PROCESS_SUSPEND_RESUME)?;
            // SAFETY: live handle with resume rights.
            nt(unsafe { NtResumeProcess(handle.as_raw_handle()) })
        }
        Action::SetPriority(priority) => {
            let class = match priority {
                Priority::Idle => IDLE_PRIORITY_CLASS,
                Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
                Priority::Normal => NORMAL_PRIORITY_CLASS,
                Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
                Priority::High => HIGH_PRIORITY_CLASS,
            };
            let handle = open_verified(target, PROCESS_SET_INFORMATION)?;
            // SAFETY: live handle with set-information rights.
            check(unsafe { SetPriorityClass(handle.as_raw_handle(), class) } != 0)
        }
        Action::RevealFile => {
            let handle = open_verified(target, 0)?;
            let exe = windows_exe::image_path(&handle).ok_or(ProbeError::Denied)?;
            // `/select,` opens the folder with the file highlighted.
            std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", exe.display()))
                .spawn()
                .map(drop)
                .map_err(|e| ProbeError::Failed(e.to_string()))
        }
        Action::CopyCommandLine => {
            let handle = open_verified(target, 0)?;
            let command_line = command_line(&handle)?;
            set_clipboard(&command_line)
        }
    }
}

/// Opens `target` with `access` (plus query rights) and checks it is still the same process.
fn open_verified(target: Target, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle> {
    // SAFETY: plain call; null means failure and owns nothing.
    let raw = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, target.pid) };
    if raw.is_null() {
        // SAFETY: reads this thread's last error, set by OpenProcess.
        return Err(match unsafe { GetLastError() } {
            ERROR_INVALID_PARAMETER => ProbeError::Gone,
            _ => ProbeError::Denied,
        });
    }
    // SAFETY: a fresh handle nothing else owns.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: live handle with query rights; valid out pointers.
    if unsafe {
        GetProcessTimes(
            handle.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(ProbeError::Denied);
    }
    let filetime = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let start_time = (filetime / 10_000_000).saturating_sub(FILETIME_TO_UNIX_S);
    if start_time != target.start_time {
        return Err(ProbeError::Gone);
    }
    Ok(handle)
}

fn check(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        // SAFETY: reads this thread's last error.
        Err(ProbeError::Failed(format!("Windows error {}", unsafe {
            GetLastError()
        })))
    }
}

fn nt(status: i32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(ProbeError::Failed(format!("NT status {status:#x}")))
    }
}

/// The command line of a process: a UNICODE_STRING followed by its characters.
fn command_line(handle: &OwnedHandle) -> Result<String> {
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    let mut buffer = vec![0u64; 4096];
    let mut needed = 0u32;
    loop {
        // SAFETY: live handle with query rights; `buffer` is valid and 8-byte aligned.
        let status = unsafe {
            NtQueryInformationProcess(
                handle.as_raw_handle(),
                ProcessCommandLineInformation,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
        match status {
            0 => break,
            // STATUS_INFO_LENGTH_MISMATCH: grow once to what the kernel asked for.
            s if s == 0xC000_0004_u32 as i32 && (needed as usize) > buffer.len() * 8 => {
                buffer.resize((needed as usize).div_ceil(8), 0);
            }
            s => return Err(ProbeError::Failed(format!("NT status {s:#x}"))),
        }
    }
    // SAFETY: on success the buffer starts with a UNICODE_STRING whose characters follow it
    // inside the same buffer.
    let text = unsafe {
        let header = &*buffer.as_ptr().cast::<UnicodeString>();
        if header.buffer.is_null() {
            return Ok(String::new());
        }
        let units = std::slice::from_raw_parts(header.buffer, usize::from(header.length) / 2);
        String::from_utf16_lossy(units)
    };
    Ok(text)
}

fn set_clipboard(text: &str) -> Result<()> {
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: standard clipboard sequence. The global memory belongs to the clipboard once
    // SetClipboardData succeeds; it is freed here otherwise.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err(ProbeError::Failed("the clipboard is busy".into()));
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if memory.is_null() {
            CloseClipboard();
            return Err(ProbeError::Failed("out of memory".into()));
        }
        let target = GlobalLock(memory).cast::<u16>();
        std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
        GlobalUnlock(memory);
        let placed = !SetClipboardData(u32::from(CF_UNICODETEXT), memory).is_null();
        if !placed {
            GlobalFree(memory);
        }
        CloseClipboard();
        if placed {
            Ok(())
        } else {
            Err(ProbeError::Failed("could not set the clipboard".into()))
        }
    }
}

/// These tests only touch processes they start themselves.
#[cfg(test)]
mod tests {
    use std::process::{Child, Command};
    use std::time::Duration;

    use super::*;
    use crate::{Probe, Sample, SystemProbe};

    /// A harmless child that would run for a minute.
    fn spawn_ping() -> Child {
        Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("ping starts")
    }

    fn target_of(child: &Child) -> Target {
        let mut probe = SystemProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        let p = sample
            .processes
            .iter()
            .find(|p| p.pid == child.id())
            .expect("child is listed");
        Target {
            pid: p.pid,
            start_time: p.start_time,
        }
    }

    #[test]
    fn ends_own_child() {
        let mut child = spawn_ping();
        let target = target_of(&child);
        act(target, Action::End).expect("end");
        let status = child.wait().expect("wait");
        assert_eq!(status.code(), Some(1));
        // While any handle (ours, a test's, Vigie's probe) keeps the exited process open, its PID
        // cannot be reused; once none does, the start time check below refuses a newcomer.
    }

    #[test]
    fn wrong_start_time_is_refused() {
        let mut child = spawn_ping();
        let mut target = target_of(&child);
        target.start_time -= 10;
        assert!(matches!(act(target, Action::End), Err(ProbeError::Gone)));
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "still running"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn suspends_resumes_and_sets_priority() {
        let mut child = spawn_ping();
        let target = target_of(&child);
        act(target, Action::Suspend).expect("suspend");
        act(target, Action::Resume).expect("resume");
        act(target, Action::SetPriority(Priority::BelowNormal)).expect("priority");
        act(target, Action::SetPriority(Priority::Normal)).expect("priority");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn reads_own_child_command_line() {
        let mut child = spawn_ping();
        // Give the child a moment to finish starting.
        std::thread::sleep(Duration::from_millis(100));
        let target = target_of(&child);
        let handle = open_verified(target, 0).expect("open");
        let text = command_line(&handle).expect("command line");
        assert!(text.contains("127.0.0.1"), "{text}");
        let _ = child.kill();
        let _ = child.wait();
    }
}
