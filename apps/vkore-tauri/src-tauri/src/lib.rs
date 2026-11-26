//! An experimental Tauri desktop for vKore.
//!
//! The window is a view over the engine, as the iced desktop is: machines are
//! read from vKore's store and run through `vkored::Session`, and nothing here
//! parses a volume or reimplements a boot. The web side gets twelve commands, each
//! validating its input, bounding its output and escaping every string that came
//! from disk or a guest ([`shown`]); it has no file, shell or network access.

#![cfg(unix)]

pub mod console;
pub mod runtime;
pub mod shown;
pub mod views;
pub mod volumes;

use tauri::{Emitter, Manager, RunEvent, State};

use runtime::{Notice, Runtime};
use views::{HostView, MachineDetail, MachineView};
use volumes::{ListingView, PreviewView, VolumeView, Volumes, Where};

/// Fail on any object key that is not camelCase. The page reads camelCase only, and
/// serde's `rename_all` on an enum renames its variants, not the fields inside them,
/// so a snake_case key reaches the page as `undefined` without any other sign.
#[cfg(test)]
pub(crate) fn assert_camel_keys(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map {
                assert!(!key.contains('_'), "the key {key:?} is not camelCase");
                assert_camel_keys(inner);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(assert_camel_keys),
        _ => {}
    }
}

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
fn machine_start(
    app: tauri::AppHandle,
    runtime: State<'_, Runtime>,
    volumes: State<'_, Volumes>,
    name: String,
) -> Reply<()> {
    // Release the machine's disk if a volume is open on it, holding the lock that
    // opening takes, so the two cannot interleave.
    let mut open = volumes.lock();
    open.close_for(&name);
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

/// Run engine work that may block (mounting, reading) off the window's threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, runtime::Refusal> + Send + 'static,
) -> Reply<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(refused)?
        .map_err(refused)
}

#[tauri::command]
async fn volume_open(
    runtime: State<'_, Runtime>,
    volumes: State<'_, Volumes>,
    name: String,
) -> Reply<VolumeView> {
    let (runtime, volumes) = (runtime.inner().clone(), volumes.inner().clone());
    blocking(move || volumes.lock().open(&runtime, &name)).await
}

#[tauri::command]
async fn volume_list(
    runtime: State<'_, Runtime>,
    volumes: State<'_, Volumes>,
    name: String,
    at: Where,
) -> Reply<ListingView> {
    let (runtime, volumes) = (runtime.inner().clone(), volumes.inner().clone());
    blocking(move || volumes.lock().list(&runtime, &name, &at)).await
}

#[tauri::command]
async fn volume_preview(
    runtime: State<'_, Runtime>,
    volumes: State<'_, Volumes>,
    name: String,
    at: Where,
) -> Reply<PreviewView> {
    let (runtime, volumes) = (runtime.inner().clone(), volumes.inner().clone());
    blocking(move || volumes.lock().preview(&runtime, &name, &at)).await
}

#[tauri::command]
async fn volume_close(volumes: State<'_, Volumes>) -> Reply<()> {
    let volumes = volumes.inner().clone();
    blocking(move || {
        volumes.lock().close();
        Ok(())
    })
    .await
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

/// The private polkit helper modes. polkit re-executes *this binary* with one of
/// these flags (`pkexec <exe> --vkore-memory-helper …`) to raise a limit or
/// prepare a device, so they must be answered before any window exists: a binary
/// that ignores them starts its whole application, as the authorising user's
/// root, in place of the helper. Returns the exit code when a helper mode ran.
pub fn helper_mode() -> Option<i32> {
    vkore_volume::helper_entry().or_else(vkore_vm::passthrough::helper_entry)
}

pub fn run() {
    if let Some(code) = helper_mode() {
        std::process::exit(code);
    }
    let root = vkored::State::default_root().unwrap_or_else(|| {
        eprintln!("vkore: cannot find a state directory; set VKORE_STATE");
        std::process::exit(2);
    });
    let runtime = Runtime::new(root, vkored::run::default_qemu());
    let app = tauri::Builder::default()
        .manage(runtime)
        .manage(Volumes::default())
        .invoke_handler(tauri::generate_handler![
            host_overview,
            machines_list,
            machine_get,
            machine_start,
            machine_stop,
            console_input,
            console_paste,
            console_resize,
            volume_open,
            volume_list,
            volume_preview,
            volume_close
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
