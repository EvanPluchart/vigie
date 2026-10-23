//! Native Windows probe, hybrid (decision 0004):
//!
//! * Every tick, `SystemBasicProcessInformation` lists every process (PID, parent, sequence
//!   number, name) without describing threads, which keeps the kernel call cheap.
//! * Processes we may open get a handle once; each tick reads their times and working set from it.
//! * Processes we may not open (services, protected processes, without elevation) are read from
//!   the full `SystemProcessInformation` snapshot, every `FULL_EVERY` ticks and whenever one
//!   appears. Their `measured_at_ns` tells the core when the value was taken.
//!
//! On Windows versions without the basic class, every tick takes the full snapshot.

use std::collections::{HashMap, HashSet};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Instant;

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemProcessInformation,
};
use windows_sys::Win32::Foundation::{FILETIME, HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetProcessTimes, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetShellWindow, GetWindow, GetWindowLongW,
    GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
};
use windows_sys::core::BOOL;

use crate::windows_exe;
use crate::{ExeInfo, Pid, Probe, ProbeError, ProcDetails, Result, Sample};

/// Not in the public headers: lists processes without their threads (recent Windows 11).
const SYSTEM_BASIC_PROCESS_INFORMATION: SYSTEM_INFORMATION_CLASS = 252;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const STATUS_INVALID_INFO_CLASS: i32 = 0xC000_0003_u32 as i32;
/// Ticks between two full snapshots for the processes we cannot open.
const FULL_EVERY: u32 = 5;
/// FILETIME epoch (1601-01-01) to Unix epoch, in seconds.
const FILETIME_TO_UNIX_S: u64 = 11_644_473_600;
/// Reserved for each process name, so names almost never reallocate.
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

/// `SYSTEM_BASICPROCESS_INFORMATION`, from the reverse-engineered NT headers (phnt).
#[repr(C)]
struct SystemBasicProcessInfo {
    next_entry_offset: u32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    /// Increases for every process the system creates: identifies a process across PID reuse.
    sequence_number: u64,
    image_name: UnicodeString,
}

/// A raw buffer for one system information class, kept between ticks.
struct InfoBuffer {
    /// `u64` keeps the buffer 8-byte aligned, as the entries require.
    words: Vec<u64>,
}

impl InfoBuffer {
    fn new(bytes: usize) -> Self {
        Self {
            words: vec![0; bytes / 8],
        }
    }

    /// Fills the buffer, growing it when too small. Returns the raw status on other failures.
    fn query(&mut self, class: SYSTEM_INFORMATION_CLASS) -> std::result::Result<(), i32> {
        loop {
            let bytes = (self.words.len() * 8) as u32;
            let mut needed = 0u32;
            // SAFETY: the buffer is valid for `bytes` bytes and outlives the call; `needed` is a
            // valid out pointer.
            let status = unsafe {
                NtQuerySystemInformation(class, self.words.as_mut_ptr().cast(), bytes, &mut needed)
            };
            match status {
                0 => return Ok(()),
                STATUS_INFO_LENGTH_MISMATCH => {
                    // Processes and threads come and go between calls: leave half again as much room.
                    let target = (needed as usize).max(bytes as usize) * 3 / 2;
                    self.words.resize(target.div_ceil(8), 0);
                }
                other => return Err(other),
            }
        }
    }

    /// Calls `f` on each entry of the chain the kernel wrote. `T` must be the entry type of the
    /// class last queried.
    fn for_each<T>(&self, mut f: impl FnMut(&T)) {
        let base = self.words.as_ptr().cast::<u8>();
        let end = self.words.len() * 8;
        let mut offset = 0usize;
        while offset + size_of::<T>() <= end {
            // SAFETY: the kernel wrote a chain of `T` entries into the buffer; `offset` stays
            // inside it (checked by the loop) and entries are 8-byte aligned.
            let entry = unsafe { &*base.add(offset).cast::<T>() };
            f(entry);
            // SAFETY: every entry type starts with a `u32` next-entry offset.
            let next = unsafe { *base.add(offset).cast::<u32>() };
            if next == 0 {
                break;
            }
            offset += next as usize;
        }
    }
}

fn set_name(out: &mut String, name: &UnicodeString) {
    out.clear();
    if !name.buffer.is_null() && name.length > 0 {
        // SAFETY: the kernel points `buffer` at `length` bytes of UTF-16 inside the snapshot
        // buffer, which outlives this call.
        let units =
            unsafe { std::slice::from_raw_parts(name.buffer, usize::from(name.length) / 2) };
        out.extend(
            char::decode_utf16(units.iter().copied())
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)),
        );
    }
}

fn filetime_100ns(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn unix_seconds(filetime_100ns: u64) -> u64 {
    (filetime_100ns / 10_000_000).saturating_sub(FILETIME_TO_UNIX_S)
}

/// Creation time (100 ns units), CPU time (ns) and working set (bytes) read from a handle.
fn read_handle(handle: &OwnedHandle) -> Option<(u64, u64, u64)> {
    let h = handle.as_raw_handle();
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `h` is a live process handle with query rights; the out pointers are valid.
    if unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return None;
    }
    // SAFETY: an all-zero PROCESS_MEMORY_COUNTERS is a valid value.
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: same handle; `memory` is valid for `cb` bytes.
    if unsafe { K32GetProcessMemoryInfo(h, &mut memory, memory.cb) } == 0 {
        return None;
    }
    let cpu_ns = (filetime_100ns(kernel) + filetime_100ns(user)) * 100;
    Some((
        filetime_100ns(created),
        cpu_ns,
        memory.WorkingSetSize as u64,
    ))
}

/// What the probe knows about one live process.
#[derive(Default)]
struct Entry {
    sequence: u64,
    /// `None` when the process cannot be opened: its values come from the full snapshot.
    handle: Option<OwnedHandle>,
    parent: Option<Pid>,
    start_time: u64,
    name: String,
    cpu_time_ns: u64,
    memory_bytes: u64,
    measured_at_ns: u64,
    session: Option<u32>,
    /// False until a first measurement exists.
    measured: bool,
    /// Tick counter value when last listed; older entries belong to exited processes.
    seen: u64,
}

pub struct WindowsProbe {
    basic: InfoBuffer,
    full: InfoBuffer,
    /// False on Windows versions without `SystemBasicProcessInformation`.
    basic_supported: bool,
    entries: HashMap<Pid, Entry>,
    /// Names of exited processes, reused for new ones so steady-state ticks do not allocate.
    spare_names: Vec<String>,
    tick: u64,
    ticks_since_full: u32,
    epoch: Instant,
    logical_cores: u32,
    memory_total_bytes: u64,
    self_pid: Pid,
    /// PIDs owning a visible top-level window, refreshed every tick.
    windowed: HashSet<Pid>,
    /// Per-core times for the Performance view.
    cores: crate::windows_system::Cores,
    network: crate::windows_system::Network,
    disks: crate::windows_system::Disks,
    gpus: crate::windows_gpu::Gpus,
    /// Socket tables, reused between reads (8-byte aligned).
    sockets: Vec<u64>,
    /// Loaded on the first request for who holds a file.
    restart_manager: Option<crate::windows_locks::RestartManager>,
}

impl WindowsProbe {
    pub fn new() -> Result<Self> {
        // SAFETY: no pointers involved; returns 0 only on failure.
        let logical_cores = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        // SAFETY: an all-zero MEMORYSTATUSEX with its length set is a valid argument.
        let mut memory: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        memory.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: `memory` is valid for writes; on failure the total stays 0.
        unsafe { GlobalMemoryStatusEx(&mut memory) };
        Ok(Self {
            basic: InfoBuffer::new(64 * 1024),
            full: InfoBuffer::new(1024 * 1024),
            basic_supported: true,
            entries: HashMap::with_capacity(1024),
            // A few ready names absorb processes that start before any has exited.
            spare_names: {
                let mut names = Vec::with_capacity(1024);
                names.resize_with(64, || String::with_capacity(NAME_CAPACITY));
                names
            },
            tick: 0,
            ticks_since_full: FULL_EVERY,
            epoch: Instant::now(),
            logical_cores: logical_cores.max(1),
            memory_total_bytes: memory.ullTotalPhys,
            self_pid: std::process::id(),
            windowed: HashSet::with_capacity(512),
            cores: crate::windows_system::Cores::new(logical_cores.max(1)),
            network: crate::windows_system::Network::default(),
            disks: crate::windows_system::Disks::default(),
            sockets: Vec::new(),
            restart_manager: None,
            gpus: crate::windows_gpu::Gpus::discover(),
        })
    }

    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    /// Lists processes with the basic class. Returns false if this Windows lacks it.
    fn list(&mut self, now_ns: u64) -> Result<bool> {
        match self.basic.query(SYSTEM_BASIC_PROCESS_INFORMATION) {
            Ok(()) => {}
            Err(STATUS_INVALID_INFO_CLASS) => return Ok(false),
            Err(status) => return Err(unavailable(status)),
        }
        let tick = self.tick;
        let mut need_full = false;
        let entries = &mut self.entries;
        let spare_names = &mut self.spare_names;
        self.basic.for_each(|info: &SystemBasicProcessInfo| {
            let pid = info.unique_process_id as Pid;
            // PID 0 is the System Idle Process: its "CPU time" is idle time.
            if pid == 0 {
                return;
            }
            if entries.len() == entries.capacity() {
                entries.reserve(entries.len() / 4 + 64);
                spare_names.reserve(entries.capacity().saturating_sub(spare_names.len()));
            }
            let entry = entries.entry(pid).or_default();
            if entry.seen == 0 || entry.sequence != info.sequence_number {
                // A new process, or a reused PID: start over.
                let mut name = std::mem::take(&mut entry.name);
                if name.capacity() == 0 {
                    name = spare_names
                        .pop()
                        .unwrap_or_else(|| String::with_capacity(NAME_CAPACITY));
                }
                set_name(&mut name, &info.image_name);
                let parent = info.inherited_from_unique_process_id as Pid;
                *entry = Entry {
                    sequence: info.sequence_number,
                    handle: open(pid),
                    session: session_of(pid),
                    parent: (parent != 0).then_some(parent),
                    name,
                    ..Entry::default()
                };
            }
            entry.seen = tick;
            match entry.handle.as_ref().and_then(read_handle) {
                Some((created, cpu_ns, memory)) => {
                    entry.start_time = unix_seconds(created);
                    entry.cpu_time_ns = cpu_ns;
                    entry.memory_bytes = memory;
                    entry.measured_at_ns = now_ns;
                    entry.measured = true;
                }
                // Could not be opened, or exited since it was listed: the snapshot decides.
                None => {
                    entry.handle = None;
                    need_full |= !entry.measured;
                }
            }
        });
        // `ticks_since_full` counts the light ticks since the last snapshot: one snapshot every
        // `FULL_EVERY` ticks.
        if need_full || self.ticks_since_full + 1 >= FULL_EVERY {
            self.refresh_unopened(now_ns)?;
        } else {
            self.ticks_since_full += 1;
        }
        Ok(true)
    }

    /// Reads the processes without a handle from one full snapshot.
    fn refresh_unopened(&mut self, now_ns: u64) -> Result<()> {
        self.full
            .query(SystemProcessInformation)
            .map_err(unavailable)?;
        let tick = self.tick;
        let entries = &mut self.entries;
        let mut first_values = false;
        self.full.for_each(|info: &SystemProcessInfo| {
            let pid = info.unique_process_id as Pid;
            let Some(entry) = entries.get_mut(&pid) else {
                return;
            };
            if entry.seen == tick && entry.session.is_none() {
                entry.session = Some(info.session_id);
            }
            if entry.handle.is_some() || entry.seen != tick {
                return;
            }
            first_values |= !entry.measured;
            entry.start_time = unix_seconds(info.create_time as u64);
            entry.cpu_time_ns = (info.user_time as u64 + info.kernel_time as u64) * 100;
            entry.memory_bytes = info.working_set_size as u64;
            entry.measured_at_ns = now_ns;
            entry.measured = true;
        });
        // A first value gives no CPU % yet: take the next snapshot on the next tick, so new
        // services show a value after one second instead of five.
        self.ticks_since_full = if first_values { FULL_EVERY } else { 0 };
        Ok(())
    }

    /// Fallback without the basic class: everything from one full snapshot per tick.
    fn sample_full(&mut self, out: &mut Sample) -> Result<()> {
        let windowed = &self.windowed;
        self.full
            .query(SystemProcessInformation)
            .map_err(unavailable)?;
        let now_ns = self.now_ns();
        let mut len = 0usize;
        self.full.for_each(|info: &SystemProcessInfo| {
            let pid = info.unique_process_id as Pid;
            if pid == 0 {
                return;
            }
            let slot = out.slot(len, NAME_CAPACITY);
            slot.pid = pid;
            let parent = info.inherited_from_unique_process_id as Pid;
            slot.parent = (parent != 0).then_some(parent);
            slot.start_time = unix_seconds(info.create_time as u64);
            slot.cpu_time_ns = (info.user_time as u64 + info.kernel_time as u64) * 100;
            slot.memory_bytes = info.working_set_size as u64;
            slot.measured_at_ns = now_ns;
            slot.session = Some(info.session_id);
            slot.has_window = windowed.contains(&pid);
            set_name(&mut slot.name, &info.image_name);
            len += 1;
        });
        out.at_ns = now_ns;
        out.finish(len);
        Ok(())
    }
}

/// Opens a process for queries only. Fails for services and protected processes without
/// elevation, which is expected.
fn open(pid: Pid) -> Option<OwnedHandle> {
    // SAFETY: plain call; a null return means failure and owns nothing.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is a fresh handle that nothing else owns; OwnedHandle closes it on drop.
    Some(unsafe { OwnedHandle::from_raw_handle(raw) })
}

/// Login session of a process; `None` when Windows refuses (the full snapshot fills it in).
fn session_of(pid: Pid) -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: `session` is a valid out pointer.
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

/// Fills `windowed` with the PIDs that own a window the user can see, as the taskbar would
/// show it: visible, top-level, not owned, not a tool window, not cloaked, with a title.
/// Returns the shell's PID.
fn find_windows(windowed: &mut HashSet<Pid>) -> Option<Pid> {
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        // SAFETY: `param` is the `&mut HashSet<Pid>` passed to EnumWindows below, alive and
        // not otherwise borrowed for the duration of the enumeration.
        let windowed = unsafe { &mut *(param as *mut HashSet<Pid>) };
        // SAFETY: `hwnd` comes from EnumWindows; these calls only read window state.
        unsafe {
            if IsWindowVisible(hwnd) == 0
                || !GetWindow(hwnd, GW_OWNER).is_null()
                || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0
                || GetWindowTextLengthW(hwnd) == 0
            {
                return 1;
            }
            let mut cloaked = 0u32;
            let attribute = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                (&mut cloaked as *mut u32).cast(),
                size_of::<u32>() as u32,
            );
            if attribute == 0 && cloaked != 0 {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid != 0 {
                if windowed.len() == windowed.capacity() {
                    windowed.reserve(64);
                }
                windowed.insert(pid);
            }
        }
        1
    }

    windowed.clear();
    // SAFETY: `visit` matches WNDENUMPROC and only uses `param` as documented above.
    unsafe { EnumWindows(Some(visit), windowed as *mut HashSet<Pid> as LPARAM) };
    // SAFETY: plain calls; a null shell window yields no PID.
    unsafe {
        let shell = GetShellWindow();
        if shell.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, &mut pid);
        (pid != 0).then_some(pid)
    }
}

fn unavailable(status: i32) -> ProbeError {
    ProbeError::Unavailable(format!("NtQuerySystemInformation failed: {status:#x}"))
}

impl Probe for WindowsProbe {
    fn sample(&mut self, out: &mut Sample) -> Result<()> {
        out.logical_cores = self.logical_cores;
        out.memory_total_bytes = self.memory_total_bytes;
        out.self_pid = self.self_pid;
        out.shell_pid = find_windows(&mut self.windowed);
        if !self.basic_supported {
            return self.sample_full(out);
        }
        self.tick += 1;
        let now_ns = self.now_ns();
        if !self.list(now_ns)? {
            self.basic_supported = false;
            return self.sample_full(out);
        }

        // Forget exited processes, keeping their name buffers.
        let tick = self.tick;
        let spare_names = &mut self.spare_names;
        self.entries.retain(|_, entry| {
            let alive = entry.seen == tick;
            if !alive && entry.name.capacity() > 0 {
                spare_names.push(std::mem::take(&mut entry.name));
            }
            alive
        });

        out.at_ns = now_ns;
        let mut len = 0usize;
        for (&pid, entry) in &self.entries {
            if !entry.measured {
                continue;
            }
            let slot = out.slot(len, NAME_CAPACITY);
            slot.pid = pid;
            slot.parent = entry.parent;
            slot.start_time = entry.start_time;
            slot.cpu_time_ns = entry.cpu_time_ns;
            slot.memory_bytes = entry.memory_bytes;
            slot.measured_at_ns = entry.measured_at_ns;
            slot.session = entry.session;
            slot.has_window = self.windowed.contains(&pid);
            slot.name.clear();
            slot.name.push_str(&entry.name);
            len += 1;
        }
        out.finish(len);
        Ok(())
    }

    fn details(&mut self, pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()> {
        out.clear();
        for &pid in pids {
            // The handle kept for sampling, or a short-lived one (fallback mode keeps none).
            let entry = self.entries.get(&pid);
            let temporary;
            let handle = match entry.and_then(|e| e.handle.as_ref()) {
                Some(handle) => handle,
                None => {
                    temporary = open(pid);
                    match temporary.as_ref() {
                        Some(handle) => handle,
                        None => continue,
                    }
                }
            };
            out.push(ProcDetails {
                pid,
                start_time: entry.map_or(0, |e| e.start_time),
                exe: windows_exe::image_path(handle),
                // Read on demand (it means reading the other process's memory), not here.
                command_line: Vec::new(),
            });
        }
        Ok(())
    }

    fn exe_info(&mut self, exe: &std::path::Path, icon_size: u32) -> ExeInfo {
        windows_exe::exe_info(exe, icon_size)
    }

    fn sockets(&mut self, out: &mut Vec<crate::Socket>) -> Result<()> {
        crate::windows_net::sockets(&mut self.sockets, out)
    }

    fn lockers(&mut self, path: &std::path::Path, out: &mut Vec<crate::Locker>) -> Result<()> {
        if self.restart_manager.is_none() {
            self.restart_manager = Some(crate::windows_locks::RestartManager::load()?);
        }
        match &self.restart_manager {
            Some(rm) => rm.lockers(path, out),
            None => Err(ProbeError::Unsupported),
        }
    }

    fn startup_items(&mut self, out: &mut Vec<crate::StartupItem>) -> Result<()> {
        crate::windows_startup::items(out)
    }

    fn set_startup(
        &mut self,
        location: crate::StartupLocation,
        name: &str,
        enabled: bool,
    ) -> Result<()> {
        crate::windows_startup::set_enabled(location, name, enabled)
    }

    fn services(&mut self, out: &mut Vec<crate::Service>) -> Result<()> {
        crate::windows_services::services(out)
    }

    fn control_service(&mut self, name: &str, start: bool) -> Result<()> {
        crate::windows_services::control(name, start)
    }

    fn act(&mut self, target: crate::Target, action: crate::Action) -> Result<()> {
        crate::windows_actions::act(target, action)
    }

    fn system(&mut self, out: &mut crate::SystemSample) -> Result<()> {
        out.at_ns = self.now_ns();
        self.cores.read(&mut out.cores)?;
        crate::windows_system::memory(&mut out.memory);
        // Compressed pages live in the working set of the Memory Compression process, which the
        // process sampling already follows (from the full snapshot: it cannot be opened).
        out.memory.compressed = self
            .entries
            .values()
            .find(|e| e.parent == Some(4) && e.name == "Memory Compression")
            .map_or(0, |e| e.memory_bytes);
        out.uptime_s = crate::windows_system::uptime_s();
        out.network = self.network.read();
        self.disks.read(&mut out.disks);
        Ok(())
    }

    fn gpus(&mut self, out: &mut crate::SystemSample) -> Result<()> {
        self.gpus.read(&mut out.gpus);
        out.gpus_at_ns = self.now_ns();
        Ok(())
    }

    fn system_info(&mut self) -> crate::SystemInfo {
        crate::SystemInfo {
            cpu_model: crate::windows_system::cpu_model(),
            logical_cores: self.logical_cores,
            gpus: self.gpus.names(),
            memory_slots: crate::windows_system::memory_slots(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by_pid(sample: &Sample, pid: Pid) -> &crate::ProcSample {
        sample
            .processes
            .iter()
            .find(|p| p.pid == pid)
            .expect("process")
    }

    #[test]
    fn hybrid_matches_full_snapshot() {
        let mut hybrid = WindowsProbe::new().expect("probe");
        let mut full = WindowsProbe::new().expect("probe");
        full.basic_supported = false;
        let (mut a, mut b) = (Sample::default(), Sample::default());
        hybrid.sample(&mut a).expect("sample");
        full.sample(&mut b).expect("sample");

        assert!(!a.processes.iter().any(|p| p.pid == 0));
        let (own, own_full) = (by_pid(&a, a.self_pid), by_pid(&b, b.self_pid));
        assert_eq!(own.name, own_full.name);
        assert_eq!(own.start_time, own_full.start_time);
        assert_eq!(own.parent, own_full.parent);
        assert!(own.memory_bytes > 0);
        assert_eq!(a.memory_total_bytes, b.memory_total_bytes);
        assert!(a.memory_total_bytes > own.memory_bytes);
        // System (PID 4) cannot be opened without elevation: it comes from the snapshot.
        assert_eq!(by_pid(&a, 4).name, "System");
        let diff = a.processes.len().abs_diff(b.processes.len());
        assert!(diff < 10, "{} vs {}", a.processes.len(), b.processes.len());
    }

    #[test]
    fn matches_sysinfo() {
        let mut native = WindowsProbe::new().expect("probe");
        let mut reference = crate::SysinfoProbe::new().expect("probe");
        let (mut a, mut b) = (Sample::default(), Sample::default());
        native.sample(&mut a).expect("sample");
        reference.sample(&mut b).expect("sample");
        assert_eq!(a.self_pid, b.self_pid);
        let (own, own_ref) = (by_pid(&a, a.self_pid), by_pid(&b, b.self_pid));
        assert_eq!(own.name, own_ref.name);
        assert_eq!(own.start_time, own_ref.start_time);
    }

    #[test]
    fn unopened_processes_refresh_every_few_ticks() {
        let mut probe = WindowsProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        // The second tick takes another snapshot, so new services get a CPU % quickly.
        probe.sample(&mut sample).expect("sample");
        let second = by_pid(&sample, 4).measured_at_ns;
        assert_eq!(second, sample.at_ns);
        // Then opened processes are measured every tick, System only on full snapshots. A new
        // service also triggers a snapshot (decision 0004), and machines starting services
        // (CI runners) do that often: some tick before the next periodic snapshot keeps
        // System's older measurement.
        let mut kept = false;
        for _ in 0..3 {
            probe.sample(&mut sample).expect("sample");
            assert_eq!(
                by_pid(&sample, sample.self_pid).measured_at_ns,
                sample.at_ns
            );
            kept |= by_pid(&sample, 4).measured_at_ns < sample.at_ns;
        }
        assert!(kept, "System was measured on every tick");
    }
}

#[cfg(test)]
mod desktop_tests {
    use super::*;

    #[test]
    fn sessions_windows_and_shell() {
        let mut probe = WindowsProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        let system = sample
            .processes
            .iter()
            .find(|p| p.pid == 4)
            .expect("System");
        assert_eq!(system.session, Some(0));
        let own = sample
            .processes
            .iter()
            .find(|p| p.pid == sample.self_pid)
            .expect("own process");
        assert!(own.session.is_some());
        // On an interactive desktop the shell runs and owns no taskbar window of its own
        // process necessarily, but some process does.
        if let Some(shell) = sample.shell_pid {
            let shell = sample
                .processes
                .iter()
                .find(|p| p.pid == shell)
                .expect("shell");
            assert_eq!(shell.name.to_lowercase(), "explorer.exe");
            assert!(sample.processes.iter().any(|p| p.has_window));
        }
    }
}

#[cfg(test)]
mod details_tests {
    use super::*;

    #[test]
    fn own_executable_and_its_icon() {
        let mut probe = WindowsProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        let mut details = Vec::new();
        probe
            .details(&[sample.self_pid, 4], &mut details)
            .expect("details");
        // System cannot be opened without elevation: it is left out, not an error.
        let own = details
            .iter()
            .find(|d| d.pid == sample.self_pid)
            .expect("own details");
        let exe = own.exe.as_ref().expect("own executable path");
        assert_eq!(Some(exe.as_path()), std::env::current_exe().ok().as_deref());
        // Test binaries have no icon resource; the call must still answer.
        let _ = probe.exe_info(exe, 32);
    }
}

#[cfg(test)]
mod system_tests {
    use super::*;
    use crate::SystemSample;

    #[test]
    fn system_sample_after_process_sample() {
        let mut probe = WindowsProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        let mut system = SystemSample::default();
        probe.system(&mut system).expect("system");
        assert_eq!(system.cores.len() as u32, sample.logical_cores);
        assert_eq!(system.memory.total, sample.memory_total_bytes);
        assert!(system.uptime_s > 0);
        let info = probe.system_info();
        assert_eq!(info.logical_cores, sample.logical_cores);
        assert!(!info.cpu_model.is_empty());
        // The machine running the tests boots from a disk Windows can count.
        assert!(!system.disks.is_empty(), "no physical disk readable");
        assert!(system.disks.iter().all(|d| d.active_ns <= d.clock_ns));
        assert!(
            system.disks.iter().any(|d| d.letters & (1 << 2) != 0),
            "the system drive C: sits on a listed disk"
        );
    }
}
