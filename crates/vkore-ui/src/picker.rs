//! Desktop-owned folder selection. Never waits on the harness HTTP loop.
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::MainThreadMarker;
    use std::{cell::Cell, rc::Rc};

    pub struct Picker {
        panel: Retained<NSOpenPanel>,
        finished: Rc<Cell<bool>>,
    }

    impl Picker {
        pub fn open(
            done: impl Fn(Result<Option<PathBuf>, String>) + Send + 'static,
        ) -> Result<Self, String> {
            let mtm = MainThreadMarker::new().ok_or("Folder picker requires the UI thread")?;
            let window = NSApplication::sharedApplication(mtm)
                .keyWindow()
                .ok_or("Activate the vKore window before choosing a folder")?;
            // SAFETY: AppKit is used on the main thread. The panel and completion
            // block are retained for the sheet lifetime, and the parent belongs
            // to this desktop process. No detached helper can outlive the window.
            unsafe {
                let panel = NSOpenPanel::openPanel(mtm);
                panel.setCanChooseFiles(false);
                panel.setCanChooseDirectories(true);
                panel.setAllowsMultipleSelection(false);
                let finished = Rc::new(Cell::new(false));
                let callback_finished = finished.clone();
                let callback_panel = panel.clone();
                let completion = RcBlock::new(move |response| {
                    callback_finished.set(true);
                    let path = if response == NSModalResponseOK {
                        callback_panel
                            .URL()
                            .and_then(|url| url.path())
                            .map(|path| PathBuf::from(path.to_string()))
                    } else {
                        None
                    };
                    done(Ok(path));
                });
                panel.beginSheetModalForWindow_completionHandler(&window, &completion);
                Ok(Self { panel, finished })
            }
        }
    }

    impl Drop for Picker {
        fn drop(&mut self) {
            if !self.finished.get() {
                // SAFETY: Picker is main-thread-only (Retained<NSOpenPanel>, Rc).
                unsafe {
                    self.panel.cancel(None);
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;
    use std::{
        io::Read,
        process::{Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    pub struct Picker {
        cancel: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl Picker {
        pub fn open(
            done: impl Fn(Result<Option<PathBuf>, String>) + Send + 'static,
        ) -> Result<Self, String> {
            #[cfg(target_os = "windows")]
            let mut command = {
                let mut command = Command::new("powershell.exe");
                command.args(["-NoProfile", "-STA", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $d=New-Object System.Windows.Forms.FolderBrowserDialog; if($d.ShowDialog() -eq 'OK'){[Console]::Out.Write($d.SelectedPath)}"]);
                command
            };
            #[cfg(not(target_os = "windows"))]
            let mut command = {
                let mut command = Command::new("zenity");
                command.args(["--file-selection", "--directory"]);
                command
            };
            let mut child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| error.to_string())?;
            let cancel = Arc::new(AtomicBool::new(false));
            let cancelled = cancel.clone();
            let worker = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(300);
                let result = loop {
                    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Ok(None);
                    }
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            if !status.success() {
                                break Ok(None);
                            }
                            let mut output = String::new();
                            let result = child
                                .stdout
                                .take()
                                .unwrap()
                                .take(65536)
                                .read_to_string(&mut output)
                                .map(|_| {
                                    (!output.trim().is_empty())
                                        .then(|| PathBuf::from(output.trim()))
                                })
                                .map_err(|error| error.to_string());
                            break result;
                        }
                        Err(error) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break Err(error.to_string());
                        }
                        Ok(None) => thread::sleep(Duration::from_millis(50)),
                    }
                };
                done(result);
            });
            Ok(Self {
                cancel,
                worker: Some(worker),
            })
        }
    }
    impl Drop for Picker {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Release);
            // Reap before process exit: detached threads are not guaranteed
            // another scheduling turn when the desktop event loop shuts down.
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

pub use platform::Picker;
