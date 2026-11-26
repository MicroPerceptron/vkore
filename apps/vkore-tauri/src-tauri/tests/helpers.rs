//! The private polkit helper modes are answered before any window exists.
//!
//! polkit re-runs this binary as `pkexec <exe> --vkore-memory-helper <MiB>` (and
//! the device and volume modes). A binary that ignores the flag starts its whole
//! application in place of the helper, with the authorising user's rights. Each
//! mode here must instead exit promptly and unsuccessfully when it is not run by
//! polkit; if one started the app it would sit in its event loop until killed.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_to_exit(arguments: &[&str]) -> (bool, Duration) {
    let began = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vkore-tauri"))
        .args(arguments)
        .env_remove("PKEXEC_UID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut out = String::new();
            let _ = child.stdout.take().unwrap().read_to_string(&mut out);
            return (status.success(), began.elapsed());
        }
        if began.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            return (true, began.elapsed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn every_helper_mode_refuses_and_exits_without_starting_the_app() {
    for arguments in [
        vec!["--vkore-memory-helper", "4096"],
        vec!["--vkore-memory-helper"],
        vec!["--vkore-device-helper"],
        vec!["--vkore-volume-reader"],
        vec!["--vkore-volume-identify"],
    ] {
        let (succeeded, took) = run_to_exit(&arguments);
        assert!(
            !succeeded,
            "{arguments:?} succeeded, or kept running as an application"
        );
        assert!(took < Duration::from_secs(8), "{arguments:?} took {took:?}");
    }
}
