//! `Probe` implemented with `sysinfo` on every system (decision 0002).

use std::time::Instant;

use sysinfo::{CpuRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Pid, Probe, ProbeError, ProcDetails, ProcSample, Result, Sample};

pub struct SysinfoProbe {
    system: System,
    epoch: Instant,
    logical_cores: u32,
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
        Ok(Self {
            system,
            epoch: Instant::now(),
            logical_cores,
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
        out.at_ns = self.epoch.elapsed().as_nanos() as u64;
        out.logical_cores = self.logical_cores;
        out.self_pid = self.self_pid;

        // Overwrite existing entries in place so their `String` buffers are reused.
        let mut len = 0;
        for (pid, process) in self.system.processes() {
            if len == out.processes.len() {
                out.processes.push(ProcSample::default());
            }
            let slot = &mut out.processes[len];
            slot.pid = pid.as_u32();
            slot.parent = process.parent().map(|p| p.as_u32());
            slot.start_time = process.start_time();
            slot.name.clear();
            slot.name.push_str(&process.name().to_string_lossy());
            slot.cpu_time_ns = process.accumulated_cpu_time().saturating_mul(1_000_000);
            slot.memory_bytes = process.memory();
            len += 1;
        }
        out.processes.truncate(len);
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
