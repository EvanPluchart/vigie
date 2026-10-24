//! The desktop on the UI thread (decision 0008): the tray icon, notifications, the mini view and
//! closing to the tray. OS work goes through `vigie-shell`.

use std::cell::RefCell;

use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, ModelRc, VecModel};
use vigie_shell::{Tray, TrayEvent, TrayMenu};

use crate::model::{AlertUi, MiniUi};
use crate::sampler::Command;
use crate::{AppWindow, MiniWindow, NotifyText};

const MINI_WIDTH: f32 = 300.0;
const MINI_HEIGHT: f32 = 330.0;

#[derive(Default)]
struct Desktop {
    tray: Option<Tray>,
    mini: Option<MiniWindow>,
    close_to_tray: bool,
    /// The user's theme: 0 system, 1 dark, 2 light.
    scheme: i32,
    /// The main window's palette was assigned while following the system (the user switched
    /// back to System): it no longer follows on its own, so system changes are applied here.
    assigned: bool,
    commands: Option<flume::Sender<Command>>,
    /// When the mini view last closed because it lost focus: a click on the tray icon makes it
    /// lose focus first, and that same click must close it, not open it again.
    focus_closed_at: Option<std::time::Instant>,
    /// The view a click on the last notification opens: Diagnostics for alerts, Settings for
    /// a new version.
    notification_tab: i32,
}

thread_local! {
    static DESKTOP: RefCell<Desktop> = RefCell::default();
    static MAIN: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// Shows the main window; a second launch of Vigie asks for this (decision 0009).
pub fn show_main_window() {
    let app = MAIN.with_borrow(|w| w.as_ref().and_then(slint::Weak::upgrade));
    if let Some(app) = app {
        show_main(&app);
    }
}

/// The Settings view's "Replace Task Manager" line: state, and whether it can be offered.
pub fn show_replace_state(app: &AppWindow) {
    use vigie_shell::ReplaceState;
    let (state, path) = match vigie_shell::taskmgr::state() {
        ReplaceState::Off => (0, String::new()),
        ReplaceState::On(p) => (1, p.display().to_string()),
        ReplaceState::Stale(p) => (2, p.display().to_string()),
        ReplaceState::Other(p) => (3, p.display().to_string()),
        ReplaceState::Exposed(p) => (5, p.display().to_string()),
    };
    app.set_taskmgr_outdated(state == 1 && vigie_shell::taskmgr::copy_outdated());
    app.set_taskmgr_state(if cfg!(windows) { state } else { 4 });
    app.set_taskmgr_path(path.into());
    app.set_taskmgr_installed(vigie_shell::taskmgr::installed().is_some());
}

/// Turns replacing Task Manager on or off: the administrator prompt and the elevated helper run
/// on a thread of their own, so the window stays responsive meanwhile.
pub fn request_replace(weak: slint::Weak<AppWindow>, on: bool) {
    std::thread::spawn(move || {
        let result = vigie_shell::taskmgr::request(on);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            show_replace_state(&app);
            let kind = match result {
                Ok(()) if on => crate::model::Notice::TASKMGR_ON,
                Ok(()) => crate::model::Notice::TASKMGR_OFF,
                Err(vigie_shell::ReplaceError::Cancelled) => {
                    crate::model::Notice::TASKMGR_CANCELLED
                }
                Err(_) => crate::model::Notice::TASKMGR_FAILED,
            };
            app.set_notice_name("".into());
            app.set_notice_kind(kind);
        });
    });
}

/// Adds the tray icon (unless `with_tray` is false) and remembers how to reach the sampler.
pub fn install(app: &AppWindow, commands: flume::Sender<Command>, with_tray: bool, scheme: i32) {
    let tray = with_tray
        .then(|| {
            let weak = app.as_weak();
            Tray::new("Vigie", menu(app), move |event| {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        on_tray_event(&app, event);
                    }
                });
            })
        })
        .flatten();
    app.set_tray_available(tray.is_some());
    MAIN.with_borrow_mut(|w| *w = Some(app.as_weak()));
    DESKTOP.with_borrow_mut(|d| {
        d.tray = tray;
        d.commands = Some(commands);
        d.scheme = scheme;
    });
}

/// Removes the tray icon and the mini view (before the event loop ends).
pub fn uninstall() {
    DESKTOP.with_borrow_mut(|d| {
        d.mini = None;
        d.tray = None;
        d.commands = None;
    });
}

fn menu(app: &AppWindow) -> TrayMenu {
    let text = app.global::<NotifyText>();
    TrayMenu {
        open: text.invoke_menu_open().into(),
        quit: text.invoke_menu_quit().into(),
    }
}

/// The language changed: menu labels follow.
pub fn refresh_language(app: &AppWindow) {
    let labels = menu(app);
    DESKTOP.with_borrow(|d| {
        if let Some(tray) = &d.tray {
            tray.set_menu(labels);
        }
    });
}

pub fn set_close_to_tray(on: bool) {
    DESKTOP.with_borrow_mut(|d| d.close_to_tray = on);
}

/// Whether closing the window should hide it to the tray rather than quit.
pub fn closes_to_tray() -> bool {
    // Only behind an icon actually shown, or the window would hide out of reach.
    DESKTOP.with_borrow(|d| d.close_to_tray && d.tray.as_ref().is_some_and(|t| t.is_shown()))
}

/// The theme the user chose changed. Slint follows the system only while the palette was never
/// assigned, so "System" chosen at runtime applies the system's current theme instead.
pub fn set_scheme(app: &AppWindow, scheme: i32) {
    let effective = if scheme == 0 {
        system_scheme(app)
    } else {
        scheme
    };
    app.invoke_set_scheme(effective);
    DESKTOP.with_borrow_mut(|d| {
        d.scheme = scheme;
        d.assigned = scheme == 0;
        if let Some(mini) = &d.mini {
            mini.invoke_set_scheme(effective);
        }
    });
}

/// The system switched between dark and light.
pub fn system_theme_changed(app: &AppWindow) {
    if DESKTOP.with_borrow(|d| d.scheme == 0 && d.assigned) {
        set_scheme(app, 0);
    }
}

/// The system's theme now: 1 dark, 2 light, 0 unknown.
fn system_scheme(app: &AppWindow) -> i32 {
    use slint::winit_030::winit::window::Theme;
    match app.window().with_winit_window(|w| w.theme()).flatten() {
        Some(Theme::Dark) => 1,
        Some(Theme::Light) => 2,
        None => 0,
    }
}

/// Says once that `version` is out; a click on it opens Settings.
pub fn notify_update(app: &AppWindow, version: &str) {
    let text = app.global::<NotifyText>();
    DESKTOP.with_borrow_mut(|d| {
        if let Some(tray) = &d.tray {
            d.notification_tab = 6;
            tray.notify(
                &text.invoke_update_title(version.into()),
                &text.invoke_update_body(),
            );
        }
    });
}

/// Sends one notification (the `--notify-test` flag).
pub fn notify_test(app: &AppWindow) {
    let alert = AlertUi {
        kind: 2,
        a: "97 %".into(),
        b: "2".into(),
        name: "Vigie test".into(),
    };
    on_frame(app, vec![alert], None, None);
}

/// What a frame brings for the desktop: notifications, the tooltip, the mini view's figures.
pub fn on_frame(
    app: &AppWindow,
    alerts: Vec<AlertUi>,
    tooltip: Option<(slint::SharedString, slint::SharedString)>,
    mini: Option<MiniUi>,
) {
    let text = app.global::<NotifyText>();
    if !alerts.is_empty() {
        DESKTOP.with_borrow_mut(|d| d.notification_tab = 2);
    }
    DESKTOP.with_borrow(|d| {
        if let Some(tray) = &d.tray {
            for alert in &alerts {
                tray.notify(
                    &text.invoke_title(alert.kind),
                    &text.invoke_body(
                        alert.kind,
                        alert.a.clone(),
                        alert.b.clone(),
                        alert.name.clone(),
                    ),
                );
            }
            if let Some((cpu, memory)) = tooltip {
                tray.set_tooltip(&text.invoke_tooltip(cpu, memory));
            }
        }
        if let (Some(window), Some(mini)) = (&d.mini, mini) {
            window.set_cpu(mini.cpu);
            window.set_memory(mini.memory);
            window.set_cpu_share(mini.cpu_share);
            window.set_memory_share(mini.memory_share);
            window.set_top(ModelRc::new(VecModel::from(mini.top)));
        }
    });
}

fn on_tray_event(app: &AppWindow, event: TrayEvent) {
    match event {
        TrayEvent::Clicked { x, y } => toggle_mini(app, x, y),
        TrayEvent::Open => show_main(app),
        TrayEvent::NotificationClicked => {
            show_main(app);
            let tab = DESKTOP.with_borrow(|d| d.notification_tab);
            app.invoke_show_tab(if tab == 0 { 2 } else { tab });
        }
        TrayEvent::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

/// Brings the main window back: shown, restored, in front.
pub fn show_main(app: &AppWindow) {
    close_mini();
    let _ = app.show();
    app.window().set_minimized(false);
    app.window().with_winit_window(|w| w.focus_window());
}

/// Opens the mini view as a click on the tray icon at (`x`, `y`) would (the `--mini` flag).
pub fn open_mini_at(app: &AppWindow, x: i32, y: i32) {
    if DESKTOP.with_borrow(|d| d.mini.is_none()) {
        toggle_mini(app, x, y);
    }
}

fn toggle_mini(app: &AppWindow, x: i32, y: i32) {
    if DESKTOP.with_borrow(|d| d.mini.is_some()) {
        close_mini();
        return;
    }
    let just_closed = DESKTOP.with_borrow(|d| {
        d.focus_closed_at
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400))
    });
    if just_closed {
        return;
    }
    let Ok(mini) = MiniWindow::new() else { return };
    let scale = app.window().scale_factor();
    let (width, height) = (
        (MINI_WIDTH * scale).round() as i32,
        (MINI_HEIGHT * scale).round() as i32,
    );
    let (px, py) = vigie_shell::place_near(x, y, width, height);
    mini.window()
        .set_position(slint::PhysicalPosition::new(px, py));
    let weak = app.as_weak();
    mini.on_open(move || {
        if let Some(app) = weak.upgrade() {
            show_main(&app);
        }
    });
    mini.on_dismiss(close_mini_later);
    // It closes when it loses focus, like the system's own flyouts.
    mini.window().on_winit_window_event(|_, event| {
        if let slint::winit_030::winit::event::WindowEvent::Focused(false) = event {
            DESKTOP.with_borrow_mut(|d| d.focus_closed_at = Some(std::time::Instant::now()));
            close_mini_later();
        }
        EventResult::Propagate
    });
    let _ = mini.show();
    mini.window().with_winit_window(|w| w.focus_window());
    DESKTOP.with_borrow_mut(|d| {
        // A new window follows the system by itself; only a chosen theme is assigned.
        if d.scheme != 0 {
            mini.invoke_set_scheme(d.scheme);
        }
        if let Some(commands) = &d.commands {
            let _ = commands.send(Command::Mini(true));
        }
        d.mini = Some(mini);
    });
}

/// Closing from inside the mini view's own event handler: do it once the handler returns.
fn close_mini_later() {
    slint::Timer::single_shot(std::time::Duration::ZERO, close_mini);
}

/// Hides and drops the mini view, so its frame buffer is freed.
fn close_mini() {
    let mini = DESKTOP.with_borrow_mut(|d| {
        let mini = d.mini.take();
        if mini.is_some()
            && let Some(commands) = &d.commands
        {
            let _ = commands.send(Command::Mini(false));
        }
        mini
    });
    if let Some(mini) = mini {
        let _ = mini.hide();
    }
}
