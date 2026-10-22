//! Per-OS system sampling behind one trait. No UI here. See docs/ARCHITECTURE.md.

mod sysinfo_probe;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_actions;
#[cfg(windows)]
mod windows_exe;
#[cfg(windows)]
mod windows_gpu;
#[cfg(windows)]
mod windows_system;

use std::path::PathBuf;

pub use sysinfo_probe::SysinfoProbe;
/// The fastest probe for the current system.
#[cfg(not(windows))]
pub use sysinfo_probe::SysinfoProbe as SystemProbe;
#[cfg(windows)]
pub use windows::WindowsProbe as SystemProbe;

pub type Pid = u32;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("system probe unavailable: {0}")]
    Unavailable(String),
    /// The process exited, or its PID now belongs to another process.
    #[error("the process is gone")]
    Gone,
    #[error("access denied")]
    Denied,
    #[error("not supported on this system")]
    Unsupported,
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, ProbeError>;

/// One process as seen by a single tick. Cheap fields only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcSample {
    pub pid: Pid,
    pub parent: Option<Pid>,
    /// Process start time in seconds since the Unix epoch; `(pid, start_time)` identifies a
    /// process across PID reuse. One second of resolution is enough for that purpose.
    pub start_time: u64,
    pub name: String,
    /// Total CPU time used so far, in nanoseconds.
    pub cpu_time_ns: u64,
    pub memory_bytes: u64,
    /// Monotonic time (same clock as `Sample::at_ns`) at which `cpu_time_ns` and `memory_bytes`
    /// were read. Usually the tick itself; older for processes that are refreshed less often.
    pub measured_at_ns: u64,
    /// Login session (Windows): 0 holds services and the system. `None` where unknown.
    pub session: Option<u32>,
    /// Owns a visible top-level window: an app the user sees.
    pub has_window: bool,
}

/// Filled in place by `Probe::sample`, so buffers are reused between ticks.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// Monotonic timestamp of the tick, in nanoseconds.
    pub at_ns: u64,
    pub logical_cores: u32,
    /// Physical memory of the machine, in bytes.
    pub memory_total_bytes: u64,
    /// Vigie's own PID, so the app can show its cost without asking the OS.
    pub self_pid: Pid,
    /// The desktop shell (explorer.exe on Windows). Programs it starts are not nested under it.
    pub shell_pid: Option<Pid>,
    pub processes: Vec<ProcSample>,
    /// Slots of processes that exited, kept with their buffers for the next new process.
    /// Not part of the sample.
    #[doc(hidden)]
    pub spare: Vec<ProcSample>,
}

impl Sample {
    /// Next free slot: a spare one when available, so steady-state ticks do not allocate.
    pub(crate) fn slot(&mut self, len: usize, name_capacity: usize) -> &mut ProcSample {
        if len == self.processes.len() {
            if self.processes.len() == self.processes.capacity() {
                let more = self.processes.len() / 4 + 32;
                self.processes.reserve(more);
                self.spare
                    .reserve(self.processes.capacity() - self.spare.len());
            }
            // Out of spare slots: make a batch at once, so a burst of new processes on later
            // ticks finds them ready instead of allocating one by one.
            if self.spare.is_empty() {
                const BATCH: usize = 32;
                self.spare.reserve(BATCH);
                self.spare.resize_with(BATCH, || ProcSample {
                    name: String::with_capacity(name_capacity),
                    ..ProcSample::default()
                });
            }
            let slot = self.spare.pop().unwrap_or_default();
            self.processes.push(slot);
        }
        &mut self.processes[len]
    }

    /// Drops the slots past `len` into the spare pool.
    pub(crate) fn finish(&mut self, len: usize) {
        if len < self.processes.len() {
            self.spare.extend(self.processes.drain(len..));
        }
    }
}

/// Expensive, rarely changing fields of one process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcDetails {
    pub pid: Pid,
    pub start_time: u64,
    pub exe: Option<PathBuf>,
    pub command_line: Vec<String>,
}

/// Monotonic times of one logical core since boot, in nanoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreTimes {
    /// Time spent running anything but the idle loop.
    pub busy_ns: u64,
    pub total_ns: u64,
}

/// Physical and virtual memory, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryInfo {
    pub total: u64,
    /// Free plus standby: what programs can get without anything being paged out.
    pub available: u64,
    /// File cache held by the system (part of it counts as available).
    pub cached: u64,
    /// Committed virtual memory and its limit (physical memory plus page files).
    pub committed: u64,
    pub commit_limit: u64,
    /// Memory held compressed in RAM (Windows memory compression); 0 where it does not exist.
    pub compressed: u64,
}

/// Monotonic byte counters of the machine's physical network interfaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetCounters {
    pub received: u64,
    pub sent: u64,
}

/// Monotonic counters of one physical disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskCounters {
    /// 0 for the first physical disk, as Windows numbers them.
    pub index: u32,
    /// Drive letters on this disk: bit 0 is A:, bit 2 is C:.
    pub letters: u32,
    pub read_bytes: u64,
    pub written_bytes: u64,
    /// Time the disk was busy, and the clock it is measured against, in nanoseconds.
    pub active_ns: u64,
    pub clock_ns: u64,
}

/// Monotonic counters of one GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuCounters {
    /// Position in `SystemInfo::gpus`.
    pub index: u32,
    /// Running time of each engine (3D, copy, video…) since boot, in nanoseconds. A wrapping
    /// counter: some drivers report nonsense, so deltas larger than the elapsed time are noise.
    pub engines_ns: Vec<u64>,
    /// Dedicated video memory in use and available, in bytes.
    pub dedicated_used: u64,
    pub dedicated_total: u64,
}

/// Machine-wide figures for the Performance view. Filled in place, like `Sample`.
#[derive(Clone, Debug, Default)]
pub struct SystemSample {
    /// Monotonic timestamp, same clock as `Sample::at_ns`.
    pub at_ns: u64,
    pub cores: Vec<CoreTimes>,
    pub memory: MemoryInfo,
    /// `None` where the system does not say.
    pub network: Option<NetCounters>,
    pub disks: Vec<DiskCounters>,
    /// Filled by `Probe::gpus`, which is costlier and called less often.
    pub gpus: Vec<GpuCounters>,
    /// When `gpus` was last read, same clock as `at_ns`.
    pub gpus_at_ns: u64,
    pub uptime_s: u64,
}

/// Facts about the machine that do not change while Vigie runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub cpu_model: String,
    pub logical_cores: u32,
    /// Hardware GPUs, by `GpuCounters::index`.
    pub gpus: Vec<String>,
    /// From the firmware, when it says.
    pub memory_slots: Option<MemorySlots>,
}

/// Physical memory slots, as the firmware describes them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySlots {
    pub used: u32,
    pub total: u32,
    /// Configured speed of the installed modules, in megatransfers per second.
    pub speed_mts: Option<u32>,
}

/// An icon as straight (not premultiplied) RGBA pixels, row by row from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What an executable file says about itself. Shared by every process of that executable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExeInfo {
    pub icon: Option<Icon>,
    pub publisher: Option<String>,
    /// A readable name, such as "Brave Browser" for brave.exe.
    pub description: Option<String>,
}

pub trait Probe: Send {
    /// One pass over the system. Cheap fields only: called every tick.
    fn sample(&mut self, out: &mut Sample) -> Result<()>;
    /// Expensive, rarely changing fields (exe path, command line, publisher, icon key) for some PIDs.
    /// Processes that cannot be read are left out.
    fn details(&mut self, pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()>;
    /// Icon, publisher and description of an executable. Call once per path and cache.
    /// `icon_size` is the icon side in physical pixels.
    fn exe_info(&mut self, _exe: &std::path::Path, _icon_size: u32) -> ExeInfo {
        ExeInfo::default()
    }
    /// Machine-wide figures. Called every tick while the Performance view is shown.
    fn system(&mut self, _out: &mut SystemSample) -> Result<()> {
        Err(ProbeError::Unsupported)
    }
    /// GPU engines and memory into `out.gpus`. Costlier than `system` (driver round trips):
    /// call it only while GPU figures are shown, and not necessarily every tick.
    fn gpus(&mut self, _out: &mut SystemSample) -> Result<()> {
        Err(ProbeError::Unsupported)
    }
    /// Facts that do not change while Vigie runs. Call once.
    fn system_info(&mut self) -> SystemInfo {
        SystemInfo::default()
    }
    /// Acts on one process, after checking it is still the process `target` names.
    fn act(&mut self, _target: Target, _action: Action) -> Result<()> {
        Err(ProbeError::Unsupported)
    }
}

/// A process as the user saw it. An action applies only if the PID still belongs to the process
/// that started at `start_time`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub pid: Pid,
    pub start_time: u64,
}

/// Scheduling priority. Real-time is left out on purpose: it can starve the whole system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
}

/// What the user can do to a process. Callers confirm destructive actions first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    End,
    Suspend,
    Resume,
    SetPriority(Priority),
    /// Show the executable in the file manager.
    RevealFile,
    CopyCommandLine,
}
