//! Vigie binary: the Slint window, fed by one sampler thread. See docs/ARCHITECTURE.md.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod model;
mod sampler;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::Context;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use vigie_core::{Sort, SortKey};

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
    app.on_show_about(move || {
        let mut about = about.borrow_mut();
        if about.is_none() {
            *about = AboutWindow::new().ok();
        }
        if let Some(window) = about.as_ref() {
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
    };
    let sampler = sampler::spawn(app.as_weak(), settings, receiver);
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
    app.on_request_action(|_, _, _| {});
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
