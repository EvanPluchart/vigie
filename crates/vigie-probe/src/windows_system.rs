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

use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfEntry2, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE};

use crate::{CoreTimes, DiskCounters, MemoryInfo, NetCounters, ProbeError, Result};

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

/// Bytes received and sent by the physical network interfaces that are up. Filter drivers and
/// virtual adapters (VPN, Hyper-V, WSL) report the same traffic again, so they are left out.
///
/// Listing every interface (`GetIfTable2`) costs about 1 ms, mostly for virtual ones, so the
/// list of physical interfaces is refreshed every `RELIST_EVERY` reads and only those are
/// queried in between (`GetIfEntry2`).
#[derive(Default)]
pub(crate) struct Network {
    interfaces: Vec<u32>,
    reads_since_list: u32,
}

impl Network {
    const RELIST_EVERY: u32 = 30;

    pub(crate) fn read(&mut self) -> Option<NetCounters> {
        if self.reads_since_list == 0 && !self.list() {
            return None;
        }
        self.reads_since_list = (self.reads_since_list + 1) % Self::RELIST_EVERY;
        let mut counters = NetCounters::default();
        for &index in &self.interfaces {
            // SAFETY: an all-zero row with its index set is a valid query.
            let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
            row.InterfaceIndex = index;
            // SAFETY: `row` is valid for writes. An interface that vanished just fails.
            if unsafe { GetIfEntry2(&mut row) } == 0 && row.OperStatus == IfOperStatusUp {
                counters.received += row.InOctets;
                counters.sent += row.OutOctets;
            }
        }
        Some(counters)
    }

    /// Finds the physical interfaces. False if the system refuses to list them.
    fn list(&mut self) -> bool {
        // Bits of MIB_IF_ROW2.InterfaceAndOperStatusFlags, in header order.
        const HARDWARE_INTERFACE: u8 = 1 << 0;
        const FILTER_INTERFACE: u8 = 1 << 1;
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: GetIfTable2 allocates the table; it is freed with FreeMibTable below.
        if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
            return false;
        }
        // SAFETY: on success `table` points at NumEntries rows laid out after the count.
        let rows = unsafe {
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize)
        };
        self.interfaces.clear();
        self.interfaces.extend(
            rows.iter()
                .filter(|row| {
                    let flags = row.InterfaceAndOperStatusFlags._bitfield;
                    flags & HARDWARE_INTERFACE != 0 && flags & FILTER_INTERFACE == 0
                })
                .map(|row| row.InterfaceIndex),
        );
        // SAFETY: allocated by GetIfTable2 above, freed once.
        unsafe { FreeMibTable(table.cast()) };
        true
    }
}

/// Physical disks (`\\.\PhysicalDriveN`), opened once with no access rights: enough for the
/// performance counters, and allowed without elevation. Reopened every `RESCAN_EVERY` reads so
/// plugged and unplugged disks show up.
#[derive(Default)]
pub(crate) struct Disks {
    open: Vec<(u32, OwnedHandle)>,
    reads_since_scan: u32,
}

impl Disks {
    const RESCAN_EVERY: u32 = 60;
    /// Highest disk number tried; numbers can have gaps.
    const MAX_INDEX: u32 = 32;

    pub(crate) fn read(&mut self, out: &mut Vec<DiskCounters>) {
        if self.reads_since_scan == 0 {
            self.scan();
        }
        self.reads_since_scan = (self.reads_since_scan + 1) % Self::RESCAN_EVERY;
        out.clear();
        for (index, handle) in &self.open {
            let mut perf = DISK_PERFORMANCE::default();
            let mut returned = 0u32;
            // SAFETY: live device handle; `perf` is valid for its size; no overlapped I/O.
            let ok = unsafe {
                DeviceIoControl(
                    handle.as_raw_handle(),
                    IOCTL_DISK_PERFORMANCE,
                    std::ptr::null(),
                    0,
                    (&mut perf as *mut DISK_PERFORMANCE).cast(),
                    size_of::<DISK_PERFORMANCE>() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                continue;
            }
            let query = perf.QueryTime as u64;
            out.push(DiskCounters {
                index: *index,
                read_bytes: perf.BytesRead as u64,
                written_bytes: perf.BytesWritten as u64,
                active_ns: query.saturating_sub(perf.IdleTime as u64) * 100,
                clock_ns: query * 100,
            });
        }
    }

    fn scan(&mut self) {
        self.open.clear();
        for index in 0..Self::MAX_INDEX {
            let path: Vec<u16> = format!("\\\\.\\PhysicalDrive{index}\0")
                .encode_utf16()
                .collect();
            // SAFETY: NUL-terminated path; no access rights asked, so no I/O can happen.
            let raw = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if raw == INVALID_HANDLE_VALUE || raw.is_null() {
                continue;
            }
            // SAFETY: a fresh handle nothing else owns.
            self.open
                .push((index, unsafe { OwnedHandle::from_raw_handle(raw) }));
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
        // A machine running these tests has a network interface up (or none at all).
        let mut network = Network::default();
        if let Some(net) = network.read() {
            let later = network.read().expect("still readable");
            assert!(later.received >= net.received && later.sent >= net.sent);
        }
    }
}
