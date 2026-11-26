//! An experimental Tauri desktop for vKore.
//!
//! The window is a view over the engine, as the iced desktop is: machines are
//! read from vKore's store and run through `vkored::Session`, and nothing here
//! parses a volume or reimplements a boot. The web side gets eight commands, each
//! validating its input, bounding its output and escaping every string that came
//! from disk or a guest ([`shown`]); it has no file, shell or network access.

#![cfg(unix)]

pub mod console;
pub mod runtime;
pub mod shown;
pub mod views;

use tauri::{Emitter, Manager, RunEvent, State};

use runtime::{Notice, Runtime};
use views::{HostView, MachineDetail, MachineView};

/// The event a worker emits when a machine changes.
pub const UPDATE_EVENT: &str = "machine-update";
/// The event the console thread emits when a machine's console screen changes.
pub const CONSOLE_EVENT: &str = "console-screen";

#[derive(Clone, serde::Serialize)]
struct ConsolePayload {
    name: String,
    #[serde(flatten)]
    screen: console::Screen,
}

type Reply<T> = Result<T, String>;

fn refused(error: impl std::fmt::Display) -> String {
    shown::shown(&error.to_string())
}

#[tauri::command]
fn host_overview(runtime: State<'_, Runtime>) -> Reply<HostView> {
    views::host(&runtime).map_err(refused)
}

#[tauri::command]
fn machines_list(runtime: State<'_, Runtime>) -> Reply<Vec<MachineView>> {
    views::machines(&runtime).map_err(refused)
}

#[tauri::command]
fn machine_get(runtime: State<'_, Runtime>, name: String) -> Reply<MachineDetail> {
    views::machine(&runtime, &name).map_err(refused)
}

#[tauri::command]
fn machine_start(app: tauri::AppHandle, runtime: State<'_, Runtime>, name: String) -> Reply<()> {
    runtime
        .start(&name, move |notice| {
            let _ = match notice {
                Notice::Machine(update) => app.emit(UPDATE_EVENT, update),
                Notice::Console { name, screen } => {
                    app.emit(CONSOLE_EVENT, ConsolePayload { name, screen })
                }
            };
        })
        .map_err(refused)
}

#[tauri::command]
fn machine_stop(runtime: State<'_, Runtime>, name: String) -> Reply<()> {
    runtime.stop(&name).map_err(refused)
}

#[tauri::command]
fn console_input(runtime: State<'_, Runtime>, name: String, text: String) -> Reply<()> {
    runtime.console_input(&name, &text).map_err(refused)
}

#[tauri::command]
fn console_paste(runtime: State<'_, Runtime>, name: String, text: String) -> Reply<()> {
    runtime.console_paste(&name, &text).map_err(refused)
}

#[tauri::command]
fn console_resize(runtime: State<'_, Runtime>, name: String, rows: u16, cols: u16) -> Reply<()> {
    runtime.console_resize(&name, rows, cols).map_err(refused)
}

pub fn run() {
    let root = vkored::State::default_root().unwrap_or_else(|| {
        eprintln!("vkore: cannot find a state directory; set VKORE_STATE");
        std::process::exit(2);
    });
    let runtime = Runtime::new(root, vkored::run::default_qemu());
    let app = tauri::Builder::default()
        .manage(runtime)
        .invoke_handler(tauri::generate_handler![
            host_overview,
            machines_list,
            machine_get,
            machine_start,
            machine_stop,
            console_input,
            console_paste,
            console_resize
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the vKore window");
    let mut closing = false;
    app.run(move |handle, event| {
        // A running machine is a QEMU process; closing the window must stop it
        // rather than orphan it. Hold the exit while the workers power off.
        if let RunEvent::ExitRequested { api, .. } = event {
            let runtime = handle.state::<Runtime>().inner().clone();
            if runtime.running() == 0 || closing {
                return;
            }
            closing = true;
            api.prevent_exit();
            let handle = handle.clone();
            std::thread::spawn(move || {
                runtime.shutdown();
                handle.exit(0);
            });
        }
    });
}
