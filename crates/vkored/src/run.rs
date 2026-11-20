//! A machine's boot, from preflight to what it changed.
//!
//! 1. **Preflight** probes what this host and its QEMU realize, finds UEFI
//!    firmware, and inspects the ISO: its SHA-256 (recorded, and reported
//!    when it changed since the machine last booted it) and the paging mode
//!    Limine will require, so a machine this host cannot boot is refused in
//!    words before QEMU starts.
//! 2. **Staging** puts the configured project into a terminal's home through
//!    Kore's own engine, while the disk is offline.
//! 3. **Supervision** follows the boot through Kore's serial witnesses and
//!    QEMU's events, appending each to the boot's transcript.
//! 4. **Stop and collect**: the power button, then QEMU's end if the guest
//!    does not answer; then what the machine changed in the project is read
//!    back and, under the project's writeback policy and bounds, applied.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::{Value, json};
use vkore_vm::{
    Accel, BootMedium, Display, Firmware, HostCaps, Machine, MachineEvent, MachineSpec, Paging,
    QemuCaps, Stage, StopOutcome, Vendor, firmware, machine::private_vars,
};
use vkore_volume::{
    Disk,
    workspace::{self, Applied, ApplyLimits, Changes, Manifest, StageOptions},
};

use crate::{
    config::{MachineConfig, MachineStore, StoreError, Writeback, broad_root_warning},
    state::{MachinePaths, private_dir},
};

/// The blank stick Kore's input enumeration expects beside its keyboard.
const USB_STICK_BYTES: u64 = 16 << 20;

/// What a boot could not do.
#[derive(Debug)]
pub enum RunError {
    Store(StoreError),
    /// Something this host cannot provide (QEMU, firmware, acceleration).
    Host(String),
    Media(String),
    Machine(String),
    Volume(vkore_volume::Error),
    Io(PathBuf, io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(f, "{error}"),
            Self::Host(message) | Self::Media(message) | Self::Machine(message) => {
                f.write_str(message)
            }
            Self::Volume(error) => write!(f, "{error}"),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for RunError {}

impl From<StoreError> for RunError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<vkore_volume::Error> for RunError {
    fn from(error: vkore_volume::Error) -> Self {
        Self::Volume(error)
    }
}

/// What a machine will boot on, established before it boots.
#[derive(Clone, Debug, Serialize)]
pub struct Preflight {
    pub qemu: PathBuf,
    pub qemu_caps: QemuCaps,
    pub host: HostCaps,
    pub vendor: Vendor,
    pub accel: Accel,
    pub firmware: Firmware,
    pub medium: BootMedium,
    /// The ISO's digest differs from the one this machine last booted.
    pub iso_changed: bool,
    pub workspace_warning: Option<String>,
}

/// One thing that happened during a boot.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Preflight(Box<Preflight>),
    Staged {
        files: usize,
        directories: usize,
        skipped: usize,
        generation: u64,
    },
    Started {
        pid: u32,
    },
    /// A console image's shell is on COM2, reachable at `socket` while the
    /// machine runs.
    Console {
        socket: PathBuf,
    },
    Stage {
        stage: Stage,
        uptime_ms: u128,
    },
    Qemu {
        name: String,
        data: Value,
    },
    Stopping,
    Stopped {
        forced: bool,
    },
    Exited {
        code: Option<i32>,
    },
    Changes {
        created: usize,
        modified: usize,
        deleted: usize,
        bytes: u64,
    },
    Applied(Applied),
    Warning {
        message: String,
    },
}

/// Explicit QEMU override, packaged runtime, then the host PATH.
pub fn default_qemu() -> Option<PathBuf> {
    if let Some(qemu) = std::env::var_os("VKORE_QEMU") {
        return Some(qemu.into());
    }
    if let Some(qemu) = std::env::current_exe().ok().and_then(|exe| {
        vkore_vm::runtime::bundled_qemu(
            &exe,
            std::env::var_os("VKORE_QEMU_BUNDLE").map(PathBuf::from),
        )
    }) {
        return Some(qemu);
    }
    let name = "qemu-system-x86_64";
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Establish what `config` will boot on `qemu`, without starting it.
pub fn preflight(config: &MachineConfig, qemu: &Path) -> Result<Preflight, RunError> {
    crate::config::validate(config)?;
    let host_resources =
        vkore_vm::hardware::HostResources::probe(config.iso.parent().unwrap_or(Path::new("/")))
            .map_err(RunError::Host)?;
    crate::hardware::validate_host(config, &host_resources).map_err(RunError::Host)?;
    crate::hardware::assigned_devices(config, &host_resources).map_err(RunError::Host)?;
    if !crate::hardware::has_devices(config)
        && crate::hardware::needs_authorization(config, &host_resources)
    {
        return Err(RunError::Host("memory pinning needs authorization: finish hardware setup with Done/Save hardware, or turn off memory pinning".into()));
    }
    let runtime = vkore_vm::runtime::Runtime::for_qemu(qemu)
        .map_err(|error| RunError::Host(format!("QEMU runtime: {error}")))?;
    let host = HostCaps::probe();
    let vendor = config.vendor.unwrap_or_else(|| host.native_vendor());
    let accel = host
        .choose(config.accel, vendor)
        .map_err(|error| RunError::Host(error.to_string()))?;
    let qemu_caps =
        QemuCaps::probe(qemu, vendor, accel).map_err(|error| RunError::Host(error.to_string()))?;
    if crate::hardware::has_devices(config)
        && (accel != vkore_vm::Accel::Kvm || !qemu_caps.vfio_iommufd)
    {
        return Err(RunError::Host(
            "Accelerator passthrough requires Linux KVM and QEMU with VFIO device-FD and IOMMUFD support."
                .into(),
        ));
    }
    let firmware = match &config.firmware {
        Some(chosen) => Firmware {
            descriptor: None,
            code: chosen.code.clone(),
            vars_template: chosen.vars_template.clone(),
            format: chosen.format.clone(),
        },
        None => runtime
            .map(|r| r.firmware)
            .or_else(firmware::discover)
            .ok_or_else(|| {
                RunError::Host(
                    "no UEFI firmware for x86_64 q35 without Secure Boot among QEMU's firmware \
                 descriptors (install OVMF/edk2, or name one with --firmware-code and \
                 --firmware-vars)"
                        .into(),
                )
            })?,
    };
    let medium =
        BootMedium::inspect(&config.iso).map_err(|error| RunError::Media(error.to_string()))?;
    if medium.paging == Some(Paging::FiveLevel) && !qemu_caps.la57 {
        return Err(RunError::Host(format!(
            "{} boots five-level paging (4k57), and this QEMU cannot realize LA57 on {accel:?}",
            config.iso.display()
        )));
    }
    let iso_changed = config
        .iso_sha256
        .as_ref()
        .is_some_and(|recorded| *recorded != medium.sha256);
    let workspace_warning = config
        .workspace
        .as_ref()
        .and_then(|workspace| broad_root_warning(&workspace.root));
    Ok(Preflight {
        qemu: qemu.to_path_buf(),
        qemu_caps,
        host,
        vendor,
        accel,
        firmware,
        medium,
        iso_changed,
        workspace_warning,
    })
}

/// One booting (or booted) machine.
pub struct Session {
    config: MachineConfig,
    paths: MachinePaths,
    machine: Machine,
    transcript: File,
    reported: Option<Stage>,
    // Drop after Machine: never release the lease while QEMU is still stopping.
    _lease: File,
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

impl Session {
    /// Preflight, stage and boot machine `name`, reporting each step to
    /// `sink`.
    pub fn start(
        store: &MachineStore,
        name: &str,
        qemu: &Path,
        display: Display,
        sink: &mut dyn FnMut(&Event),
    ) -> Result<Self, RunError> {
        store.get(name)?;
        let paths = store.state().machine(name);
        let lease = crate::hardware::lease(&paths.dir)?;
        let mut config = store.get(name)?;
        let checked = preflight(&config, qemu)?;
        let paging = checked.medium.paging.unwrap_or(Paging::FourLevel);
        private_dir(&paths.runs).map_err(|error| RunError::Io(paths.runs.clone(), error))?;
        private_dir(&paths.scratch).map_err(|error| RunError::Io(paths.scratch.clone(), error))?;
        let transcript_path = paths.runs.join(format!("{}.jsonl", unix_ms()));
        let transcript = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&transcript_path)
            .map_err(|error| RunError::Io(transcript_path.clone(), error))?;
        let mut record = Recorder { transcript, sink };
        record.emit(&Event::Preflight(Box::new(checked.clone())));
        if checked.iso_changed {
            record.emit(&Event::Warning {
                message: format!(
                    "{} changed since this machine last booted it (now {})",
                    config.iso.display(),
                    checked.medium.sha256
                ),
            });
        }
        if let Some(warning) = &checked.workspace_warning {
            record.emit(&Event::Warning {
                message: format!("workspace: {warning}"),
            });
        }
        if config.iso_sha256.as_deref() != Some(checked.medium.sha256.as_str()) {
            config.iso_sha256 = Some(checked.medium.sha256.clone());
            store.save(&config)?;
        }

        // The machine's own media, created once.
        let disk = if paths.disk.exists() {
            Disk::new(&paths.disk)?
        } else if config.resources.reserve_storage {
            let host = vkore_vm::hardware::HostResources::probe(store.state().root())
                .map_err(RunError::Host)?;
            if config.disk_mib > host.storage_available_mib {
                return Err(RunError::Host(
                    "not enough free storage for the machine disk".into(),
                ));
            }
            Disk::create_reserved_image(&paths.disk, config.disk_mib << 20)?
        } else {
            Disk::create_image(&paths.disk, config.disk_mib << 20)?
        };
        if !paths.usb_stick.exists() {
            let stick = File::create(&paths.usb_stick)
                .map_err(|error| RunError::Io(paths.usb_stick.clone(), error))?;
            stick
                .set_len(USB_STICK_BYTES)
                .map_err(|error| RunError::Io(paths.usb_stick.clone(), error))?;
        }
        if !paths.vars.exists() {
            private_vars(&checked.firmware.vars_template, &paths.vars)
                .map_err(|error| RunError::Io(paths.vars.clone(), error))?;
        }

        if config.screenshots_folder
            && let Some(generation) = ensure_screenshots_folder(&disk, &paths.scratch)?
        {
            record.emit(&Event::Warning {
                message: format!(
                    "created {} in terminal 0's home so PrintScreen saves (generation {generation})",
                    vkore_volume::SCREENSHOTS
                ),
            });
        }
        if let Some(project) = &config.workspace {
            let mut options = StageOptions::new(project.name.clone());
            options.face = project.face;
            options.exclude = project.exclude.iter().cloned().collect();
            let manifest = workspace::stage(&disk, &project.root, &paths.scratch, &options)?;
            manifest.save(&paths.manifest)?;
            record.emit(&Event::Staged {
                files: manifest.files.len(),
                directories: manifest.directories.len(),
                skipped: manifest.skipped.len(),
                generation: manifest.generation,
            });
        }

        let mut resources = config.resources.clone();
        let host_resources =
            vkore_vm::hardware::HostResources::probe(&paths.dir).map_err(RunError::Host)?;
        resources.passthrough =
            crate::hardware::assigned_devices(&config, &host_resources).map_err(RunError::Host)?;
        resources.gpu_vfs.clear(); // MachineSpec contains resolved VF identities only.
        let spec = MachineSpec {
            name: config.name.clone(),
            iso: config.iso.clone(),
            disk: paths.disk.clone(),
            usb_stick: paths.usb_stick.clone(),
            firmware_code: checked.firmware.code.clone(),
            firmware_vars: paths.vars.clone(),
            firmware_format: checked.firmware.format.clone(),
            vendor: checked.vendor,
            accel: checked.accel,
            paging,
            cpus: config.cpus,
            memory_mib: config.memory_mib,
            resources,
            display,
            framebuffer: config.framebuffer,
            serial_log: paths.serial.clone(),
            debugcon_log: paths.debugcon.clone(),
            qmp_socket: paths.qmp.clone(),
            console_socket: checked.medium.console.then(|| paths.console.clone()),
        };
        let machine = Machine::start(qemu, &checked.qemu_caps, &spec, &paths.qemu_log)
            .map_err(|error| RunError::Machine(error.to_string()))?;
        record.emit(&Event::Started { pid: machine.id() });
        if let Some(socket) = &spec.console_socket {
            record.emit(&Event::Console {
                socket: socket.clone(),
            });
        }
        let Recorder { transcript, .. } = record;
        Ok(Self {
            _lease: lease,
            config,
            paths,
            machine,
            transcript,
            reported: None,
        })
    }

    pub fn config(&self) -> &MachineConfig {
        &self.config
    }

    pub fn paths(&self) -> &MachinePaths {
        &self.paths
    }

    pub fn machine(&mut self) -> &mut Machine {
        &mut self.machine
    }

    pub fn stage(&self) -> Stage {
        self.machine.readiness().stage()
    }

    /// Follow the machine for up to `wait`; `false` once QEMU has exited.
    pub fn poll(&mut self, wait: Duration, sink: &mut dyn FnMut(&Event)) -> Result<bool, RunError> {
        let events = self
            .machine
            .poll(wait)
            .map_err(|error| RunError::Machine(error.to_string()))?;
        let mut record = Recorder {
            transcript: &mut self.transcript,
            sink,
        };
        let mut running = true;
        for event in events {
            match event {
                MachineEvent::Stage(stage) if self.reported != Some(stage) => {
                    self.reported = Some(stage);
                    record.emit(&Event::Stage {
                        stage,
                        uptime_ms: self.machine.uptime().as_millis(),
                    });
                }
                MachineEvent::Stage(_) => {}
                MachineEvent::Qemu(name, data) => record.emit(&Event::Qemu { name, data }),
                MachineEvent::Exited(code) => {
                    running = false;
                    record.emit(&Event::Exited { code });
                }
            }
        }
        Ok(running && self.machine.running())
    }

    /// Ask the guest to power off; end QEMU after `grace`.
    pub fn stop(
        &mut self,
        grace: Duration,
        sink: &mut dyn FnMut(&Event),
    ) -> Result<StopOutcome, RunError> {
        let mut record = Recorder {
            transcript: &mut self.transcript,
            sink,
        };
        record.emit(&Event::Stopping);
        let outcome = self
            .machine
            .stop(grace)
            .map_err(|error| RunError::Machine(error.to_string()))?;
        if outcome == StopOutcome::Forced {
            record.emit(&Event::Warning {
                message: format!(
                    "the guest did not power off within {} s of the power button; QEMU was \
                     ended (the volume keeps its last committed state)",
                    grace.as_secs()
                ),
            });
        }
        record.emit(&Event::Stopped {
            forced: outcome == StopOutcome::Forced,
        });
        Ok(outcome)
    }

    /// After QEMU has exited: collect what the machine changed in its
    /// project and, when the policy is automatic, apply it within bounds.
    pub fn finish(
        mut self,
        store: &MachineStore,
        sink: &mut dyn FnMut(&Event),
    ) -> Result<Option<Changes>, RunError> {
        if self.machine.running() {
            return Err(RunError::Machine("the machine is still running".into()));
        }
        let name = self.config.name.clone();
        drop(self.machine);
        let mut record = Recorder {
            transcript: &mut self.transcript,
            sink,
        };
        let Some(project) = self.config.workspace.clone() else {
            return Ok(None);
        };
        if project.writeback == Writeback::Off {
            return Ok(None);
        }
        let changes = collect(store, &name)?;
        record.emit(&Event::Changes {
            created: changes.created.len(),
            modified: changes.modified.len(),
            deleted: changes.deleted.len(),
            bytes: changes.bytes(),
        });
        if project.writeback == Writeback::Automatic && !changes.is_empty() {
            let applied = apply(store, &name, &changes)?;
            record.emit(&Event::Applied(applied));
        }
        Ok(Some(changes))
    }
}

/// Create `pictures/screenshots` in terminal 0's home when it is missing,
/// formatting a blank image first as its first boot would; answer the
/// generation that names it, or `None` when it was there.
fn ensure_screenshots_folder(disk: &Disk, scratch: &Path) -> Result<Option<u64>, RunError> {
    use vkore_volume::{Browser, Listing, VolPath};
    let folder = VolPath::parse(0, vkore_volume::SCREENSHOTS)?;
    if disk.is_blank()? {
        if disk.kind() != vkore_volume::DiskKind::Image {
            return Ok(None);
        }
        disk.format()?;
    }
    let present = {
        let browser = Browser::open(disk)?;
        matches!(browser.list(&folder), Ok(Listing::Directory { .. }))
    };
    if present {
        return Ok(None);
    }
    let empty = scratch.join(format!("empty-{}", std::process::id()));
    private_dir(&empty).map_err(|error| RunError::Io(empty.clone(), error))?;
    let installed = disk.install(&empty, &folder, false);
    let _ = fs::remove_dir(&empty);
    Ok(Some(installed?.generation))
}

/// What machine `name` changed in its staged project since staging. The
/// machine must be stopped.
pub fn collect(store: &MachineStore, name: &str) -> Result<Changes, RunError> {
    let paths = store.state().machine(name);
    let manifest = Manifest::load(&paths.manifest)?;
    private_dir(&paths.scratch).map_err(|error| RunError::Io(paths.scratch.clone(), error))?;
    Ok(workspace::collect(
        &Disk::new(&paths.disk)?,
        &manifest,
        &paths.scratch,
    )?)
}

/// Apply `changes` to machine `name`'s project under its bounds.
pub fn apply(store: &MachineStore, name: &str, changes: &Changes) -> Result<Applied, RunError> {
    let config = store.get(name)?;
    let project = config
        .workspace
        .ok_or_else(|| RunError::Machine(format!("{name} stages no project")))?;
    let manifest = Manifest::load(&store.state().machine(name).manifest)?;
    Ok(workspace::apply(
        changes,
        &manifest,
        ApplyLimits {
            max_files: project.max_files,
            max_bytes: project.max_bytes,
        },
    )?)
}

/// Each event to the caller and, as one JSON line, to the boot's transcript.
struct Recorder<'a, T: Write> {
    transcript: T,
    sink: &'a mut dyn FnMut(&Event),
}

impl<T: Write> Recorder<'_, T> {
    fn emit(&mut self, event: &Event) {
        (self.sink)(event);
        let mut line = json!({ "unix_ms": unix_ms().to_string() });
        if let Ok(Value::Object(fields)) = serde_json::to_value(event) {
            for (key, value) in fields {
                line[key] = value;
            }
        }
        // The transcript is diagnostic: a full disk must not stop a boot.
        let _ = writeln!(self.transcript, "{line}");
    }
}

/// Remove machine `name`'s transient files that a crash may leave behind.
pub fn tidy(paths: &MachinePaths) {
    let _ = fs::remove_file(&paths.qmp);
}
