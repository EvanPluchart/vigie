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
    /// 0 system, 1 dark, 2 light: given to the mini view too.
    scheme: i32,
    commands: Option<flume::Sender<Command>>,
}

thread_local! {
    static DESKTOP: RefCell<Desktop> = RefCell::default();
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
    DESKTOP.with_borrow(|d| d.close_to_tray && d.tray.is_some())
}

pub fn set_scheme(scheme: i32) {
    DESKTOP.with_borrow_mut(|d| {
        d.scheme = scheme;
        if let Some(mini) = &d.mini {
            mini.invoke_set_scheme(scheme);
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
            app.invoke_show_tab(2);
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
    mini.on_close(close_mini_later);
    // It closes when it loses focus, like the system's own flyouts.
    mini.window().on_winit_window_event(|_, event| {
        if let slint::winit_030::winit::event::WindowEvent::Focused(false) = event {
            close_mini_later();
        }
        EventResult::Propagate
    });
    let _ = mini.show();
    mini.window().with_winit_window(|w| w.focus_window());
    DESKTOP.with_borrow_mut(|d| {
        mini.invoke_set_scheme(d.scheme);
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
