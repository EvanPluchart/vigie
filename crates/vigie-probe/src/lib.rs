//! Per-OS system sampling behind one trait. No UI here. See docs/ARCHITECTURE.md.

pub type Pid = u32;

/// One process as seen by a single tick. Cheap fields only.
#[derive(Clone, Debug, Default)]
pub struct ProcSample {
    pub pid: Pid,
    pub parent: Option<Pid>,
    /// Process start time; `(pid, start_time)` identifies a process across PID reuse.
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
    pub processes: Vec<ProcSample>,
}

pub trait Probe: Send {
    type Error: std::error::Error + Send + Sync + 'static;

    fn sample(&mut self, out: &mut Sample) -> Result<(), Self::Error>;
}
