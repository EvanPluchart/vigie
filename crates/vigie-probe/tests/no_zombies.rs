//! The probe keeps a handle on every process it can open. It must let go once a process exits,
//! or Windows keeps the dead process (and its PID) alive. One test per binary, so no other
//! probe in the same process holds handles. Only touches a process it starts itself.

#[cfg(windows)]
#[test]
fn exited_processes_are_released() {
    use std::process::{Command, Stdio};
    use vigie_probe::{Probe, Sample, SystemProbe};

    let mut probe = SystemProbe::new().expect("probe");
    let mut sample = Sample::default();
    let mut child = Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .expect("ping starts");
    let pid = child.id();
    probe.sample(&mut sample).expect("sample");
    assert!(
        sample.processes.iter().any(|p| p.pid == pid),
        "listed while running"
    );

    child.kill().expect("kill own child");
    child.wait().expect("wait");
    drop(child);
    for _ in 0..2 {
        probe.sample(&mut sample).expect("sample");
    }
    assert!(
        !sample.processes.iter().any(|p| p.pid == pid),
        "an exited process is still listed: the probe keeps it alive"
    );
}
