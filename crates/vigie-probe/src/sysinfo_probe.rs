//! `Probe` implemented with `sysinfo` on every system (decision 0002).

use std::time::Instant;

use sysinfo::{CpuRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Pid, Probe, ProbeError, ProcDetails, Result, Sample};

pub struct SysinfoProbe {
    system: System,
    epoch: Instant,
    logical_cores: u32,
    memory_total_bytes: u64,
    self_pid: Pid,
}

impl SysinfoProbe {
    pub fn new() -> Result<Self> {
        let self_pid = sysinfo::get_current_pid()
            .map_err(|e| ProbeError::Unavailable(e.to_string()))?
            .as_u32();
        let mut system = System::new();
        system.refresh_cpu_list(CpuRefreshKind::nothing());
        let logical_cores = system.cpus().len().max(1) as u32;
        system.refresh_memory();
        let memory_total_bytes = system.total_memory();
        Ok(Self {
            system,
            epoch: Instant::now(),
            logical_cores,
            memory_total_bytes,
            self_pid,
        })
    }
}

impl Probe for SysinfoProbe {
    fn sample(&mut self, out: &mut Sample) -> Result<()> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let at_ns = self.epoch.elapsed().as_nanos() as u64;
        out.at_ns = at_ns;
        out.logical_cores = self.logical_cores;
        out.memory_total_bytes = self.memory_total_bytes;
        out.self_pid = self.self_pid;

        // Overwrite existing entries in place so their `String` buffers are reused.
        let mut len = 0;
        for (pid, process) in self.system.processes() {
            let slot = out.slot(len, 0);
            slot.pid = pid.as_u32();
            slot.parent = process.parent().map(|p| p.as_u32());
            slot.start_time = process.start_time();
            slot.name.clear();
            slot.name.push_str(&process.name().to_string_lossy());
            slot.cpu_time_ns = process.accumulated_cpu_time().saturating_mul(1_000_000);
            slot.memory_bytes = process.memory();
            slot.measured_at_ns = at_ns;
            slot.session = None;
            slot.has_window = false;
            len += 1;
        }
        out.finish(len);
        Ok(())
    }

    fn details(&mut self, pids: &[Pid], out: &mut Vec<ProcDetails>) -> Result<()> {
        let pids: Vec<sysinfo::Pid> = pids.iter().map(|&p| sysinfo::Pid::from_u32(p)).collect();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            false,
            ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        );
        out.clear();
        for pid in &pids {
            // A process that vanished since the tick is skipped, never an error.
            let Some(process) = self.system.process(*pid) else {
                continue;
            };
            out.push(ProcDetails {
                pid: pid.as_u32(),
                start_time: process.start_time(),
                exe: process.exe().map(Into::into),
                command_line: process
                    .cmd()
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
                user: None,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_includes_own_process() {
        let mut probe = SysinfoProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        assert!(sample.logical_cores >= 1);
        assert!(sample.memory_total_bytes > 0);
        assert!(sample.processes.iter().any(|p| p.pid == sample.self_pid));
    }

    #[test]
    fn details_for_own_process() {
        let mut probe = SysinfoProbe::new().expect("probe");
        let mut sample = Sample::default();
        probe.sample(&mut sample).expect("sample");
        let mut details = Vec::new();
        probe
            .details(&[sample.self_pid], &mut details)
            .expect("details");
        assert_eq!(details.len(), 1);
        assert!(details[0].exe.is_some());
    }
}
