//! Declares the commands the front end may call. Tauri generates one
//! `allow-<command>` permission for each, and `capabilities/default.json`
//! grants exactly those: a command added here but not granted is unreachable.

const COMMANDS: &[&str] = &[
    "host_overview",
    "machines_list",
    "machine_get",
    "machine_start",
    "machine_stop",
    "console_input",
    "console_paste",
    "console_resize",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
