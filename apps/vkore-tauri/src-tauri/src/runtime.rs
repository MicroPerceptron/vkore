//! The machines this process is running.
//!
//! The Tauri shell owns no policy: starting a machine is the engine's own
//! `Session` sequence (preflight, stage, boot, follow, stop, finish), run on one
//! worker thread per machine. This module only remembers what each worker has
//! reported, bounded, so a window opened mid-boot can show where it is.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;
use vkore_vm::{Display, Stage};
use vkored::{Event, MachineStore, Session, State, config::valid_name};

use crate::shown::shown;

/// Machines this process runs at once. Each is a QEMU process and a volume.
pub const MAX_RUNNING: usize = 8;
/// Lines kept per machine, and the most one `machine_get` returns.
pub const MAX_LINES: usize = 200;
/// How long a stop waits for the guest to power off before QEMU is ended.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Where a machine is in its life, as far as this process knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    /// Preflight, staging and the QEMU launch.
    Starting,
    /// QEMU is up and the guest is working towards Ready.
    Booting,
    Ready,
    /// Booted, but a row the system needs did not come up.
    Degraded,
    Stopping,
    /// The last run ended in an error; `message` says which.
    Failed,
}

impl Phase {
    /// Whether a worker owns the machine, so it cannot be started again or
    /// have its volume opened.
    pub fn active(self) -> bool {
        matches!(
            self,
            Phase::Starting | Phase::Booting | Phase::Ready | Phase::Degraded | Phase::Stopping
        )
    }
}

/// One line of a machine's event log.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// Milliseconds since this machine's worker started.
    pub at_ms: u64,
    pub text: String,
}

/// Where one machine's current or last run stands, without its log.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub stage: Option<&'static str>,
    /// How the last run ended, when it did.
    pub message: String,
    /// The guest's own clock when it last changed stage.
    pub uptime_ms: Option<u64>,
}

impl Snapshot {
    pub fn idle() -> Self {
        Self {
            phase: Phase::Idle,
            stage: None,
            message: String::new(),
            uptime_ms: None,
        }
    }
}

struct Run {
    phase: Phase,
    stage: Option<Stage>,
    uptime_ms: Option<u64>,
    message: String,
    lines: VecDeque<Line>,
    /// The newest line has not been sent to the window yet.
    fresh: bool,
    began: Instant,
    stop: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Run {
    fn new(stop: mpsc::Sender<()>) -> Self {
        Self {
            phase: Phase::Starting,
            stage: None,
            uptime_ms: None,
            message: String::new(),
            lines: VecDeque::new(),
            fresh: false,
            began: Instant::now(),
            stop: Some(stop),
            worker: None,
        }
    }

    fn push(&mut self, text: &str) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        let at_ms = u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.lines.push_back(Line {
            at_ms,
            text: shown(text),
        });
        self.fresh = true;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            phase: self.phase,
            stage: self.stage.map(stage_name),
            message: self.message.clone(),
            uptime_ms: self.uptime_ms,
        }
    }
}

pub fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Firmware => "firmware",
        Stage::Kernel => "kernel",
        Stage::Services => "services",
        Stage::Shell => "shell",
        Stage::Ready => "ready",
        Stage::Degraded => "degraded",
        Stage::Failed => "failed",
    }
}

/// What a worker tells the window: where the machine stands, and the one new
/// log line when there is one.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    pub name: String,
    #[serde(flatten)]
    pub snapshot: Snapshot,
    pub line: Option<Line>,
}

/// Why a command was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    BadName,
    Busy,
    TooMany,
    NotRunning,
    NoQemu,
    State(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::BadName => f.write_str("that is not a machine name"),
            Refusal::Busy => f.write_str("that machine is already running"),
            Refusal::TooMany => write!(f, "at most {MAX_RUNNING} machines run at once"),
            Refusal::NotRunning => f.write_str("that machine is not running"),
            Refusal::NoQemu => f.write_str("qemu-system-x86_64 was not found on this host"),
            Refusal::State(error) => f.write_str(&shown(error)),
        }
    }
}

/// The machines this process runs, shared with their workers.
#[derive(Clone)]
pub struct Runtime {
    root: PathBuf,
    qemu: Option<PathBuf>,
    runs: Arc<Mutex<HashMap<String, Run>>>,
}

impl Runtime {
    pub fn new(root: PathBuf, qemu: Option<PathBuf>) -> Self {
        Self {
            root,
            qemu,
            runs: Arc::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn qemu(&self) -> Option<&Path> {
        self.qemu.as_deref()
    }

    pub fn store(&self) -> Result<MachineStore, Refusal> {
        State::open(&self.root)
            .map(MachineStore::new)
            .map_err(|error| Refusal::State(error.to_string()))
    }

    fn runs(&self) -> MutexGuard<'_, HashMap<String, Run>> {
        // A worker that panicked mid-update leaves a plain map behind; keep serving.
        self.runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn snapshot(&self, name: &str) -> Snapshot {
        self.runs()
            .get(name)
            .map_or_else(Snapshot::idle, Run::snapshot)
    }

    /// The remembered log, oldest first, at most [`MAX_LINES`] lines.
    pub fn lines(&self, name: &str) -> Vec<Line> {
        self.runs()
            .get(name)
            .map_or_else(Vec::new, |run| run.lines.iter().cloned().collect())
    }

    pub fn running(&self) -> usize {
        self.runs()
            .values()
            .filter(|run| run.phase.active())
            .count()
    }

    /// Start machine `name` on a worker thread. `notify` is called from that
    /// thread each time the machine's snapshot changes.
    pub fn start(
        &self,
        name: &str,
        notify: impl Fn(Update) + Send + Sync + 'static,
    ) -> Result<(), Refusal> {
        if !valid_name(name) {
            return Err(Refusal::BadName);
        }
        let qemu = self.qemu.clone().ok_or(Refusal::NoQemu)?;
        let store = self.store()?;
        // Only a machine that exists; this also bounds the map to real names.
        store
            .get(name)
            .map_err(|error| Refusal::State(error.to_string()))?;
        let (stop, stopping) = mpsc::channel();
        {
            let mut runs = self.runs();
            if runs.get(name).is_some_and(|run| run.phase.active()) {
                return Err(Refusal::Busy);
            }
            if runs.values().filter(|run| run.phase.active()).count() >= MAX_RUNNING {
                return Err(Refusal::TooMany);
            }
            runs.insert(name.to_owned(), Run::new(stop));
        }
        let notify = Arc::new(notify);
        let runtime = self.clone();
        let worker_name = name.to_owned();
        let spawned = thread::Builder::new()
            .name(format!("vkore-machine-{name}"))
            .spawn(move || runtime.work(&worker_name, &qemu, &stopping, &*notify));
        match spawned {
            Ok(handle) => {
                if let Some(run) = self.runs().get_mut(name) {
                    run.worker = Some(handle);
                }
                Ok(())
            }
            Err(error) => {
                self.runs().remove(name);
                Err(Refusal::State(error.to_string()))
            }
        }
    }

    /// Ask machine `name` to power off. The worker follows it to the end.
    pub fn stop(&self, name: &str) -> Result<(), Refusal> {
        if !valid_name(name) {
            return Err(Refusal::BadName);
        }
        let runs = self.runs();
        let run = runs
            .get(name)
            .filter(|run| run.phase.active())
            .ok_or(Refusal::NotRunning)?;
        // The worker drops its receiver when it ends; a send then fails harmlessly.
        if let Some(stop) = &run.stop {
            let _ = stop.send(());
        }
        Ok(())
    }

    /// Ask every machine to power off and wait for the workers, which are
    /// themselves bounded by the stop grace. Called as the window closes, so
    /// no QEMU outlives the app.
    pub fn shutdown(&self) {
        let workers: Vec<JoinHandle<()>> = {
            let mut runs = self.runs();
            runs.values_mut()
                .filter_map(|run| {
                    if let Some(stop) = &run.stop {
                        let _ = stop.send(());
                    }
                    run.worker.take()
                })
                .collect()
        };
        for worker in workers {
            let _ = worker.join();
        }
    }

    fn update(&self, name: &str, notify: &dyn Fn(Update), change: impl FnOnce(&mut Run)) {
        let (snapshot, line) = {
            let mut runs = self.runs();
            let Some(run) = runs.get_mut(name) else {
                return;
            };
            change(run);
            let line = std::mem::take(&mut run.fresh)
                .then(|| run.lines.back().cloned())
                .flatten();
            (run.snapshot(), line)
        };
        notify(Update {
            name: name.to_owned(),
            snapshot,
            line,
        });
    }

    fn work(
        &self,
        name: &str,
        qemu: &Path,
        stopping: &mpsc::Receiver<()>,
        notify: &dyn Fn(Update),
    ) {
        let outcome = self.run(name, qemu, stopping, notify);
        self.update(name, notify, |run| {
            run.stop = None;
            match outcome {
                Ok(summary) => {
                    run.phase = Phase::Idle;
                    run.stage = None;
                    run.message = shown(&summary);
                    run.push(&summary);
                }
                Err(error) => {
                    run.phase = Phase::Failed;
                    run.message = shown(&error);
                    run.push(&error);
                }
            }
        });
    }

    fn run(
        &self,
        name: &str,
        qemu: &Path,
        stopping: &mpsc::Receiver<()>,
        notify: &dyn Fn(Update),
    ) -> Result<String, String> {
        let store = self.store().map_err(|refusal| refusal.to_string())?;
        let config = store.get(name).map_err(|error| error.to_string())?;
        self.update(name, notify, |run| {
            run.push("Checking hardware settings and permissions…")
        });
        let host = vkore_vm::hardware::HostResources::probe(&self.root)?;
        if vkored::hardware::uses_guardian(&config) {
            vkored::hardware::validate_host(&config, &host)?;
        } else {
            vkored::hardware::authorize_configuration(&config, &host)?;
        }
        let mut sink = |event: &Event| self.on_event(name, notify, event);
        let mut session = Session::start(&store, name, qemu, Display::Headless, &mut sink)
            .map_err(|error| error.to_string())?;
        loop {
            let running = session
                .poll(Duration::from_millis(250), &mut sink)
                .map_err(|error| error.to_string())?;
            if !running || stopping.try_recv().is_ok() {
                break;
            }
        }
        if session.machine().running() {
            self.update(name, notify, |run| run.phase = Phase::Stopping);
            session
                .stop(STOP_GRACE, &mut sink)
                .map_err(|error| error.to_string())?;
        }
        let changes = session
            .finish(&store, &mut sink)
            .map_err(|error| error.to_string())?;
        Ok(match changes {
            Some(changes) if !changes.is_empty() => format!(
                "stopped; the machine changed {} file(s) in its project (vkore machine apply {name})",
                changes.file_count()
            ),
            _ => "stopped".into(),
        })
    }

    fn on_event(&self, name: &str, notify: &dyn Fn(Update), event: &Event) {
        self.update(name, notify, |run| {
            match event {
                Event::Started { .. } => run.phase = Phase::Booting,
                Event::Stage { stage, uptime_ms } => {
                    run.stage = Some(*stage);
                    run.uptime_ms = u64::try_from(*uptime_ms).ok();
                    if run.phase != Phase::Stopping {
                        run.phase = match stage {
                            Stage::Ready => Phase::Ready,
                            Stage::Degraded => Phase::Degraded,
                            _ => Phase::Booting,
                        };
                    }
                }
                Event::Stopping => run.phase = Phase::Stopping,
                _ => {}
            }
            // QEMU's RTC ticks are not news.
            if !matches!(event, Event::Qemu { name, .. } if name == "RTC_CHANGE") {
                run.push(&describe(event));
            }
        });
    }
}

/// One line for an engine event.
pub fn describe(event: &Event) -> String {
    match event {
        Event::Preflight(preflight) => format!(
            "preflight: image {} on {:?}",
            preflight
                .medium
                .sha256
                .get(..12)
                .unwrap_or(&preflight.medium.sha256),
            preflight.accel
        ),
        Event::Staged { files, skipped, .. } => {
            format!("staged the project: {files} file(s), {skipped} left out")
        }
        Event::Started { pid } => format!("QEMU started (pid {pid})"),
        Event::CpuPowerManagement => "CPU power management enabled on pinned CPUs".into(),
        Event::MemoryBacking { summary } => summary.clone(),
        Event::Console { .. } => "the console shell is up on COM2".into(),
        Event::Stage { stage, uptime_ms } => {
            format!(
                "boot: {} at {:.1} s",
                stage_name(*stage),
                *uptime_ms as f64 / 1000.0
            )
        }
        Event::Qemu { name, .. } => format!("qemu: {name}"),
        Event::Stopping => "stopping".into(),
        Event::Stopped { forced: true } => "stopped (QEMU was ended)".into(),
        Event::Stopped { forced: false } => "stopped".into(),
        Event::Exited { code } => match code {
            Some(code) => format!("QEMU exited with status {code}"),
            None => "QEMU exited".into(),
        },
        Event::Changes {
            created,
            modified,
            deleted,
            bytes,
        } => format!(
            "project changes: {created} new, {modified} changed, {deleted} removed ({bytes} bytes)"
        ),
        Event::Applied(applied) => format!("applied project changes: {applied:?}"),
        Event::Warning { message } => format!("warning: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> (tempdir::Dir, Runtime) {
        let dir = tempdir::Dir::new();
        let runtime = Runtime::new(
            dir.path().to_owned(),
            Some(PathBuf::from("/nonexistent/qemu")),
        );
        (dir, runtime)
    }

    /// A scratch directory without a new dependency.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "vkore-tauri-test-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
                let _ = std::fs::remove_dir_all(&path);
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn a_name_that_is_not_a_machine_is_refused_before_anything_opens() {
        let (_dir, runtime) = runtime();
        for name in ["", "../escape", "a/b", "x\0y", &"n".repeat(500)] {
            assert_eq!(
                runtime.start(name, |_| {}),
                Err(Refusal::BadName),
                "{name:?}"
            );
            assert_eq!(runtime.stop(name), Err(Refusal::BadName), "{name:?}");
        }
    }

    #[test]
    fn a_machine_that_does_not_exist_is_not_remembered() {
        let (_dir, runtime) = runtime();
        assert!(matches!(
            runtime.start("ghost", |_| {}),
            Err(Refusal::State(_))
        ));
        assert!(runtime.runs().is_empty());
        assert_eq!(runtime.snapshot("ghost").phase, Phase::Idle);
    }

    #[test]
    fn stopping_what_is_not_running_is_refused() {
        let (_dir, runtime) = runtime();
        assert_eq!(runtime.stop("kore"), Err(Refusal::NotRunning));
    }

    #[test]
    fn the_log_is_bounded_and_escaped() {
        let (stop, _keep) = mpsc::channel();
        let mut run = Run::new(stop);
        for i in 0..MAX_LINES * 3 {
            run.push(&format!("line {i}\u{202E}"));
        }
        assert_eq!(run.lines.len(), MAX_LINES);
        assert!(run.lines.iter().all(|line| !line.text.contains('\u{202E}')));
        assert_eq!(
            run.lines.front().unwrap().text,
            format!("line {}\\u{{202E}}", MAX_LINES * 2)
        );
    }

    #[test]
    fn only_an_owned_machine_is_active() {
        assert!(!Phase::Idle.active());
        assert!(!Phase::Failed.active());
        for phase in [
            Phase::Starting,
            Phase::Booting,
            Phase::Ready,
            Phase::Degraded,
            Phase::Stopping,
        ] {
            assert!(phase.active());
        }
    }
}
