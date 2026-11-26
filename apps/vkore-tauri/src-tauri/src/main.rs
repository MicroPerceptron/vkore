#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(unix)]
fn main() {
    vkore_tauri_lib::run()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("vKore runs on Linux and macOS hosts");
    std::process::exit(2);
}
