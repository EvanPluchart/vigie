//! Vigie binary: the Slint window, fed by one sampler thread. See docs/ARCHITECTURE.md.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod model;

use std::cell::RefCell;
use std::rc::Rc;
use std::thread;

use anyhow::Context;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use vigie_core::Core;
use vigie_probe::{Probe, Sample, SysinfoProbe};

use crate::cli::Options;
use crate::model::UiFrame;

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

    // Dropping the sender tells the sampler to stop.
    let (stop_tx, stop_rx) = flume::bounded::<()>(1);
    let sampler = spawn_sampler(app.as_weak(), options, stop_rx);

    app.run().context("running the event loop")?;
    drop(stop_tx);
    let _ = sampler.join();
    Ok(())
}

/// The sampler thread owns the probe: sample, update the core, send a frame, wait.
fn spawn_sampler(
    window: slint::Weak<AppWindow>,
    options: Options,
    stop: flume::Receiver<()>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("vigie-sampler".into())
        .spawn(move || {
            let mut probe = match SysinfoProbe::new() {
                Ok(probe) => probe,
                Err(err) => {
                    eprintln!("vigie: {err}");
                    return;
                }
            };
            let mut core = Core::new(options.sort, options.filter);
            let mut sample = Sample::default();
            loop {
                match probe.sample(&mut sample) {
                    Ok(()) => {
                        let frame = UiFrame::from(core.update(&sample));
                        let window = window.clone();
                        // Fails once the event loop has quit: time to stop.
                        if slint::invoke_from_event_loop(move || apply(&window, frame)).is_err() {
                            break;
                        }
                    }
                    Err(err) => eprintln!("vigie: sampling failed: {err}"),
                }
                match stop.recv_timeout(options.interval) {
                    Err(flume::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
        })
        .expect("spawning the sampler thread")
}

/// Runs on the UI thread: patch the models, nothing else.
fn apply(window: &slint::Weak<AppWindow>, frame: UiFrame) {
    let Some(app) = window.upgrade() else {
        return;
    };
    let rows = app.get_rows();
    if let Some(model) = rows.as_any().downcast_ref::<VecModel<ProcRow>>() {
        model::patch(model, frame.rows);
    }
    app.set_process_count(frame.process_count);
    app.set_self_cpu(frame.self_cpu);
    app.set_self_memory(frame.self_memory);
}
