//! Closing the window ends the process. A Vigie that stays alive unseen holds the single
//! instance, and then neither Vigie nor (when it replaces it) Task Manager opens again.

/// Starts Vigie with `args`, asks its window to close, and checks the process ends.
#[cfg(windows)]
fn closes_with(args: &[&str]) {
    use std::process::Command;
    use std::time::{Duration, Instant};

    // The test starts this Vigie itself, so closing it here is safe.
    let mut vigie = Command::new(env!("CARGO_BIN_EXE_vigie"))
        .args(args)
        .spawn()
        .expect("vigie starts");
    std::thread::sleep(Duration::from_secs(4));
    assert!(
        vigie.try_wait().expect("status").is_none(),
        "vigie is running"
    );

    // Without /F, taskkill asks the windows to close, as the close button does.
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
    panic!("vigie ({args:?}) was still running 8 s after its window closed");
}

#[cfg(windows)]
#[test]
fn closing_the_window_ends_the_process() {
    closes_with(&["--no-settings", "--no-tray"]);
}

/// With its tray icon, whose teardown runs after the event loop: the hidden tray window gets
/// the close request too and must neither keep Vigie alive nor drop the icon early.
#[cfg(windows)]
#[test]
fn closing_the_window_ends_the_process_with_a_tray_icon() {
    closes_with(&["--no-settings"]);
}
