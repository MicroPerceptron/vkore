//! One Kore machine: a supervised QEMU, its boot followed through the
//! serial console, and its stop.
//!
//! **Stopping.** The machine is first asked through its ACPI power button,
//! the one request a guest can honour on its own terms; QEMU then exits when
//! the guest powers off. A guest that does not answer within the caller's
//! grace is ended through QMP, and that is reported as such
//! ([`StopOutcome::Forced`]): Kore's volume keeps its last committed root, so
//! what the guest committed survives, and what it had not committed is lost.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use crate::{
    probe::QemuCaps,
    profile::{MachineSpec, ProfileError},
    qmp::{Qmp, QmpError},
    readiness::{Readiness, Stage},
};

/// The most console bytes read per poll, so a flood cannot stall a caller.
const READ_CHUNK: usize = 256 << 10;

/// Startup may populate/lock guest RAM and establish VFIO DMA mappings before
/// QMP negotiation completes. Allow 10 s for fixed setup plus 2 s per GiB,
/// rounded up. This is patience, not a throughput claim. MachineSpec admission
/// bounds RAM to 1 TiB, so the longest wait is 2058 s; process exit fails early.
fn startup_patience(memory_mib: u32) -> Duration {
    Duration::from_secs(10 + 2 * u64::from(memory_mib).div_ceil(1024))
}

/// What happened to a machine since the last poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineEvent {
    /// The boot reached a new stage.
    Stage(Stage),
    /// A QEMU event (`POWERDOWN`, `SHUTDOWN`, `STOP`, `RESET`, …) and its
    /// data.
    Qemu(String, serde_json::Value),
    /// QEMU exited.
    Exited(Option<i32>),
}

/// How a stop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    /// The guest powered itself off after the power button.
    Graceful,
    /// The guest did not answer in time; QEMU was ended.
    Forced,
    /// QEMU had already exited.
    AlreadyStopped,
}

/// What a machine could not do.
#[derive(Debug)]
pub enum MachineError {
    Profile(ProfileError),
    Io(PathBuf, io::Error),
    Qmp(QmpError),
}

impl fmt::Display for MachineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => write!(f, "{error}"),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Self::Qmp(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for MachineError {}

impl From<ProfileError> for MachineError {
    fn from(error: ProfileError) -> Self {
        Self::Profile(error)
    }
}

impl From<QmpError> for MachineError {
    fn from(error: QmpError) -> Self {
        Self::Qmp(error)
    }
}

/// A running (or exited) Kore machine.
pub struct Machine {
    child: Child,
    qemu_pid: u32,
    qemu_log: PathBuf,
    managed: bool,
    /// The guardian owns QEMU and restores host drivers after reaping it.
    device_control: Option<std::process::ChildStdin>,
    qmp: Option<Qmp>,
    serial: PathBuf,
    console: Option<File>,
    readiness: Readiness,
    exited: Option<ExitStatus>,
    started: Instant,
}

impl Machine {
    /// Start `spec` on the QEMU at `qemu`, as probed. QEMU's own messages go
    /// to `qemu_log`; the console to the spec's serial log.
    pub fn start(
        qemu: &Path,
        caps: &QemuCaps,
        spec: &MachineSpec,
        qemu_log: &Path,
    ) -> Result<Self, MachineError> {
        let mut arguments = spec.arguments(caps)?;
        // Keep guest CPUs stopped until QMP negotiation and launch checks finish.
        arguments.push("-S".into());
        if let Some(runtime) = crate::runtime::Runtime::for_qemu(qemu)
            .map_err(|error| MachineError::Io(qemu.to_path_buf(), error))?
        {
            arguments.extend(["-L".into(), runtime.data.display().to_string()]);
        }
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |error| MachineError::Io(path, error)
        };
        // A fresh console: readiness reads from its first byte.
        prepare_socket(&spec.qmp_socket).map_err(io(&spec.qmp_socket))?;
        if let Some(socket) = &spec.console_socket {
            prepare_socket(socket).map_err(io(socket))?;
        }
        File::create(&spec.serial_log).map_err(io(&spec.serial_log))?;
        let log = File::create(qemu_log).map_err(io(qemu_log))?;
        let errors = log.try_clone().map_err(io(qemu_log))?;
        let (child, qemu_pid, device_control) = if spec.resources.passthrough.is_empty() {
            let child = Command::new(qemu)
                .args(&arguments)
                .stdin(Stdio::null())
                .stdout(log)
                .stderr(errors)
                .spawn()
                .map_err(io(qemu))?;
            let pid = child.id();
            (child, pid, None)
        } else {
            #[cfg(target_os = "linux")]
            {
                let started = crate::passthrough::launch(
                    qemu,
                    arguments,
                    &spec.resources.passthrough,
                    spec.memory_mib,
                    errors,
                )
                .map_err(|error| MachineError::Io(qemu.to_owned(), std::io::Error::other(error)))?;
                (started.child, started.pid, Some(started.control))
            }
            #[cfg(not(target_os = "linux"))]
            return Err(ProfileError::Unsupported("NPU passthrough requires Linux.".into()).into());
        };
        let mut machine = Self {
            child,
            qemu_pid,
            qemu_log: qemu_log.to_owned(),
            managed: device_control.is_some(),
            device_control,
            qmp: None,
            serial: spec.serial_log.clone(),
            console: None,
            readiness: Readiness::new(),
            exited: None,
            started: Instant::now(),
        };
        match Qmp::connect_while(&spec.qmp_socket, startup_patience(spec.memory_mib), || {
            machine.child.try_wait().map(|status| status.is_none())
        }) {
            Ok(mut qmp) => {
                let ready = (|| {
                    if qmp.status()?.running {
                        return Err(QmpError::Protocol(
                            "QEMU ran before launch checks completed".into(),
                        ));
                    }
                    let cpus = qmp.execute(&qapi_qmp::query_cpus_fast {})?;
                    if cpus.len() != usize::from(spec.cpus) {
                        return Err(QmpError::Protocol(format!(
                            "QEMU created {} CPUs; {} requested",
                            cpus.len(),
                            spec.cpus
                        )));
                    }
                    let threads: Vec<_> = cpus
                        .iter()
                        .map(|cpu| match cpu {
                            qapi_qmp::CpuInfoFast::x86_64(base)
                            | qapi_qmp::CpuInfoFast::i386(base) => {
                                Ok((base.cpu_index, base.thread_id))
                            }
                            _ => Err(QmpError::Protocol("QEMU reported a non-x86 CPU".into())),
                        })
                        .collect::<Result<_, _>>()?;
                    crate::hardware::bind_vcpus(machine.qemu_pid, &threads, &spec.resources)
                        .map_err(QmpError::Protocol)?;
                    qmp.resume()
                })();
                if let Err(error) = ready {
                    machine.kill_and_reap();
                    return Err(error.into());
                }
                machine.qmp = Some(qmp);
            }
            Err(error) => {
                // QEMU refused its own command line: report what it said.
                machine.kill_and_reap();
                let said = log_tail(qemu_log).unwrap_or_default();
                return Err(MachineError::Io(
                    qemu.to_path_buf(),
                    io::Error::other(format!("QEMU did not start ({error}): {}", said.trim())),
                ));
            }
        }
        Ok(machine)
    }

    fn kill_and_reap(&mut self) {
        if self.device_control.take().is_none() {
            let _ = self.child.kill();
        }
        self.exited = self.child.wait().ok();
        self.qmp = None;
    }

    pub fn id(&self) -> u32 {
        self.qemu_pid
    }

    pub fn readiness(&self) -> &Readiness {
        &self.readiness
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Whether QEMU still runs.
    pub fn running(&mut self) -> bool {
        self.reap();
        self.exited.is_none()
    }

    fn reap(&mut self) -> Option<ExitStatus> {
        if self.exited.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exited = Some(status);
            self.qmp = None;
        }
        self.exited
    }

    /// Read what the console wrote and what QEMU announced since the last
    /// poll, waiting up to `wait` for a QEMU event when nothing else came.
    pub fn poll(&mut self, wait: Duration) -> Result<Vec<MachineEvent>, MachineError> {
        let mut events = Vec::new();
        let was_running = self.exited.is_none();
        let bytes = self.read_console()?;
        events.extend(
            self.readiness
                .feed(&bytes)
                .into_iter()
                .map(MachineEvent::Stage),
        );
        if let Some(qmp) = self.qmp.as_mut() {
            let wait = if events.is_empty() {
                wait
            } else {
                Duration::from_millis(1)
            };
            match qmp.next_event(wait) {
                Ok(Some(event)) => {
                    let event = event.into_value();
                    events.push(MachineEvent::Qemu(
                        event["event"].as_str().unwrap_or("").to_owned(),
                        event.get("data").cloned().unwrap_or_default(),
                    ));
                }
                Ok(None) => {}
                // QEMU closed the protocol: it is exiting.
                Err(_) => self.qmp = None,
            }
        } else if events.is_empty() {
            std::thread::sleep(wait.min(Duration::from_millis(100)));
        }
        if was_running && let Some(status) = self.reap() {
            // The console's last lines, then the exit.
            let tail = self.read_console()?;
            events.extend(
                self.readiness
                    .feed(&tail)
                    .into_iter()
                    .map(MachineEvent::Stage),
            );
            self.check_guardian(status)?;
            events.push(MachineEvent::Exited(status.code()));
        }
        Ok(events)
    }

    fn check_guardian(&self, status: ExitStatus) -> Result<(), MachineError> {
        if self.managed && status.code() == Some(125) {
            return Err(MachineError::Io(
                self.qemu_log.clone(),
                io::Error::other(format!(
                    "NPU guardian failed; host driver restoration may need attention: {}",
                    log_tail(&self.qemu_log).unwrap_or_default().trim()
                )),
            ));
        }
        Ok(())
    }

    fn read_console(&mut self) -> Result<Vec<u8>, MachineError> {
        if self.console.is_none() {
            match File::open(&self.serial) {
                Ok(file) => self.console = Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(MachineError::Io(self.serial.clone(), error)),
            }
        }
        let console = self.console.as_mut().expect("opened above");
        let mut bytes = Vec::new();
        console
            .take(READ_CHUNK as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| MachineError::Io(self.serial.clone(), error))?;
        Ok(bytes)
    }

    /// Ask the guest to power off, wait up to `grace` for it, then end QEMU.
    pub fn stop(&mut self, grace: Duration) -> Result<StopOutcome, MachineError> {
        if let Some(status) = self.reap() {
            self.check_guardian(status)?;
            return Ok(StopOutcome::AlreadyStopped);
        }
        if let Some(qmp) = self.qmp.as_mut() {
            let _ = qmp.power_button();
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if let Some(status) = self.reap() {
                self.check_guardian(status)?;
                return Ok(StopOutcome::Graceful);
            }
            self.poll(Duration::from_millis(200))?;
        }
        self.force();
        if let Some(status) = self.exited {
            self.check_guardian(status)?;
        }
        Ok(StopOutcome::Forced)
    }

    /// End QEMU now: through QMP, then by signal if it lingers.
    pub fn force(&mut self) {
        if self.reap().is_some() {
            return;
        }
        if let Some(qmp) = self.qmp.as_mut() {
            let _ = qmp.quit();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.reap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.kill_and_reap();
    }

    /// The guest's screen, as a PPM image at `path`.
    pub fn screendump(&mut self, path: &Path) -> Result<(), MachineError> {
        let qmp = self.qmp.as_mut().ok_or_else(|| {
            MachineError::Io(
                path.to_path_buf(),
                io::Error::other("the machine is not running"),
            )
        })?;
        qmp.screendump(path)?;
        Ok(())
    }

    /// Press and release `keys` together on the guest's keyboard.
    pub fn send_keys(&mut self, keys: &[&str]) -> Result<(), MachineError> {
        let qmp = self.qmp.as_mut().ok_or_else(|| {
            MachineError::Io(
                self.serial.clone(),
                io::Error::other("the machine is not running"),
            )
        })?;
        qmp.send_keys(keys)?;
        Ok(())
    }

    /// The first 4 MiB of the console log, matching a bounded preview.
    pub fn console(&self) -> Result<Vec<u8>, MachineError> {
        let mut file = File::open(&self.serial)
            .map_err(|error| MachineError::Io(self.serial.clone(), error))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| MachineError::Io(self.serial.clone(), error))?;
        let mut bytes = Vec::new();
        file.take(4 << 20)
            .read_to_end(&mut bytes)
            .map_err(|error| MachineError::Io(self.serial.clone(), error))?;
        Ok(bytes)
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.force();
    }
}

/// A private copy of the firmware's variable store for one machine: the
/// template is never written.
pub fn private_vars(template: &Path, destination: &Path) -> io::Result<()> {
    let mut source = File::open(template)?;
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut source, &mut target)?;
    target.sync_all()
}

/// Keep failure diagnostics bounded even if an emulator floods its log.
fn log_tail(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(64 << 10)))?;
    let mut bytes = Vec::new();
    file.take(64 << 10).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Never unlink a live QMP/console endpoint or an unexpected file at its path.
fn prepare_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "machine socket path is not a stale socket",
        ));
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "a machine already owns this socket",
        )),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn startup_window_scales_with_ram_including_partial_gib() {
        assert_eq!(startup_patience(2048), Duration::from_secs(14));
        assert_eq!(startup_patience(8192), Duration::from_secs(26));
        assert_eq!(startup_patience(65536), Duration::from_secs(138));
        assert_eq!(startup_patience(65537), Duration::from_secs(140));
        assert_eq!(startup_patience(1 << 20), Duration::from_secs(2058));
    }

    #[test]
    fn memory_scaled_startup_allows_negotiation_after_ten_seconds() {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qmp.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            writeln!(stream, r#"{{"QMP": {{"version": {{"qemu": {{"major": 10, "minor": 0, "micro": 0}}, "package": ""}}, "capabilities": []}}}}"#).unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["execute"], "qmp_capabilities");
            // QEMU can expose the socket/greeting before memory setup finishes.
            // The larger window must cover negotiation too, not just connect().
            std::thread::sleep(Duration::from_secs(11));
            writeln!(
                stream,
                "{}",
                serde_json::json!({"return": {}, "id": request["id"]})
            )
            .unwrap();
        });
        let connected = Qmp::connect_while(&path, startup_patience(8192), || Ok(true));
        server.join().unwrap();
        connected.unwrap();
    }

    #[test]
    fn launch_does_not_unlink_live_sockets_or_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qmp.sock");
        fs::write(&path, b"keep").unwrap();
        assert!(prepare_socket(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        fs::remove_file(&path).unwrap();
        let listener = UnixListener::bind(&path).unwrap();
        assert!(prepare_socket(&path).is_err());
        drop(listener);
        // Another test may be between fork and exec, temporarily retaining a
        // CLOEXEC copy of the listener. It is correctly treated as live until
        // exec closes that copy; do not assume the close is instantly visible.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match prepare_socket(&path) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::AddrInUse && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("stale socket: {e}"),
            }
        }
        assert!(!path.exists());
    }

    #[test]
    fn failure_log_reads_only_the_bounded_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("qemu.log");
        let bytes = vec![b'x'; 128 << 10];
        fs::write(&path, bytes).unwrap();
        assert_eq!(log_tail(&path).unwrap().len(), 64 << 10);
    }
}
