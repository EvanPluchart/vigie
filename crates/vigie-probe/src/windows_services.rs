//! Services on Windows (decision 0007): the service control manager lists them with their
//! state and process; the start type takes one query per service, so the whole list is read
//! on demand only (the Services view opening, Refresh, after an action).

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DEPENDENT_SERVICES_RUNNING, ERROR_INSUFFICIENT_BUFFER,
    ERROR_MORE_DATA, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_NOT_ACTIVE, GetLastError,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW,
    OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfig2W, QueryServiceConfigW,
    SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE,
    SERVICE_AUTO_START, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONTINUE_PENDING,
    SERVICE_CONTROL_STOP, SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DEMAND_START, SERVICE_DISABLED,
    SERVICE_PAUSE_PENDING, SERVICE_PAUSED, SERVICE_QUERY_CONFIG, SERVICE_RUNNING, SERVICE_START,
    SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, SERVICE_WIN32, StartServiceW,
};

use crate::{ProbeError, Result, Service, ServiceState, StartType};

/// A service control handle, closed on drop.
struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle from OpenSCManagerW or OpenServiceW, closed once.
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager(access: u32) -> Result<Handle> {
    // SAFETY: local machine, default database.
    let handle = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    if handle.is_null() {
        return Err(last_error("opening the service manager"));
    }
    Ok(Handle(handle))
}

fn open(manager: &Handle, name: &str, access: u32) -> Result<Handle> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: valid manager handle and NUL-terminated name.
    let handle = unsafe { OpenServiceW(manager.0, wide.as_ptr(), access) };
    if handle.is_null() {
        return Err(last_error("opening the service"));
    }
    Ok(Handle(handle))
}

fn last_error(what: &str) -> ProbeError {
    // SAFETY: no arguments.
    match unsafe { GetLastError() } {
        ERROR_ACCESS_DENIED => ProbeError::Denied,
        ERROR_DEPENDENT_SERVICES_RUNNING => ProbeError::DependentsRunning,
        code => ProbeError::Failed(format!("{what}: error {code}")),
    }
}

/// A NUL-terminated UTF-16 string at `p`.
///
/// # Safety
/// `p` is null or points to a NUL-terminated string.
unsafe fn text(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the caller promises a terminator.
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

pub(crate) fn services(out: &mut Vec<Service>) -> Result<()> {
    out.clear();
    let scm = manager(SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT)?;
    let mut buffer: Vec<u64> = vec![0; 64 * 1024 / 8];
    let mut resume = 0u32;
    loop {
        let (mut needed, mut count) = (0u32, 0u32);
        // SAFETY: `buffer` holds `len * 8` bytes, 8-byte aligned.
        let ok = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut needed,
                &mut count,
                &mut resume,
                std::ptr::null(),
            )
        };
        // SAFETY: no arguments.
        let more = ok == 0 && unsafe { GetLastError() } == ERROR_MORE_DATA;
        if ok == 0 && !more {
            return Err(last_error("listing services"));
        }
        let entries = buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for i in 0..count as usize {
            // SAFETY: the call wrote `count` entries; their strings live in the same buffer.
            let entry = unsafe { &*entries.add(i) };
            let status = &entry.ServiceStatusProcess;
            let name = unsafe { text(entry.lpServiceName) };
            out.push(Service {
                display_name: unsafe { text(entry.lpDisplayName) },
                state: state(status.dwCurrentState),
                pid: (status.dwProcessId != 0).then_some(status.dwProcessId),
                start_type: start_type(&scm, &name),
                name,
            });
        }
        if !more {
            return Ok(());
        }
        if count == 0 {
            // Not even one entry fits: grow.
            buffer.resize((needed as usize).div_ceil(8) + 1024, 0);
        }
    }
}

fn state(raw: u32) -> ServiceState {
    match raw {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED => ServiceState::Stopped,
        SERVICE_START_PENDING | SERVICE_CONTINUE_PENDING => ServiceState::Starting,
        SERVICE_STOP_PENDING => ServiceState::Stopping,
        SERVICE_PAUSED | SERVICE_PAUSE_PENDING => ServiceState::Paused,
        _ => ServiceState::Other,
    }
}

/// `None` when the service cannot be queried.
fn start_type(scm: &Handle, name: &str) -> Option<StartType> {
    let service = open(scm, name, SERVICE_QUERY_CONFIG).ok()?;
    let mut buffer: Vec<u64> = vec![0; 1024];
    let mut needed = 0u32;
    // SAFETY: `buffer` is large enough for most configurations; grown once otherwise.
    let mut ok = unsafe {
        QueryServiceConfigW(
            service.0,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    // SAFETY: no arguments.
    if ok == 0 && unsafe { GetLastError() } == ERROR_INSUFFICIENT_BUFFER {
        buffer.resize((needed as usize).div_ceil(8), 0);
        // SAFETY: as above, with the size asked for.
        ok = unsafe {
            QueryServiceConfigW(
                service.0,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
    }
    if ok == 0 {
        return None;
    }
    // SAFETY: the call filled a QUERY_SERVICE_CONFIGW at the start of the buffer.
    let config = unsafe { &*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
    Some(match config.dwStartType {
        SERVICE_AUTO_START => {
            let mut delayed = SERVICE_DELAYED_AUTO_START_INFO::default();
            // SAFETY: the structure is the documented buffer for this level.
            let ok = unsafe {
                QueryServiceConfig2W(
                    service.0,
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    (&mut delayed as *mut SERVICE_DELAYED_AUTO_START_INFO).cast(),
                    std::mem::size_of::<SERVICE_DELAYED_AUTO_START_INFO>() as u32,
                    &mut needed,
                )
            };
            if ok != 0 && delayed.fDelayedAutostart != 0 {
                StartType::AutomaticDelayed
            } else {
                StartType::Automatic
            }
        }
        SERVICE_DEMAND_START => StartType::Manual,
        SERVICE_DISABLED => StartType::Disabled,
        // Boot and system starts are for drivers, not listed here.
        _ => StartType::Automatic,
    })
}

/// Starts or stops a service. Most services need Vigie to run as administrator.
pub(crate) fn control(name: &str, start: bool) -> Result<()> {
    let scm = manager(SC_MANAGER_CONNECT)?;
    let service = open(&scm, name, if start { SERVICE_START } else { SERVICE_STOP })?;
    let ok = if start {
        // SAFETY: valid handle, no arguments.
        unsafe { StartServiceW(service.0, 0, std::ptr::null()) }
    } else {
        let mut status = SERVICE_STATUS::default();
        // SAFETY: valid handle; `status` receives the latest state.
        unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) }
    };
    if ok != 0 {
        return Ok(());
    }
    // SAFETY: no arguments.
    match unsafe { GetLastError() } {
        // Already in the asked state: nothing to do.
        ERROR_SERVICE_ALREADY_RUNNING | ERROR_SERVICE_NOT_ACTIVE => Ok(()),
        ERROR_ACCESS_DENIED => Err(ProbeError::Denied),
        ERROR_DEPENDENT_SERVICES_RUNNING => Err(ProbeError::DependentsRunning),
        code => Err(ProbeError::Failed(format!(
            "controlling the service: error {code}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read only: never starts or stops anything.
    #[test]
    fn lists_services_with_states_and_start_types() {
        let started = std::time::Instant::now();
        let mut out = Vec::new();
        services(&mut out).unwrap();
        eprintln!("{} services in {:?}", out.len(), started.elapsed());
        assert!(out.len() > 20, "{}", out.len());
        // The event log runs on every Windows machine, automatically, in a process.
        let log = out
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("EventLog"))
            .expect("EventLog");
        assert_eq!(log.state, ServiceState::Running);
        assert!(log.pid.is_some());
        assert!(matches!(log.start_type, Some(StartType::Automatic)));
        assert!(!log.display_name.is_empty());
    }
}
