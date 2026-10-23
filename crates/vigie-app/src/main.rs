//! Vigie binary: the Slint window, fed by one sampler thread. See docs/ARCHITECTURE.md.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod desktop;
mod model;
mod prefs;
mod sampler;
mod translations;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::Context;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use vigie_core::{Group, Sort, SortKey};

use vigie_probe::{Action, Priority};

use crate::sampler::{Command, Settings, UserAction};

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("vigie: {message}\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };

    // The elevated helper of "Replace Task Manager" does its one job and exits (decision 0009).
    if let Some(on) = options.set_replace_taskmgr {
        std::process::exit(match vigie_shell::taskmgr::apply(on) {
            Ok(()) => 0,
            Err(_) => 1,
        });
    }
    // One Vigie per session: a second launch (Ctrl+Shift+Esc, say) shows the first and exits.
    // Test runs (--no-settings) may run next to the user's Vigie.
    let _instance = if options.no_settings {
        None
    } else {
        match vigie_shell::single_instance(|| {
            let _ = slint::invoke_from_event_loop(desktop::show_main_window);
        }) {
            Some(instance) => Some(instance),
            None => return Ok(()),
        }
    };

    // Settings from the file, with the flags on top for this session only (decision 0008).
    let (prefs, session) = prefs::Prefs::load(&options);
    let app = AppWindow::new().context("creating the main window")?;
    // Translations can only be chosen once a component exists.
    prefs::select_language(session.language);
    // "System" assigns nothing: the palette then follows the system theme live.
    if session.theme != vigie_core::settings::Theme::System {
        app.invoke_set_scheme(prefs::scheme(session.theme));
    }
    prefs::show(&app, &session);
    app.set_settings_path(prefs.path().into());
    app.set_app_version(env!("CARGO_PKG_VERSION").into());
    desktop::show_replace_state(&app);
    let weak = app.as_weak();
    app.on_taskmgr_toggled(move |on| desktop::request_replace(weak.clone(), on));
    let weak = app.as_weak();
    app.on_taskmgr_repair(move || desktop::request_replace(weak.clone(), false));
    desktop::set_close_to_tray(session.close_to_tray);
    app.set_rows(ModelRc::from(Rc::new(VecModel::<ProcRow>::default())));
    app.set_icons(ModelRc::from(Rc::new(VecModel::<slint::Image>::default())));
    app.set_perf_cores(ModelRc::from(Rc::new(VecModel::<f32>::default())));
    app.set_perf_disks(ModelRc::from(Rc::new(VecModel::<DiskCard>::default())));
    app.set_perf_gpus(ModelRc::from(Rc::new(VecModel::<GpuCard>::default())));
    app.set_findings(ModelRc::from(Rc::new(VecModel::<FindingRow>::default())));
    app.set_ports(ModelRc::from(Rc::new(VecModel::<PortRowUi>::default())));
    app.set_locks(ModelRc::from(Rc::new(VecModel::<LockRowUi>::default())));
    app.set_startup_items(ModelRc::from(Rc::new(VecModel::<StartupRowUi>::default())));
    app.set_services(ModelRc::from(Rc::new(VecModel::<ServiceRowUi>::default())));
    app.set_palette_items(ModelRc::from(Rc::new(VecModel::<PaletteItem>::default())));
    app.set_tab(options.tab);
    app.set_perf_span_index(options.range);
    app.set_history_open(options.history);
    if options.tab != 3 {
        app.set_search_text(options.filter.as_str().into());
    }
    show_sort(&app, options.sort);

    let about: Rc<RefCell<Option<AboutWindow>>> = Rc::default();
    let weak = app.as_weak();
    app.on_show_about(move || {
        let mut about = about.borrow_mut();
        if about.is_none() {
            *about = AboutWindow::new().ok();
        }
        if let (Some(window), Some(app)) = (about.as_ref(), weak.upgrade()) {
            window.set_history_memory(app.get_history_memory());
            window.set_version(app.get_app_version());
            let _ = window.show();
        }
    });
    if options.about {
        app.invoke_show_about();
    }

    // The sampler stops once every sender is dropped.
    let (commands, receiver) = flume::unbounded::<Command>();

    let sort = Rc::new(Cell::new(options.sort));
    let (weak, tx) = (app.as_weak(), commands.clone());
    app.on_sort_clicked(move |column| {
        let Some(key) = column_key(column) else {
            return;
        };
        let next = sort.get().clicked(key);
        sort.set(next);
        if let Some(app) = weak.upgrade() {
            show_sort(&app, next);
        }
        let _ = tx.send(Command::Sort(next));
    });
    let weak = app.as_weak();
    app.on_move_selection(move |delta| {
        let Some(app) = weak.upgrade() else { return };
        let rows = app.get_rows();
        let Some(model) = rows.as_any().downcast_ref::<VecModel<ProcRow>>() else {
            return;
        };
        let selected = app.get_has_selection().then(|| app.get_selected().key);
        if let Some(index) = model::step_selection(model, selected, delta)
            && let Some(row) = model.row_data(index)
        {
            if selected != Some(row.key) {
                app.invoke_selection_changed(row.pid, row.start);
            }
            app.set_selected(row);
            app.set_has_selection(true);
            app.invoke_scroll_to(index as i32);
        }
    });
    // Adaptive refresh: tell the sampler when the window goes to the background or comes back.
    // Polled, because a window that never had focus gets no focus events, and minimising does
    // not always report occlusion.
    let tx = commands.clone();
    app.on_tab_changed(move |tab| {
        let _ = tx.send(Command::Tab(tab));
    });
    let tx = commands.clone();
    app.on_range_changed(move |index, offset| {
        let _ = tx.send(Command::Range(model::Range::new(index, offset)));
    });
    let tx = commands.clone();
    app.on_process_range_changed(move |index, offset| {
        let _ = tx.send(Command::ProcessRange(model::Range::new(index, offset)));
    });
    let tx = commands.clone();
    app.on_selection_changed(move |pid, start| {
        let process = u32::try_from(pid).ok().zip(u32::try_from(start).ok());
        let _ = tx.send(Command::Select(process));
    });
    let tx = commands.clone();
    app.on_history_toggled(move |open| {
        let _ = tx.send(Command::History(open));
    });
    let tx = commands.clone();
    app.on_ports_filter_edited(move |text| {
        let _ = tx.send(Command::PortsFilter(text.to_string()));
    });
    let tx = commands.clone();
    app.on_check_file(move |path| {
        let path = path.trim().trim_matches('"');
        if !path.is_empty() {
            let _ = tx.send(Command::CheckFile(path.into()));
        }
    });
    let tx = commands.clone();
    app.on_startup_pane_changed(move |pane| {
        let _ = tx.send(Command::StartupPane(pane));
    });
    let tx = commands.clone();
    app.on_startup_refresh(move || {
        let _ = tx.send(Command::StartupRefresh);
    });
    let tx = commands.clone();
    app.on_startup_toggled(move |location, name, enabled| {
        if let Some(location) = model::location_from_code(location) {
            let _ = tx.send(Command::SetStartup {
                location,
                name: name.to_string(),
                enabled,
            });
        }
    });
    let tx = commands.clone();
    app.on_service_filter_edited(move |text| {
        let _ = tx.send(Command::ServiceFilter(text.to_string()));
    });
    let tx = commands.clone();
    app.on_service_control(move |name, start| {
        let _ = tx.send(Command::ControlService {
            name: name.to_string(),
            start,
        });
    });
    // Files dropped anywhere on the window: show who is using them. Every other event goes on
    // to Slint untouched.
    let tx = commands.clone();
    let weak = app.as_weak();
    app.window().on_winit_window_event(move |_, event| {
        use slint::winit_030::winit::event::WindowEvent;
        let Some(app) = weak.upgrade() else {
            return slint::winit_030::EventResult::Propagate;
        };
        match event {
            WindowEvent::HoveredFile(_) => app.set_drop_hover(true),
            WindowEvent::HoveredFileCancelled => app.set_drop_hover(false),
            WindowEvent::DroppedFile(path) => {
                app.set_drop_hover(false);
                app.set_file_path(path.to_string_lossy().as_ref().into());
                app.invoke_show_tab(4);
                let _ = tx.send(Command::CheckFile(path.clone()));
            }
            WindowEvent::ThemeChanged(_) => desktop::system_theme_changed(&app),
            _ => {}
        }
        slint::winit_030::EventResult::Propagate
    });
    let tx = commands.clone();
    app.on_group_clicked(move |index| {
        let group = match index {
            0 => Group::Apps,
            1 => Group::Background,
            2 => Group::System,
            _ => Group::All,
        };
        let _ = tx.send(Command::ToggleGroup(group));
    });
    let tx = commands.clone();
    let weak = app.as_weak();
    let state = Cell::new((true, true));
    let pace_timer = slint::Timer::default();
    pace_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(500),
        move || {
            let Some(app) = weak.upgrade() else { return };
            let window = app.window();
            let focused = window.with_winit_window(|w| w.has_focus()).unwrap_or(true);
            let visible = window.is_visible() && !window.is_minimized();
            if (focused, visible) != state.get() {
                state.set((focused, visible));
                let _ = tx.send(Command::Window { focused, visible });
            }
        },
    );
    let tx = commands.clone();
    app.on_request_action(move |pid, start, code| {
        let (Ok(pid), Ok(start_low_bits), Some(action)) =
            (u32::try_from(pid), u32::try_from(start), user_action(code))
        else {
            return;
        };
        let _ = tx.send(Command::Act {
            pid,
            start_low_bits,
            action,
        });
    });
    let tx = commands.clone();
    app.on_toggle_clicked(move |pid| {
        if let Ok(pid) = u32::try_from(pid) {
            let _ = tx.send(Command::Toggle(pid));
        }
    });
    let tx = commands.clone();
    app.on_search_edited(move |text| {
        let _ = tx.send(Command::Filter(text.to_string()));
    });

    // The palette's commands are few: filtered here, on the UI thread, by their translated titles.
    let weak = app.as_weak();
    app.on_palette_edited(move |text| {
        if let Some(app) = weak.upgrade() {
            let items = palette_items(&app, &text);
            app.set_palette_items(ModelRc::new(VecModel::from(items)));
        }
    });
    app.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    // Changes in the Settings view apply at once; what the user changed is saved.
    let prefs = Rc::new(RefCell::new(prefs));
    let session = Rc::new(Cell::new(session));
    let start = session.get();
    let (weak, tx) = (app.as_weak(), commands.clone());
    app.on_settings_edited(move || {
        let Some(app) = weak.upgrade() else { return };
        let before = session.get();
        let now = prefs.borrow_mut().edited(prefs::read(&app, &before));
        session.set(now);
        if now.interval_ms != before.interval_ms {
            let _ = tx.send(Command::Interval(prefs::interval(&now)));
        }
        if now.theme != before.theme {
            desktop::set_scheme(&app, prefs::scheme(now.theme));
        }
        if now.language != before.language {
            prefs::select_language(now.language);
            desktop::refresh_language(&app);
        }
        if (
            now.alerts,
            now.cpu_alert,
            now.cpu_alert_minutes,
            now.memory_alert,
        ) != (
            before.alerts,
            before.cpu_alert,
            before.cpu_alert_minutes,
            before.memory_alert,
        ) {
            let _ = tx.send(Command::Alerts(prefs::rules(
                &now,
                app.get_tray_available(),
            )));
        }
        desktop::set_close_to_tray(now.close_to_tray);
    });
    // Closing hides to the tray when the user asked for it; otherwise it quits.
    app.window().on_close_requested(|| {
        if !desktop::closes_to_tray() {
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::HideWindow
    });

    // Showing the window first gives its real scale factor, so icons are read at the size they
    // are drawn (16 logical pixels) and never scaled per frame.
    app.show().context("showing the main window")?;
    desktop::install(
        &app,
        commands.clone(),
        !options.no_tray,
        prefs::scheme(start.theme),
    );
    if let Some(text) = options.palette.clone() {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(800), move || {
            if let Some(app) = weak.upgrade() {
                app.invoke_open_palette();
                app.set_palette_text(text.as_str().into());
                app.invoke_palette_edited(text.as_str().into());
            }
        });
    }
    if options.mini {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
            if let Some(app) = weak.upgrade() {
                // Far bottom right: the placement keeps it inside the work area.
                desktop::open_mini_at(&app, i32::MAX / 2, i32::MAX / 2);
            }
        });
    }
    if options.notify_test {
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_secs(2), move || {
            if let Some(app) = weak.upgrade() {
                desktop::notify_test(&app);
            }
        });
    }
    let icon_size = (16.0 * app.window().scale_factor()).round() as u32;
    let settings = Settings {
        sort: options.sort,
        // On the Ports view at start, the search is for ports (below).
        filter: if options.tab == 3 {
            String::new()
        } else {
            options.filter.clone()
        },
        interval: prefs::interval(&start),
        icon_size,
        adaptive: !options.no_adaptive,
        tab: options.tab,
        range: model::Range::new(options.range, 0),
        process_range: model::Range::new(app.get_process_span_index(), 0),
        history_open: options.history,
        alerts: prefs::rules(&start, app.get_tray_available()),
    };
    let sampler = sampler::spawn(app.as_weak(), settings, receiver);
    if let Some(pid) = options.select {
        // Once rows have arrived, select as a click would.
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
            let Some(app) = weak.upgrade() else { return };
            let row = app
                .get_rows()
                .iter()
                .find(|r| r.kind == 1 && r.pid as u32 == pid);
            if let Some(row) = row {
                app.invoke_select(row);
            }
        });
    }
    if let Some((code, pid)) = options.ask {
        // Once rows have arrived, open the confirmation as the menu would. Nothing runs unless
        // someone clicks the confirm button.
        let weak = app.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(1500), move || {
            let Some(app) = weak.upgrade() else { return };
            let row = app
                .get_rows()
                .iter()
                .find(|r| r.kind == 1 && r.pid as u32 == pid);
            if let Some(row) = row {
                app.invoke_ask(row, code);
            }
        });
    }
    for pid in options.expand {
        let _ = commands.send(Command::Toggle(pid));
    }
    // The search box of the view shown at start: processes, or ports.
    if options.tab == 3 && !options.filter.is_empty() {
        app.set_ports_filter(options.filter.as_str().into());
        let _ = commands.send(Command::PortsFilter(options.filter.clone()));
    }
    if options.services {
        app.set_startup_pane(1);
        let _ = commands.send(Command::StartupPane(1));
    }
    if let Some(path) = options.locks {
        app.set_file_path(path.to_string_lossy().as_ref().into());
        let _ = commands.send(Command::CheckFile(path));
    }

    // Runs until "Quit" (tray menu) or closing the window when it does not hide to the tray.
    slint::run_event_loop_until_quit().context("running the event loop")?;
    let _ = app.hide();
    desktop::uninstall();
    // The callbacks hold senders too: replace them so the sampler sees the channel close.
    drop(commands);
    app.on_sort_clicked(|_| {});
    app.on_search_edited(|_| {});
    app.on_toggle_clicked(|_| {});
    app.on_group_clicked(|_| {});
    app.on_tab_changed(|_| {});
    app.on_range_changed(|_, _| {});
    app.on_process_range_changed(|_, _| {});
    app.on_selection_changed(|_, _| {});
    app.on_history_toggled(|_| {});
    app.on_ports_filter_edited(|_| {});
    app.on_check_file(|_| {});
    app.on_startup_pane_changed(|_| {});
    app.on_startup_refresh(|| {});
    app.on_startup_toggled(|_, _, _| {});
    app.on_service_filter_edited(|_| {});
    app.on_service_control(|_, _| {});
    app.on_settings_edited(|| {});
    app.on_palette_edited(|_| {});
    app.window()
        .on_winit_window_event(|_, _| slint::winit_030::EventResult::Propagate);
    app.on_request_action(|_, _, _| {});
    drop(pace_timer);
    let _ = sampler.join();
    Ok(())
}

/// Commands of the palette (`PaletteText` in `ui/app.slint`) whose titles hold every word of
/// `query`, then, when there is a query, searching processes for it.
fn palette_items(app: &AppWindow, query: &str) -> Vec<PaletteItem> {
    const COMMANDS: [i32; 13] = [0, 1, 2, 3, 4, 5, 6, 10, 11, 12, 13, 14, 15];
    let text = app.global::<PaletteText>();
    let query = query.trim().to_lowercase();
    let mut items: Vec<PaletteItem> = COMMANDS
        .iter()
        .map(|&id| PaletteItem {
            id,
            title: text.invoke_title(id),
            hint: text.invoke_hint(id),
        })
        .filter(|item| {
            let title = item.title.to_lowercase();
            query.split_whitespace().all(|word| title.contains(word))
        })
        .collect();
    if !query.is_empty() {
        items.push(PaletteItem {
            id: 16,
            title: text.invoke_search_for(query.as_str().into()),
            hint: "Ctrl+F".into(),
        });
    }
    items
}

/// Column indices used by the header in `ui/app.slint`.
fn column_key(column: i32) -> Option<SortKey> {
    match column {
        0 => Some(SortKey::Name),
        1 => Some(SortKey::Pid),
        2 => Some(SortKey::Cpu),
        3 => Some(SortKey::Memory),
        _ => None,
    }
}

fn show_sort(app: &AppWindow, sort: Sort) {
    let column = match sort.key {
        SortKey::Name => 0,
        SortKey::Pid => 1,
        SortKey::Cpu => 2,
        SortKey::Memory => 3,
    };
    app.set_sort_column(column);
    app.set_sort_ascending(sort.ascending);
}

/// Action codes used by the process menu in `ui/app.slint`.
fn user_action(code: i32) -> Option<UserAction> {
    let priority = |p| Some(UserAction::Probe(Action::SetPriority(p)));
    match code {
        0 => Some(UserAction::End),
        1 => Some(UserAction::EndTree),
        2 => Some(UserAction::Probe(Action::Suspend)),
        3 => Some(UserAction::Probe(Action::Resume)),
        4 => priority(Priority::Idle),
        5 => priority(Priority::BelowNormal),
        6 => priority(Priority::Normal),
        7 => priority(Priority::AboveNormal),
        8 => priority(Priority::High),
        9 => Some(UserAction::Probe(Action::RevealFile)),
        10 => Some(UserAction::Probe(Action::CopyCommandLine)),
        _ => None,
    }
}
