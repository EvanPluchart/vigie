//! Native Windows probe: one `NtQuerySystemInformation(SystemProcessInformation)` call per tick
//! returns every process with its times and memory, without opening any process (decision 0002).

use std::time::Instant;

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
use windows_sys::Win32::System::Threading::{ALL_PROCESSOR_GROUPS, GetActiveProcessorCount};

use crate::{Pid, Probe, ProbeError, ProcDetails, Result, Sample, SysinfoProbe};

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
/// FILETIME epoch (1601-01-01) to Unix epoch, in seconds.
const FILETIME_TO_UNIX_S: u64 = 11_644_473_600;
/// Reserved for each new process name, so most names never reallocate.
const NAME_CAPACITY: usize = 128;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// `SYSTEM_PROCESS_INFORMATION` with the fields the official headers leave as `Reserved`.
/// Layout from the NT native API definitions (stable since Windows 7); `repr(C)` adds the padding.
#[repr(C)]
struct SystemProcessInfo {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
}

pub struct WindowsProbe {
    /// Raw buffer for the system call, kept between ticks. `u64` keeps it 8-byte aligned.
    buffer: Vec<u64>,
    epoch: Instant,
    logical_cores: u32,
    self_pid: Pid,
    /// Expensive per-process details stay on `sysinfo` until they are measured.
    details: SysinfoProbe,
}

impl WindowsProbe {
    pub fn new() -> Result<Self> {
        // SAFETY: no pointers involved; returns 0 only on failure.
        let logical_cores = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        Ok(Self {
            buffer: vec![0; 256 * 1024 / 8],
            epoch: Instant::now(),
            logical_cores: logical_cores.max(1),
            self_pid: std::process::id(),
            details: SysinfoProbe::new()?,
        })
    }

    /// Fills `self.buffer` with one snapshot of every process, growing it when too small.
    fn query(&mut self) -> Result<()> {
        loop {
            let bytes = (self.buffer.len() * 8) as u32;
            let mut needed = 0u32;
            // SAFETY: the buffer is valid for `bytes` bytes and outlives the call; `needed` is a
            // valid out pointer.
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buffer.as_mut_ptr().cast(),
                    bytes,
                    &mut needed,
                )
            };
            match status {
                0 => return Ok(()),
                STATUS_INFO_LENGTH_MISMATCH => {
                    // Processes may start between two calls: leave a quarter of headroom.
                    let target = (needed as usize).max(bytes as usize) * 5 / 4;
                    self.buffer.resize(target.div_ceil(8), 0);
                }
                other => {
                    return Err(ProbeError::Unavailable(format!(
                        "NtQuerySystemInformation failed: {other:#x}"
                    )));
                }
            }
        }
    }
}

impl Probe for WindowsProbe {
    fn sample(&mut self, out: &mut Sample) -> Result<()> {
        self.query()?;
        out.at_ns = self.epoch.elapsed().as_nanos() as u64;
        out.logical_cores = self.logical_cores;
        out.self_pid = self.self_pid;

        let base = self.buffer.as_ptr().cast::<u8>();
        let end = self.buffer.len() * 8;
        let mut offset = 0usize;
        let mut len = 0usize;
        loop {
            if offset + size_of::<SystemProcessInfo>() > end {
                break;
            }
            // SAFETY: the kernel wrote a chain of entries into the buffer; `offset` stays inside
            // it (checked above) and every entry is 8-byte aligned.
            let info = unsafe { &*base.add(offset).cast::<SystemProcessInfo>() };
            let pid = info.unique_process_id as Pid;
            // PID 0 is the System Idle Process: its "CPU time" is idle time.
            if pid != 0 {
                let slot = out.slot(len, NAME_CAPACITY);
                slot.pid = pid;
                let parent = info.inherited_from_unique_process_id as Pid;
                slot.parent = (parent != 0).then_some(parent);
                slot.start_time =
                    (info.create_time as u64 / 10_000_000).saturating_sub(FILETIME_TO_UNIX_S);
                slot.cpu_time_ns = (info.user_time as u64 + info.kernel_time as u64) * 100;
                slot.memory_bytes = info.working_set_size as u64;
                slot.name.clear();
                let name = &info.image_name;
                if !name.buffer.is_null() && name.length > 0 {
                    // SAFETY: the kernel points `buffer` at `length` bytes of UTF-16 inside the
                    // snapshot buffer, which is alive for this whole loop.
                    let units = unsafe {
                        std::slice::from_raw_parts(name.buffer, usize::from(name.length) / 2)
                    };
                    slot.name.extend(
                        char::decode_utf16(units.iter().copied())
                            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)),
                    );
                }
                len += 1;
            }
            if info.next_entry_offset == 0 {
                break;
            }
            offset += info.next_entry_offset as usize;
        }
        out.finish(len);
        Ok(())
    }

    fn details(&mut self, pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()> {
        self.details.details(pids, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_sysinfo() {
        let mut native = WindowsProbe::new().expect("probe");
        let mut reference = SysinfoProbe::new().expect("probe");
        let (mut a, mut b) = (Sample::default(), Sample::default());
        native.sample(&mut a).expect("sample");
        reference.sample(&mut b).expect("sample");

        assert_eq!(a.self_pid, b.self_pid);
        assert!(!a.processes.iter().any(|p| p.pid == 0));
        let own = a
            .processes
            .iter()
            .find(|p| p.pid == a.self_pid)
            .expect("own process");
        let own_ref = b
            .processes
            .iter()
            .find(|p| p.pid == b.self_pid)
            .expect("own process");
        assert_eq!(own.name, own_ref.name);
        assert_eq!(own.start_time, own_ref.start_time);
        assert_eq!(own.parent, own_ref.parent);
        // Process churn between the two calls is small.
        let diff = a.processes.len().abs_diff(b.processes.len());
        assert!(diff < 10, "{} vs {}", a.processes.len(), b.processes.len());
    }
}
