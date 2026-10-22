//! Per-OS system sampling behind one trait. No UI here. See docs/ARCHITECTURE.md.

mod sysinfo_probe;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_actions;
#[cfg(windows)]
mod windows_exe;

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
            let slot = self.spare.pop().unwrap_or_else(|| ProcSample {
                name: String::with_capacity(name_capacity),
                ..ProcSample::default()
            });
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
