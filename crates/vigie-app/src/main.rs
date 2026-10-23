//! Vigie binary: the Slint window, fed by one sampler thread. See docs/ARCHITECTURE.md.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod model;
mod sampler;

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

    let app = AppWindow::new().context("creating the main window")?;
    if let Some(dark) = options.dark {
        app.invoke_set_dark(dark);
    }
    app.set_rows(ModelRc::from(Rc::new(VecModel::<ProcRow>::default())));
    app.set_icons(ModelRc::from(Rc::new(VecModel::<slint::Image>::default())));
    app.set_perf_cores(ModelRc::from(Rc::new(VecModel::<f32>::default())));
    app.set_perf_disks(ModelRc::from(Rc::new(VecModel::<DiskCard>::default())));
    app.set_perf_gpus(ModelRc::from(Rc::new(VecModel::<GpuCard>::default())));
    app.set_tab(options.tab);
    app.set_perf_span_index(options.range);
    app.set_history_open(options.history);
    app.set_search_text(options.filter.as_str().into());
    show_sort(&app, options.sort);
    if let Some(columns) = &options.columns {
        let has = |name: &str| columns.iter().any(|c| c == name);
        app.set_columns(ColumnSet {
            publisher: has("publisher"),
            pid: has("pid"),
            cpu: has("cpu"),
            memory: has("memory"),
        });
    }

    let about: Rc<RefCell<Option<AboutWindow>>> = Rc::default();
    let weak = app.as_weak();
    app.on_show_about(move || {
        let mut about = about.borrow_mut();
        if about.is_none() {
            *about = AboutWindow::new().ok();
        }
        if let (Some(window), Some(app)) = (about.as_ref(), weak.upgrade()) {
            window.set_history_memory(app.get_history_memory());
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

    // Showing the window first gives its real scale factor, so icons are read at the size they
    // are drawn (16 logical pixels) and never scaled per frame.
    app.show().context("showing the main window")?;
    let icon_size = (16.0 * app.window().scale_factor()).round() as u32;
    let settings = Settings {
        sort: options.sort,
        filter: options.filter,
        interval: options.interval,
        icon_size,
        adaptive: !options.no_adaptive,
        tab: options.tab,
        range: model::Range::new(options.range, 0),
        process_range: model::Range::new(app.get_process_span_index(), 0),
        history_open: options.history,
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

    app.run().context("running the event loop")?;
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
    app.on_request_action(|_, _, _| {});
    drop(pace_timer);
    let _ = sampler.join();
    Ok(())
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
