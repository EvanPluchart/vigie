//! Desktop integration (decision 0008): the settings file, the tray icon with its menu and
//! notifications, and placing a window near the taskbar. No UI here: like `vigie-probe`, this
//! crate never depends on Slint, so `vigie-app` makes no OS calls of its own.

#[cfg(windows)]
mod desktop_windows;
#[cfg(windows)]
mod instance_windows;
pub mod settings_file;
#[cfg(windows)]
mod taskmgr_windows;
#[cfg(windows)]
mod tray_windows;

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Small actions on the desktop, for the context menus. Each returns false when it could not be
/// done (or does not exist on this system).
pub mod desktop {
    use std::path::Path;

    /// Puts `text` on the clipboard.
    pub fn copy_text(text: &str) -> bool {
        #[cfg(windows)]
        {
            crate::desktop_windows::copy_text(text)
        }
        #[cfg(not(windows))]
        {
            let _ = text;
            false
        }
    }

    /// Opens the folder of `path` in the file manager, with the file highlighted.
    pub fn reveal(path: &Path) -> bool {
        #[cfg(windows)]
        {
            crate::desktop_windows::reveal(path)
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            false
        }
    }

    /// Shows the system's Properties dialog of a file. Call on the UI thread: the dialog lives
    /// on the caller's message loop.
    pub fn properties(path: &Path) -> bool {
        #[cfg(windows)]
        {
            crate::desktop_windows::properties(path)
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            false
        }
    }

    /// Searches the web for `query` in the default browser, as Task Manager's "Search online".
    pub fn search_online(query: &str) -> bool {
        let url = search_url(query);
        #[cfg(windows)]
        {
            crate::desktop_windows::open_url(&url)
        }
        #[cfg(not(windows))]
        {
            let _ = url;
            false
        }
    }

    /// A Bing search for `query`, as Task Manager uses, with the query percent-encoded.
    pub fn search_url(query: &str) -> String {
        let mut url = String::from("https://www.bing.com/search?q=");
        for byte in query.trim().bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    url.push(char::from(byte));
                }
                b' ' => url.push('+'),
                _ => url.push_str(&format!("%{byte:02X}")),
            }
        }
        url
    }
}

/// Held while this process is the running Vigie (decision 0009).
pub struct Instance {
    #[cfg(windows)]
    _inner: instance_windows::Instance,
}

/// Makes this process the single running Vigie: `Some` when it is the first, and `on_show` then
/// runs (on another thread) whenever a later launch wants the window shown. `None` when Vigie
/// already runs; it has been asked to show itself, and this process should exit.
pub fn single_instance(on_show: impl Fn() + Send + Sync + 'static) -> Option<Instance> {
    #[cfg(windows)]
    {
        instance_windows::acquire(Arc::new(on_show)).map(|inner| Instance { _inner: inner })
    }
    #[cfg(not(windows))]
    {
        let _ = on_show;
        Some(Instance {})
    }
}

/// Who opens when Task Manager is started (decision 0009).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaceState {
    /// Task Manager itself.
    Off,
    /// Vigie, at this path.
    On(PathBuf),
    /// A Vigie that no longer exists at this path: Task Manager cannot start at all.
    Stale(PathBuf),
    /// Another program (Process Explorer, say).
    Other(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceError {
    /// The user declined the administrator prompt.
    Cancelled,
    Denied,
    /// Only an installed Vigie may take Task Manager's place.
    NotInstalled,
    Unsupported,
    Failed(u32),
}

/// The `Debugger` value naming `exe`, quoted.
pub fn debugger_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Reads a `Debugger` value: the program it starts, and whether that is a live Vigie.
#[cfg_attr(not(windows), allow(dead_code))]
fn state_of(value: &str) -> ReplaceState {
    let value = value.trim();
    let program = if let Some(rest) = value.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else {
        match value.to_ascii_lowercase().find(".exe") {
            Some(at) => &value[..at + 4],
            None => value.split_whitespace().next().unwrap_or(value),
        }
    };
    if program.is_empty() {
        return ReplaceState::Off;
    }
    let path = PathBuf::from(program);
    let is_vigie = path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("vigie.exe"));
    if !is_vigie {
        ReplaceState::Other(path)
    } else if path.exists() {
        ReplaceState::On(path)
    } else {
        ReplaceState::Stale(path)
    }
}

/// Replacing Task Manager (Windows; decision 0009).
pub mod taskmgr {
    use super::{ReplaceError, ReplaceState};
    use std::path::PathBuf;

    pub fn state() -> ReplaceState {
        #[cfg(windows)]
        {
            super::taskmgr_windows::state()
        }
        #[cfg(not(windows))]
        {
            ReplaceState::Off
        }
    }

    /// The installed Vigie, when this process is it.
    pub fn installed() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            let installed = super::taskmgr_windows::installed_path()?;
            let current = std::env::current_exe().ok()?;
            let same = |a: &std::path::Path, b: &std::path::Path| match (
                a.canonicalize(),
                b.canonicalize(),
            ) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            same(&installed, &current).then_some(installed)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Asks for the administrator prompt, and turns the replacement on or off in an elevated
    /// copy of this Vigie. Turning it on needs an installed Vigie; turning it off works from any.
    pub fn request(on: bool) -> Result<(), ReplaceError> {
        #[cfg(windows)]
        {
            if on && installed().is_none() {
                return Err(ReplaceError::NotInstalled);
            }
            let exe = std::env::current_exe().map_err(|_| ReplaceError::Failed(0))?;
            let args = if on {
                "--set-replace-taskmgr=on"
            } else {
                "--set-replace-taskmgr=off"
            };
            super::taskmgr_windows::run_elevated(&exe, args)
        }
        #[cfg(not(windows))]
        {
            let _ = on;
            Err(ReplaceError::Unsupported)
        }
    }

    /// In the elevated copy: writes or removes the value. On names the installed Vigie only.
    pub fn apply(on: bool) -> Result<(), ReplaceError> {
        #[cfg(windows)]
        {
            if on {
                let installed = installed().ok_or(ReplaceError::NotInstalled)?;
                super::taskmgr_windows::write(Some(&installed))
            } else {
                super::taskmgr_windows::write(None)
            }
        }
        #[cfg(not(windows))]
        {
            let _ = on;
            Err(ReplaceError::Unsupported)
        }
    }
}

/// What happens on the tray icon, reported to the callback given to `Tray::new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left click on the icon, at this screen position (physical pixels).
    Clicked { x: i32, y: i32 },
    /// "Open Vigie" in the menu, or a double click.
    Open,
    /// A notification was clicked.
    NotificationClicked,
    /// "Quit" in the menu.
    Quit,
}

/// Menu labels, given by the app so they follow its language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenu {
    pub open: String,
    pub quit: String,
}

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The tray icon. It disappears when dropped.
pub struct Tray {
    #[cfg(windows)]
    inner: tray_windows::Tray,
}

impl Tray {
    /// Adds the icon, with `tooltip`. `on_event` runs on the tray's own thread. `None` where the
    /// system has no tray Vigie supports yet.
    pub fn new(
        tooltip: &str,
        menu: TrayMenu,
        on_event: impl Fn(TrayEvent) + Send + Sync + 'static,
    ) -> Option<Tray> {
        #[cfg(windows)]
        {
            tray_windows::Tray::new(tooltip, menu, Arc::new(on_event)).map(|inner| Tray { inner })
        }
        #[cfg(not(windows))]
        {
            let _ = (tooltip, menu, Arc::new(on_event));
            None
        }
    }

    /// Updates the hover text (truncated to what the system shows).
    pub fn set_tooltip(&self, tooltip: &str) {
        #[cfg(windows)]
        self.inner.set_tooltip(tooltip);
        #[cfg(not(windows))]
        let _ = tooltip;
    }

    /// Updates the menu labels (the language changed).
    pub fn set_menu(&self, menu: TrayMenu) {
        #[cfg(windows)]
        self.inner.set_menu(menu);
        #[cfg(not(windows))]
        let _ = menu;
    }

    /// Shows a system notification from the tray icon.
    pub fn notify(&self, title: &str, body: &str) {
        #[cfg(windows)]
        self.inner.notify(title, body);
        #[cfg(not(windows))]
        let _ = (title, body);
    }
}

/// The user's language as a BCP 47 tag ("fr-FR"), from the system settings on Windows and from
/// `LC_ALL`, `LC_MESSAGES` or `LANG` elsewhere.
pub fn user_language() -> Option<String> {
    #[cfg(windows)]
    {
        tray_windows::user_language()
    }
    #[cfg(not(windows))]
    {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|v| !v.is_empty()))
    }
}

/// Where to put a `width` × `height` window opened from the tray at screen point (`x`, `y`):
/// next to the point, inside the work area of the monitor that holds it (the taskbar excluded).
pub fn place_near(x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    #[cfg(windows)]
    let area = tray_windows::work_area(x, y);
    #[cfg(not(windows))]
    let area: Option<Rect> = None;
    match area {
        Some(area) => clamp_into(area, x - width / 2, y - height - 12, width, height),
        None => (x, y),
    }
}

/// Moves a rectangle at (`x`, `y`) into `area`.
fn clamp_into(area: Rect, x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    let margin = 8;
    let x = x.clamp(
        area.x + margin,
        (area.x + area.width - width - margin).max(area.x),
    );
    let y = y.clamp(
        area.y + margin,
        (area.y + area.height - height - margin).max(area.y),
    );
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_queries_are_encoded() {
        assert_eq!(
            desktop::search_url(" brave.exe Brave Browser "),
            "https://www.bing.com/search?q=brave.exe+Brave+Browser"
        );
        assert_eq!(
            desktop::search_url("a&b=é"),
            "https://www.bing.com/search?q=a%26b%3D%C3%A9"
        );
    }

    #[test]
    fn windows_stay_in_the_work_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1032,
        };
        // A click on the taskbar at the bottom right: the window sits above it, inside.
        assert_eq!(
            clamp_into(area, 1900 - 160, 1060 - 400, 320, 380),
            (1592, 644)
        );
        assert_eq!(clamp_into(area, -50, -50, 320, 380), (8, 8));
    }

    /// Windows paths: only meaningful where the value is read.
    #[cfg(windows)]
    #[test]
    fn debugger_values_round_trip_and_stale_paths_are_seen() {
        let here = std::env::current_exe().unwrap();
        // The test binary is not vigie.exe: another program.
        assert_eq!(
            state_of(&debugger_value(&here)),
            ReplaceState::Other(here.clone())
        );
        let gone = Path::new(r"C:\nowhere\Vigie\vigie.exe");
        assert_eq!(debugger_value(gone), r#""C:\nowhere\Vigie\vigie.exe""#);
        assert_eq!(
            state_of(&debugger_value(gone)),
            ReplaceState::Stale(gone.to_path_buf())
        );
        assert_eq!(
            state_of(r"C:\Tools\procexp64.exe /e"),
            ReplaceState::Other(r"C:\Tools\procexp64.exe".into())
        );
        assert_eq!(state_of("  "), ReplaceState::Off);
    }
}
