//! Closing the window ends the process. A Vigie that stays alive unseen holds the single
//! instance, and then neither Vigie nor (when it replaces it) Task Manager opens again.

#[cfg(windows)]
#[test]
fn closing_the_window_ends_the_process() {
    use std::process::Command;
    use std::time::{Duration, Instant};

    // The test starts this Vigie itself, so closing it here is safe.
    let mut vigie = Command::new(env!("CARGO_BIN_EXE_vigie"))
        .args(["--no-settings", "--no-tray"])
        .spawn()
        .expect("vigie starts");
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        vigie.try_wait().expect("status").is_none(),
        "vigie is running"
    );

    // Without /F, taskkill asks the window to close, as its close button does.
    let status = Command::new("taskkill")
        .args(["/PID", &vigie.id().to_string()])
        .output()
        .expect("taskkill runs");
    assert!(status.status.success(), "the close request was delivered");

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if vigie.try_wait().expect("status").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = vigie.kill();
    panic!("vigie was still running 8 s after its window closed");
}
