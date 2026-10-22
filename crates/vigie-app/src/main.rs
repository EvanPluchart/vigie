//! Vigie binary: the Slint window, fed by one sampler thread. See docs/ARCHITECTURE.md.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod model;
mod sampler;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::Context;
use slint::{ComponentHandle, ModelRc, VecModel};
use vigie_core::{Sort, SortKey};

use crate::sampler::{Command, Settings};

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
    app.set_search_text(options.filter.as_str().into());
    show_sort(&app, options.sort);

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
    app.on_search_edited(move |text| {
        let _ = tx.send(Command::Filter(text.to_string()));
    });

    let settings = Settings {
        sort: options.sort,
        filter: options.filter,
        interval: options.interval,
    };
    let sampler = sampler::spawn(app.as_weak(), settings, receiver);

    app.run().context("running the event loop")?;
    // The callbacks hold senders too: replace them so the sampler sees the channel close.
    drop(commands);
    app.on_sort_clicked(|_| {});
    app.on_search_edited(|_| {});
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
