//! Per-OS system sampling behind one trait. No UI here. See docs/ARCHITECTURE.md.

mod sysinfo_probe;

use std::path::PathBuf;

pub use sysinfo_probe::SysinfoProbe;

pub type Pid = u32;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("system probe unavailable: {0}")]
    Unavailable(String),
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
}

/// Filled in place by `Probe::sample`, so buffers are reused between ticks.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// Monotonic timestamp of the tick, in nanoseconds.
    pub at_ns: u64,
    pub logical_cores: u32,
    /// Vigie's own PID, so the app can show its cost without asking the OS.
    pub self_pid: Pid,
    pub processes: Vec<ProcSample>,
}

/// Expensive, rarely changing fields of one process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcDetails {
    pub pid: Pid,
    pub start_time: u64,
    pub exe: Option<PathBuf>,
    pub command_line: Vec<String>,
}

pub trait Probe: Send {
    /// One pass over the system. Cheap fields only: called every tick.
    fn sample(&mut self, out: &mut Sample) -> Result<()>;
    /// Expensive, rarely changing fields (exe path, command line, publisher, icon key) for some PIDs.
    fn details(&mut self, pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()>;
}
