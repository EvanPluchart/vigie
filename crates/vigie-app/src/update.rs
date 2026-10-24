//! Updates from the app (decision 0012). Checks run on a thread of their own, at launch (after
//! a pause) and every 6 hours; the Settings view shows the result and installs on demand.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::Thread;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Weak};
use vigie_core::update::{self, Version};

use crate::AppWindow;

/// `update-state` in `ui/app.slint`.
const CHECKING: i32 = 1;
const UP_TO_DATE: i32 = 2;
const AVAILABLE: i32 = 3;
const DOWNLOADING: i32 = 4;
const FAILED: i32 = 5;
const INSTALLING: i32 = 6;

/// `update-mode` in `ui/app.slint`.
const INSTALL: i32 = 0;
const RELEASE_PAGE: i32 = 1;
const ELEVATED: i32 = 2;
const UNSUPPORTED: i32 = 3;

/// `update-error-kind` in `ui/app.slint`: what went wrong, worded there.
const CHECK_FAILED: i32 = 1;
const NO_CHECKSUM: i32 = 2;
const MISMATCH: i32 = 3;
const DOWNLOAD_FAILED: i32 = 4;
const NOT_STARTED: i32 = 5;
const TEST_RUN: i32 = 6;

const EVERY: Duration = Duration::from_secs(6 * 3600);

/// Installer flags: no wizard, no prompts, and Vigie started again at the end (`vigie.iss`).
const INSTALLER_ARGS: [&str; 4] = ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/relaunch=1"];

/// An error to show: its kind (worded by the UI) and a technical detail.
type Failure = (i32, String);

struct Shared {
    enabled: AtomicBool,
    /// "Check now" was clicked.
    asked: AtomicBool,
    /// A download or an install is under way.
    busy: AtomicBool,
}

/// The checking thread, and what the UI tells it.
pub struct Updater {
    thread: Thread,
    shared: Arc<Shared>,
}

thread_local! {
    /// The single instance, released just before the installer starts (it waits for that).
    static INSTANCE: RefCell<Option<vigie_shell::Instance>> = const { RefCell::new(None) };
}

/// Keeps the single instance until an update or the end of the process releases it.
pub fn hold_instance(instance: Option<vigie_shell::Instance>) {
    INSTANCE.with_borrow_mut(|held| *held = instance);
}

/// Releases the single instance: a launch from now on starts a new Vigie.
pub fn release_instance() {
    INSTANCE.with_borrow_mut(|held| drop(held.take()));
}

/// What the button can do in this process.
fn mode() -> i32 {
    if !vigie_shell::update::supported() {
        UNSUPPORTED
    } else if vigie_shell::taskmgr::installed().is_none() {
        RELEASE_PAGE
    } else if vigie_shell::update::is_elevated() {
        ELEVATED
    } else {
        INSTALL
    }
}

/// Starts checking: the first check after `first`, then every 6 hours while `enabled`.
/// `current` is this Vigie's version (a test may pretend an older one). `None` when this
/// system has no updater.
pub fn start(app: &AppWindow, enabled: bool, current: String, first: Duration) -> Option<Updater> {
    let mode = mode();
    app.set_update_mode(mode);
    if mode == UNSUPPORTED {
        return None;
    }
    let shared = Arc::new(Shared {
        enabled: AtomicBool::new(enabled),
        asked: AtomicBool::new(false),
        busy: AtomicBool::new(false),
    });
    let (weak, thread_shared) = (app.as_weak(), shared.clone());
    let thread = std::thread::Builder::new()
        .name("vigie-update".into())
        .spawn(move || run(&weak, &thread_shared, &current, first))
        .ok()?
        .thread()
        .clone();
    Some(Updater { thread, shared })
}

fn run(weak: &Weak<AppWindow>, shared: &Shared, current: &str, first: Duration) {
    let mut next = Instant::now() + first;
    loop {
        let asked = shared.asked.swap(false, Ordering::Relaxed);
        let due = shared.enabled.load(Ordering::Relaxed) && Instant::now() >= next;
        if (asked || due) && !shared.busy.load(Ordering::Relaxed) {
            check(weak, current, asked);
            next = Instant::now() + EVERY;
        }
        // Woken early by "Check now" or by the setting turned on.
        let wait = next.saturating_duration_since(Instant::now());
        std::thread::park_timeout(wait.max(Duration::from_secs(1)));
    }
}

/// One check. A background check that fails stays quiet (being offline is normal); one the
/// user asked for says why.
fn check(weak: &Weak<AppWindow>, current: &str, asked: bool) {
    if asked {
        set_state(weak, CHECKING, String::new(), (0, String::new()));
    }
    let latest = vigie_shell::update::redirect_of(update::LATEST_URL).and_then(|to| {
        Version::from_release_url(&to).ok_or_else(|| format!("unexpected address {to}"))
    });
    match latest {
        Ok(latest) => match update::newer(current, latest) {
            Some(newer) => set_state(weak, AVAILABLE, newer.to_string(), (0, String::new())),
            None => set_state(weak, UP_TO_DATE, String::new(), (0, String::new())),
        },
        Err(error) if asked => set_state(weak, FAILED, String::new(), (CHECK_FAILED, error)),
        Err(_) => {}
    }
}

fn set_state(weak: &Weak<AppWindow>, state: i32, version: String, failure: Failure) {
    let _ = weak.upgrade_in_event_loop(move |app| {
        // A download under way keeps its state.
        if matches!(app.get_update_state(), DOWNLOADING | INSTALLING) {
            return;
        }
        app.set_update_state(state);
        if !version.is_empty() {
            app.set_update_version(version.into());
        }
        show_failure(&app, failure);
    });
}

/// The error shown under a failure (kind 0: none) and its technical detail.
fn show_failure(app: &AppWindow, (kind, detail): Failure) {
    app.set_update_error_kind(kind);
    app.set_update_error(detail.into());
}

impl Updater {
    /// The setting changed.
    pub fn set_enabled(&self, on: bool) {
        if !self.shared.enabled.swap(on, Ordering::Relaxed) && on {
            self.thread.unpark();
        }
    }

    /// "Check now".
    pub fn check_now(&self) {
        self.shared.asked.store(true, Ordering::Relaxed);
        self.thread.unpark();
    }

    /// "Update": installs the version shown, or opens its release page. `test` stops after
    /// the checksum (a test run must not install over the user's Vigie).
    pub fn update(&self, app: &AppWindow, test: bool) {
        let Some(version) = Version::parse(&app.get_update_version()) else {
            return;
        };
        match app.get_update_mode() {
            RELEASE_PAGE => {
                vigie_shell::desktop::open_url(&update::release_page(version));
            }
            INSTALL if !self.shared.busy.swap(true, Ordering::Relaxed) => {
                app.set_update_state(DOWNLOADING);
                let (weak, shared) = (app.as_weak(), self.shared.clone());
                let started = std::thread::Builder::new()
                    .name("vigie-download".into())
                    .spawn(move || {
                        let result = fetch(version);
                        let _ = weak.upgrade_in_event_loop(move |app| {
                            install(&app, result, test);
                            shared.busy.store(false, Ordering::Relaxed);
                        });
                    });
                if started.is_err() {
                    self.shared.busy.store(false, Ordering::Relaxed);
                    app.set_update_state(FAILED);
                    show_failure(app, (DOWNLOAD_FAILED, String::new()));
                }
            }
            _ => {}
        }
    }
}

/// Downloads the installer of `version` into a new folder and checks it against the release's
/// `SHA256SUMS`. A missing checksum refuses the update.
fn fetch(version: Version) -> Result<PathBuf, Failure> {
    let name = version.setup_name();
    let sums = vigie_shell::update::text(&version.download_url("SHA256SUMS"))
        .map_err(|e| (NO_CHECKSUM, e))?;
    let expected =
        update::checksum_for(&sums, &name).ok_or_else(|| (NO_CHECKSUM, name.clone()))?;
    let folder = vigie_shell::update::fresh_folder()
        .ok_or_else(|| (DOWNLOAD_FAILED, std::env::temp_dir().display().to_string()))?;
    let path = folder.join(&name);
    let checked = vigie_shell::update::download(&version.download_url(&name), &path)
        .map_err(|e| (DOWNLOAD_FAILED, e))
        .and_then(|()| {
            (vigie_shell::update::sha256_file(&path) == Some(expected))
                .then_some(())
                .ok_or((MISMATCH, String::new()))
        });
    if let Err(failure) = checked {
        let _ = std::fs::remove_dir_all(&folder);
        return Err(failure);
    }
    Ok(path)
}

/// On the UI thread: runs the checked installer, then quits so it can replace Vigie.
fn install(app: &AppWindow, result: Result<PathBuf, Failure>, test: bool) {
    let path = match result {
        Ok(path) => path,
        Err(failure) => {
            app.set_update_state(FAILED);
            show_failure(app, failure);
            return;
        }
    };
    if test {
        app.set_update_state(FAILED);
        show_failure(app, (TEST_RUN, path.display().to_string()));
        return;
    }
    app.set_update_state(INSTALLING);
    // The installer waits until no Vigie holds the single instance.
    release_instance();
    if vigie_shell::update::start_installer(&path, &INSTALLER_ARGS) {
        let _ = slint::quit_event_loop();
    } else {
        app.set_update_state(FAILED);
        show_failure(app, (NOT_STARTED, path.display().to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.1.1 was published before releases carried `SHA256SUMS`: its installer is refused,
    /// never run. Goes online, so only by hand: `cargo test -p vigie -- --ignored checksum`.
    #[test]
    #[ignore = "network"]
    fn a_release_without_checksums_is_refused() {
        let version = Version::parse("0.1.1").unwrap();
        assert!(matches!(fetch(version), Err((NO_CHECKSUM, _))));
    }
}
