//! Privileged device guardian. The only privileged operations are module loading,
//! bounded sysfs handoff, device opens and resource limits. QEMU drops to the
//! authenticated caller before exec; no caller-supplied program runs as root.
use super::*;
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::OFlags,
    io::{FdFlags, fcntl_setfd},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, Instant},
};

const MAX_REQUEST: usize = 32 << 10;
const TIMEOUT: Duration = Duration::from_secs(120);
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    parent: u32,
    selected: Vec<Selection>,
    memory_bytes: Option<u64>,
    launch: Option<Launch>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    qemu: PathBuf,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
}
#[derive(Serialize, Deserialize)]
enum Reply {
    Checked,
    Started(u32),
    Error(String),
}

/// Child stdin is the lifetime lease. EOF requests stop; the guardian reaps
/// QEMU before restoring drivers, even if the desktop itself crashes.
pub(crate) struct Started {
    pub child: Child,
    pub control: ChildStdin,
    pub pid: u32,
}
fn spawn(request: &Request, errors: Option<File>) -> Result<(Child, ChildStdin, Reply), String> {
    let data = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    if data.len() >= MAX_REQUEST {
        return Err("Device launch request exceeds 32 KiB.".into());
    }
    let mut command = Command::new("/usr/bin/pkexec");
    command
        .arg("--disable-internal-agent")
        .arg(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--vkore-device-helper")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(errors.map_or(Stdio::null(), Stdio::from));
    let mut child = command.spawn().map_err(|e| {
        format!("NPU authorization needs polkit and a desktop authentication agent: {e}")
    })?;
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let deadline = Instant::now() + TIMEOUT;
    let result = (|| {
        // Nonblocking pipe writes keep the authentication timeout effective even
        // when pkexec is waiting and the request is larger than a pipe buffer.
        rustix::fs::fcntl_setfl(&input, OFlags::NONBLOCK).map_err(|e| e.to_string())?;
        let data = [data.as_slice(), b"\n"].concat();
        let mut offset = 0;
        while offset < data.len() {
            if Instant::now() >= deadline {
                return Err("NPU authorization timed out.".into());
            }
            match input.write(&data[offset..]) {
                Ok(0) => return Err("NPU authorization pipe closed.".into()),
                Ok(n) => offset += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => return Err(format!("NPU authorization was cancelled or failed: {e}")),
            }
        }
        let mut bytes = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err(
                    "NPU authorization timed out; pending preparation will be rolled back.".into(),
                );
            }
            let mut fds = [PollFd::new(&output, PollFlags::IN)];
            poll(
                &mut fds,
                Some(&rustix::event::Timespec {
                    tv_sec: 0,
                    tv_nsec: 100_000_000,
                }),
            )
            .map_err(|e| e.to_string())?;
            if fds[0].revents().is_empty() {
                continue;
            }
            let mut b = [0];
            if output.read(&mut b).map_err(|e| e.to_string())? == 0 {
                return Err("NPU authorization was cancelled, denied, or the helper failed. Check the QEMU log for launch details.".into());
            }
            if b[0] == b'\n' {
                break;
            }
            bytes.push(b[0]);
            if bytes.len() > 4096 {
                return Err("Invalid device-helper response.".into());
            }
        }
        serde_json::from_slice::<Reply>(&bytes).map_err(|e| e.to_string())
    })();
    match result {
        Ok(Reply::Error(error)) | Err(error) => {
            drop(input); // Never kill the guardian during a driver transition.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Err(error)
        }
        Ok(reply) => Ok((child, input, reply)),
    }
}

pub(super) fn authorize(selected: &[Selection], memory_bytes: Option<u64>) -> Result<(), String> {
    let request = Request {
        parent: std::process::id(),
        selected: selected.to_vec(),
        memory_bytes,
        launch: None,
    };
    let (mut child, control, reply) = spawn(&request, None)?;
    drop(control);
    let status = child.wait().map_err(|e| e.to_string())?;
    if matches!(reply, Reply::Checked) && status.success() {
        Ok(())
    } else {
        Err("NPU setup authorization did not finish.".into())
    }
}

pub(crate) fn launch(
    qemu: &Path,
    arguments: Vec<String>,
    selected: &[Selection],
    memory_bytes: u64,
    errors: File,
) -> Result<Started, String> {
    let environment = [
        "HOME",
        "PATH",
        "LANG",
        "DISPLAY",
        "XAUTHORITY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "LD_LIBRARY_PATH",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
    .collect();
    let request = Request {
        parent: std::process::id(),
        selected: selected.to_vec(),
        memory_bytes: Some(memory_bytes),
        launch: Some(Launch {
            qemu: fs::canonicalize(qemu).map_err(|e| e.to_string())?,
            arguments,
            environment,
        }),
    };
    let (mut child, control, reply) = spawn(&request, Some(errors))?;
    if let Reply::Started(pid) = reply {
        return Ok(Started {
            child,
            control,
            pid,
        });
    }
    drop(control);
    let _ = child.wait();
    Err("NPU helper did not start QEMU.".into())
}
pub(super) fn authorize_memory_limit(memory_mib: u32) -> Result<(), String> {
    if !(2048..=1 << 20).contains(&memory_mib) {
        return Err("Invalid memory authorization size.".into());
    }
    let mut child = Command::new("/usr/bin/pkexec")
        .arg("--disable-internal-agent")
        .arg(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--vkore-memory-helper")
        .arg(memory_mib.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            format!("Memory authorization needs polkit and a desktop authentication agent: {e}")
        })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err("Memory authorization was cancelled or denied; retry or turn off memory pinning.".into())
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Memory authorization timed out.".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn memory_entry() -> i32 {
    fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args_os().take(4).collect();
        if args.len() != 3 {
            return Err("Invalid memory helper request.".into());
        }
        let mib: u32 = args[2]
            .to_str()
            .and_then(|s| s.parse().ok())
            .filter(|m| (2048..=1 << 20).contains(m))
            .ok_or("Invalid memory authorization size.")?;
        let parent = rustix::process::getppid().ok_or("Memory caller exited.")?;
        caller(parent.as_raw_nonzero().get() as u32)?;
        // The target is the authenticated parent, never a PID supplied in args.
        // Recheck the parent after reading its credentials.
        if rustix::process::getppid() != Some(parent) {
            return Err("Memory caller changed.".into());
        }
        let budget = (u64::from(mib) + 1024) << 20;
        rustix::process::prlimit(
            Some(parent),
            rustix::process::Resource::Memlock,
            rustix::process::Rlimit {
                current: Some(budget),
                maximum: Some(budget),
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("vKore memory: {e}");
            1
        }
    }
}

fn reply(value: &Reply) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, value).map_err(|e| e.to_string())?;
    out.write_all(b"\n")
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}
pub(super) fn entry() -> i32 {
    if let Err(error) =
        ctrlc::set_handler(|| STOP.store(true, std::sync::atomic::Ordering::Relaxed))
    {
        let _ = reply(&Reply::Error(format!(
            "Cannot install device cleanup signal handler: {error}"
        )));
        return 125;
    }
    match serve() {
        Ok(code) => code,
        Err(error) => {
            let error: String = error.chars().take(512).collect();
            eprintln!("NPU passthrough: {error}");
            let _ = reply(&Reply::Error(error));
            125
        }
    }
}
fn read(path: impl AsRef<Path>) -> Result<String, String> {
    let mut s = String::new();
    File::open(path.as_ref())
        .and_then(|f| f.take(8193).read_to_string(&mut s))
        .map_err(|e| format!("{}: {e}", path.as_ref().display()))?;
    if s.len() > 8192 {
        return Err("Host attribute exceeded its size limit.".into());
    }
    Ok(s.trim().into())
}
fn write(path: impl AsRef<Path>, value: &str) -> Result<(), String> {
    fs::write(path.as_ref(), value).map_err(|e| format!("{}: {e}", path.as_ref().display()))
}
struct Caller {
    uid: u32,
    gid: u32,
    groups: Vec<u32>,
}
fn caller(parent: u32) -> Result<Caller, String> {
    if !rustix::process::geteuid().is_root() {
        return Err("Device helper must be authorized through polkit.".into());
    }
    if rustix::process::getppid().map(|p| p.as_raw_nonzero().get() as u32) != Some(parent) {
        return Err("Device helper caller changed.".into());
    }
    let uid: u32 = std::env::var("PKEXEC_UID")
        .map_err(|_| "Missing polkit caller identity.")?
        .parse()
        .map_err(|_| "Invalid polkit identity.")?;
    if uid == 0 {
        return Err("Run vKore as your normal desktop user, not root.".into());
    }
    let status = read(format!("/proc/{parent}/status"))?;
    let numbers = |key: &str| -> Result<Vec<u32>, String> {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .ok_or_else(|| format!("Missing caller {key}"))?
            .split_whitespace()
            .map(|s| {
                s.parse::<u32>()
                    .map_err(|_| "Invalid caller credentials.".into())
            })
            .collect()
    };
    if numbers("Uid:")? != [uid; 4] {
        return Err("Polkit caller credentials do not match.".into());
    }
    let gids = numbers("Gid:")?;
    if gids.len() != 4 || !gids.iter().all(|g| *g == gids[0]) {
        return Err("Caller has mixed group credentials.".into());
    }
    let groups = numbers("Groups:")?;
    if groups.len() > 256 {
        return Err("Too many caller groups.".into());
    }
    Ok(Caller {
        uid,
        gid: gids[0],
        groups,
    })
}
fn module(name: &str) -> Result<(), String> {
    let mut child = Command::new("/sbin/modprobe")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Cannot load {name}: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "Host kernel cannot load {name}; install/enable VFIO and IOMMUFD support."
                ))
            };
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Loading {name} timed out."));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn host_setup() -> Result<(), String> {
    module("vfio-pci")?;
    module("iommufd")?;
    if read("/sys/module/vfio/parameters/enable_unsafe_noiommu_mode")
        .is_ok_and(|s| s == "Y" || s == "1")
    {
        return Err(
            "Unsafe no-IOMMU VFIO mode is enabled; disable it before using managed passthrough."
                .into(),
        );
    }
    let _ = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/iommu")
        .map_err(|e| format!("IOMMUFD is unavailable on this host: {e}"))?;
    Ok(())
}
fn lease(address: &str) -> Result<File, String> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = Path::new("/run/vkore-vfio");
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    let meta = fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o777 != 0o700 {
        return Err("Invalid device lease directory.".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(OFlags::NOFOLLOW.bits() as i32)
        .open(dir.join(address))
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.uid() != 0 || meta.nlink() != 1 {
        return Err("Invalid device lease file.".into());
    }
    file.try_lock().map_err(|_| {
        format!("NPU {address} is already assigned or being prepared by another vKore session.")
    })?;
    Ok(file)
}
fn device_path(s: &Selection) -> PathBuf {
    Path::new("/sys/bus/pci/devices").join(&s.address)
}
fn reset_method(path: &Path) -> Result<(String, &'static str), String> {
    let original = read(path.join("reset_method"))?;
    let method = ["flr", "af_flr"]
        .into_iter()
        .find(|m| original.split_whitespace().any(|s| s == *m))
        .ok_or(
            "This NPU has no reported function-level reset. Bus-wide resets are not supported.",
        )?;
    Ok((original, method))
}
pub(super) fn check_host(selected: &[Selection]) -> Result<(), String> {
    validate_selections(selected)?;
    for selection in selected {
        reset_method(&device_path(selection))
            .map_err(|e| format!("NPU {}: {e}", selection.address))?;
    }
    Ok(())
}
fn idle(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::FileTypeExt;
    let target = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let mut devices = Vec::new();
    match fs::read_dir("/sys/class/accel") {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
        Ok(entries) => {
            for entry in entries.take(256) {
                let entry = entry.map_err(|e| e.to_string())?;
                if fs::canonicalize(entry.path().join("device")).ok().as_ref() == Some(&target) {
                    let dev = read(entry.path().join("dev"))?;
                    let (major, minor) = dev
                        .split_once(':')
                        .ok_or("Invalid accelerator device number.")?;
                    devices.push(rustix::fs::makedev(
                        major.parse().map_err(|_| "Invalid major number.")?,
                        minor.parse().map_err(|_| "Invalid minor number.")?,
                    ));
                }
            }
        }
    }
    if let Ok(group) = fs::read_link(path.join("iommu_group"))
        && let Some(name) = group.file_name()
        && let Ok(meta) = fs::metadata(Path::new("/dev/vfio").join(name))
        && meta.file_type().is_char_device()
    {
        devices.push(meta.rdev());
    }
    if let Ok(entries) = fs::read_dir(path.join("vfio-dev")) {
        for entry in entries.take(64) {
            let entry = entry.map_err(|e| e.to_string())?;
            if let Ok(meta) = fs::metadata(Path::new("/dev/vfio/devices").join(entry.file_name()))
                && meta.file_type().is_char_device()
            {
                devices.push(meta.rdev());
            }
        }
    }
    if devices.is_empty() {
        return Ok(());
    }
    let mut count = 0usize;
    for (index, process) in fs::read_dir("/proc")
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        if index == 65536 {
            return Err("Host process scan exceeded its limit.".into());
        }
        let process = process.map_err(|e| e.to_string())?;
        if process
            .file_name()
            .to_string_lossy()
            .parse::<u32>()
            .is_err()
        {
            continue;
        }
        let entries = match fs::read_dir(process.path().join("fd")) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Cannot check NPU users: {e}")),
        };
        for fd in entries {
            count += 1;
            if count > 262144 {
                return Err("Host file-descriptor scan exceeded its limit; cannot establish that the NPU is idle.".into());
            }
            let fd = fd.map_err(|e| e.to_string())?;
            let meta = match fs::metadata(fd.path()) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(format!(
                        "Cannot inspect a host process's device handles: {e}"
                    ));
                }
            };
            if meta.file_type().is_char_device() && devices.contains(&meta.rdev()) {
                return Err("The NPU is open in a host process. Close host NPU workloads before starting this guest.".into());
            }
        }
    }
    Ok(())
}
struct SysfsDriver {
    path: PathBuf,
    address: String,
}
impl super::handoff::Driver for SysfsDriver {
    fn current(&self) -> Result<Option<String>, String> {
        match fs::read_link(self.path.join("driver")) {
            Ok(p) => Ok(Some(
                p.file_name()
                    .and_then(|s| s.to_str())
                    .ok_or("Invalid native driver.")?
                    .to_owned(),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set_override(&mut self, value: &str) -> Result<(), String> {
        write(
            self.path.join("driver_override"),
            if value.is_empty() || value == "(null)" {
                "\n"
            } else {
                value
            },
        )
    }
    fn set_reset(&mut self, value: &str) -> Result<(), String> {
        write(self.path.join("reset_method"), value)
    }
    fn unbind(&mut self) -> Result<(), String> {
        write(self.path.join("driver/unbind"), &self.address)
    }
    fn probe(&mut self) -> Result<(), String> {
        write("/sys/bus/pci/drivers_probe", &self.address)
    }
}
struct Bound {
    path: PathBuf,
    address: String,
    handoff: super::handoff::Handoff<SysfsDriver>,
    _lease: File,
}
impl Bound {
    fn inspect(selection: &Selection) -> Result<Self, String> {
        use super::handoff::Driver;
        let lease = lease(&selection.address)?;
        validate_inventory(std::slice::from_ref(selection), &vkore_devices::discover())?;
        let path = device_path(selection);
        let (reset, _) = reset_method(&path)?;
        let driver = SysfsDriver {
            path: path.clone(),
            address: selection.address.clone(),
        };
        let original = driver.current()?;
        let over = read(path.join("driver_override"))?;
        idle(&path)?;
        Ok(Self {
            path,
            address: selection.address.clone(),
            handoff: super::handoff::Handoff::new(driver, original, over, reset),
            _lease: lease,
        })
    }
    fn bind(&mut self) -> Result<File, String> {
        let (_, method) = reset_method(&self.path)?;
        self.handoff.bind(method)?;
        let mut entries = fs::read_dir(self.path.join("vfio-dev")).map_err(|e| {
            format!("VFIO device interface is unavailable; this host needs VFIO cdev support: {e}")
        })?;
        let name = entries
            .next()
            .ok_or("VFIO reported no device interface.")?
            .map_err(|e| e.to_string())?
            .file_name();
        if entries.next().is_some() {
            return Err("Ambiguous VFIO device interface.".into());
        }
        let name = name.to_str().ok_or("Invalid VFIO device name.")?;
        if !name
            .strip_prefix("vfio")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("Invalid VFIO device name.".into());
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(OFlags::NOFOLLOW.bits() as i32)
            .open(Path::new("/dev/vfio/devices").join(name))
            .map_err(|e| format!("Cannot open assigned NPU: {e}"))
    }
    fn restore(&mut self) -> Result<(), String> {
        self.handoff.restore()
    }
}
impl Drop for Bound {
    fn drop(&mut self) {
        if let Err(e) = self.restore() {
            eprintln!(
                "NPU HOST DRIVER RESTORATION FAILED for {}: {e}",
                self.address
            );
        }
    }
}
fn cancelled() -> Result<bool, String> {
    if STOP.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(true);
    }
    let input = std::io::stdin();
    let mut fds = [PollFd::new(&input, PollFlags::IN)];
    poll(
        &mut fds,
        Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    )
    .map_err(|e| e.to_string())?;
    // Any subsequent input or pipe closure means stop. No further privileged
    // commands are accepted over this channel.
    Ok(!fds[0].revents().is_empty())
}
fn serve() -> Result<i32, String> {
    let mut bytes = Vec::new();
    let mut input = std::io::stdin();
    loop {
        let mut b = [0];
        if input.read(&mut b).map_err(|e| e.to_string())? == 0 {
            return Err("Caller closed the request.".into());
        }
        if b[0] == b'\n' {
            break;
        }
        bytes.push(b[0]);
        if bytes.len() > MAX_REQUEST {
            return Err("Device request is too large.".into());
        }
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let caller = caller(request.parent)?;
    validate_selections(&request.selected)?;
    if request.selected.is_empty() {
        return Err("No NPU was selected.".into());
    }
    host_setup()?;
    let mut bound = Vec::new();
    // Lock all devices before any handoff, in a deterministic order.
    let mut selected = request.selected.clone();
    selected.sort_by(|a, b| a.address.cmp(&b.address));
    for selection in &selected {
        bound.push(Bound::inspect(selection)?);
    }
    if let Some(budget) = request.memory_bytes {
        if !(3 << 30..=(1024u64 * 1024 + 1024) << 20).contains(&budget) {
            return Err("Invalid locked-memory request.".into());
        }
        // Apply the budget only to this guardian and its future unprivileged
        // child. Never change a caller-supplied PID's resource limits.
        rustix::process::setrlimit(
            rustix::process::Resource::Memlock,
            rustix::process::Rlimit {
                current: Some(budget),
                maximum: Some(budget),
            },
        )
        .map_err(|e| e.to_string())?;
    }
    let Some(mut launch) = request.launch else {
        reply(&Reply::Checked)?;
        return Ok(0);
    };
    if !launch.qemu.is_absolute() || launch.arguments.len() > 512 || launch.environment.len() > 32 {
        return Err("Invalid QEMU launch request.".into());
    }
    // Validate the complete mapping before touching any host driver.
    fd_arguments(
        &mut launch.arguments.clone(),
        &request.selected,
        &(3..3 + request.selected.len() as i32 + 1).collect::<Vec<_>>(),
    )?;
    if cancelled()? {
        return Err("NPU launch was cancelled.".into());
    }
    let iommu = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/iommu")
        .map_err(|e| e.to_string())?;
    let mut fds = vec![iommu];
    for selection in &request.selected {
        let device = bound
            .iter_mut()
            .find(|b| b.address == selection.address)
            .unwrap();
        fds.push(device.bind()?);
    }
    // The generated argument layout is verified before any privileged FD is
    // inherited. It remains safe if a caller substitutes its own executable:
    // that executable receives only the explicitly authorized device handles,
    // and runs as the caller, never as root.
    fd_arguments(
        &mut launch.arguments,
        &request.selected,
        &fds.iter().map(AsRawFd::as_raw_fd).collect::<Vec<_>>(),
    )?;
    if cancelled()? {
        return Err("NPU launch was cancelled.".into());
    }
    let mut command = Command::new(&launch.qemu);
    command
        .args(&launch.arguments)
        .env_clear()
        .envs(launch.environment)
        .current_dir("/")
        .uid(caller.uid)
        .gid(caller.gid)
        .groups(&caller.groups)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .stderr(Stdio::inherit());
    // SAFETY: this child-only hook performs only prctl and fcntl syscalls.
    // All allocation and credential setup is prepared before fork. The parent's
    // FDs retain CLOEXEC, so concurrent parent launches cannot inherit them.
    unsafe {
        command.pre_exec(move || {
            rustix::thread::set_no_new_privs(true)?;
            for fd in &fds {
                fcntl_setfd(fd, FdFlags::empty())?;
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot launch unprivileged QEMU: {e}"))?;
    drop(command); // Closes guardian copies of every VFIO FD captured above.
    if let Err(e) = reply(&Reply::Started(child.id())) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let status = super::supervision::supervise(child, std::io::stdin(), || {
        STOP.load(std::sync::atomic::Ordering::Relaxed)
    })?;
    let mut errors = Vec::new();
    for device in bound.iter_mut().rev() {
        if let Err(e) = device.restore() {
            errors.push(e);
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "Host driver restoration failed: {}",
            errors.join("; ")
        ));
    }
    Ok(status.code().unwrap_or(1))
}
