//! Machine-wide figures on Windows, for the Performance view.

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessorPerformanceInformation,
};
use windows_sys::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::{
    GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION;

use crate::{CoreTimes, MemoryInfo, ProbeError, Result};

/// Per-core times, read into a buffer kept between ticks.
#[derive(Default)]
pub(crate) struct Cores {
    buffer: Vec<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>,
}

impl Cores {
    pub(crate) fn new(logical_cores: u32) -> Self {
        Self {
            buffer: vec![
                SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION::default();
                logical_cores as usize
            ],
        }
    }

    /// Fills `out` with one entry per core. `KernelTime` includes the idle time, so the busy
    /// time is kernel + user − idle (checked by a test).
    pub(crate) fn read(&mut self, out: &mut Vec<CoreTimes>) -> Result<()> {
        let bytes =
            (self.buffer.len() * size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>()) as u32;
        let mut written = 0u32;
        // SAFETY: the buffer is valid for `bytes` bytes of correctly aligned entries.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessorPerformanceInformation,
                self.buffer.as_mut_ptr().cast(),
                bytes,
                &mut written,
            )
        };
        if status != 0 {
            return Err(ProbeError::Failed(format!(
                "processor times: NT status {status:#x}"
            )));
        }
        let count = written as usize / size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>();
        out.clear();
        out.extend(self.buffer[..count].iter().map(|core| {
            let (idle, kernel, user) = (
                core.IdleTime as u64,
                core.KernelTime as u64,
                core.UserTime as u64,
            );
            let total = kernel + user;
            CoreTimes {
                busy_ns: total.saturating_sub(idle) * 100,
                total_ns: total * 100,
            }
        }));
        Ok(())
    }
}

/// Physical memory, cache and commit. `compressed` is left to the caller, who knows the
/// Memory Compression process.
pub(crate) fn memory(out: &mut MemoryInfo) {
    // SAFETY: zeroed structures with their size set are valid arguments; on failure the fields
    // stay 0.
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) != 0 {
            out.total = status.ullTotalPhys;
            out.available = status.ullAvailPhys;
        }
        let mut info: PERFORMANCE_INFORMATION = std::mem::zeroed();
        info.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
        if K32GetPerformanceInfo(&mut info, info.cb) != 0 {
            let page = info.PageSize as u64;
            out.cached = info.SystemCache as u64 * page;
            out.committed = info.CommitTotal as u64 * page;
            out.commit_limit = info.CommitLimit as u64 * page;
        }
    }
}

pub(crate) fn uptime_s() -> u64 {
    // SAFETY: plain call.
    unsafe { GetTickCount64() / 1000 }
}

/// The processor's marketing name, as Windows records it.
pub(crate) fn cpu_model() -> String {
    let key: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "ProcessorNameString\0".encode_utf16().collect();
    let mut buffer = [0u16; 256];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: NUL-terminated names; `buffer` is valid for `bytes` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return String::new();
    }
    let units = &buffer[..(bytes as usize / 2).min(buffer.len())];
    String::from_utf16_lossy(units)
        .trim_end_matches('\0')
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_times_are_consistent() {
        // SAFETY: plain call.
        let logical = unsafe {
            windows_sys::Win32::System::Threading::GetActiveProcessorCount(
                windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
            )
        };
        let mut cores = Cores::new(logical);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        cores.read(&mut a).expect("first read");
        std::thread::sleep(std::time::Duration::from_millis(200));
        cores.read(&mut b).expect("second read");
        // On machines with more than 64 logical processors this class may only return the
        // calling thread's processor group: this test tells.
        assert_eq!(a.len(), logical as usize, "one entry per logical processor");
        for (before, after) in a.iter().zip(&b) {
            let busy = after.busy_ns - before.busy_ns;
            let total = after.total_ns - before.total_ns;
            assert!(
                busy <= total,
                "idle is part of kernel time: busy {busy} total {total}"
            );
            // About 200 ms per core passed.
            assert!(total > 100_000_000 && total < 1_000_000_000, "{total}");
        }
    }

    #[test]
    fn memory_and_machine_facts() {
        let mut info = MemoryInfo::default();
        memory(&mut info);
        assert!(info.total > 0 && info.available <= info.total);
        assert!(info.committed > 0 && info.committed <= info.commit_limit);
        assert!(uptime_s() > 0);
        assert!(!cpu_model().is_empty());
    }
}
