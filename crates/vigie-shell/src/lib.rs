//! Desktop integration (decision 0008): the settings file, the tray icon with its menu and
//! notifications, and placing a window near the taskbar. No UI here: like `vigie-probe`, this
//! crate never depends on Slint, so `vigie-app` makes no OS calls of its own.

pub mod settings_file;
#[cfg(windows)]
mod tray_windows;

use std::sync::Arc;

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
}
