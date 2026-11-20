//! `vkore`: Kore machines and their volumes, from a Linux or macOS host.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use vkore_vm::{AccelChoice, BootMedium, Display, HostCaps, QemuCaps, Stage, Vendor, firmware};
use vkore_volume::{
    Browser, Disk, EntryKind, ExportOptions, Listing, Preview, VolPath, preview,
    workspace::{Changes, DEFAULT_EXCLUDES},
};
use vkored::{
    Event, MachineConfig, MachineStore, State, WorkspaceConfig, Writeback,
    config::FirmwareConfig,
    run::{self, Session},
};

#[derive(Parser)]
#[command(
    name = "vkore",
    version,
    about = "Kore machines and their volumes, from a Linux or macOS host",
    long_about = "Boot Kore ISOs under QEMU with the topology Kore's own production gate boots, \
                  stage projects into a terminal's home, and read and write Kore's DagFS volumes \
                  through Kore's own engine (libkore)."
)]
struct Cli {
    /// vKore's state directory [default: $VKORE_STATE, $XDG_STATE_HOME/vkore, ~/.local/state/vkore]
    #[arg(long, global = true)]
    state: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// What this host and its QEMU can boot
    Probe {
        #[arg(long)]
        qemu: Option<PathBuf>,
    },
    /// Discover host CPU, memory and storage resources
    Hardware,
    /// Inspect a Kore ISO: its SHA-256 and the paging mode it boots
    Iso { iso: PathBuf },
    /// Kore machines
    #[command(subcommand)]
    Machine(MachineCommand),
    /// A Kore volume: a raw disk image, a disk's block device (Linux), or @MACHINE
    Volume(VolumeArgs),
    /// Run the local OpenAI-compatible API (guest backend not connected yet)
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: std::net::SocketAddr,
    },
}

#[derive(Subcommand)]
enum MachineCommand {
    /// List public official images available to download
    Images,
    /// Create a machine that boots ISO
    Create(Box<CreateArgs>),
    /// List machines
    List,
    /// Show a machine's configuration
    Show { name: String },
    /// Boot a machine in the foreground; Ctrl-C asks it to stop
    Run(RunArgs),
    /// What a stopped machine changed in its staged project
    Changes { name: String },
    /// Apply what a stopped machine changed, under its project's bounds
    Apply { name: String },
    /// Remove a machine and its disk
    Remove {
        name: String,
        /// Confirm removing the machine's disk and everything on it
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum VendorArg {
    Intel,
    Amd,
}

#[derive(Clone, Copy, ValueEnum)]
enum AccelArg {
    Auto,
    Kvm,
    Hvf,
    Tcg,
}

#[derive(Clone, Copy, ValueEnum)]
enum WritebackArg {
    Off,
    Review,
    Automatic,
}

#[derive(Args)]
struct CreateArgs {
    name: String,
    /// A local Kore x86_64 ISO or an HTTPS download link
    #[arg(long, required_unless_present = "image", conflicts_with = "image")]
    iso: Option<String>,
    /// Official image ID from `vkore machine images`
    #[arg(long, conflicts_with_all = ["iso", "sha256"])]
    image: Option<String>,
    /// Expected SHA-256 of the image, checked before creating the machine
    #[arg(long, requires = "iso")]
    sha256: Option<String>,
    /// Virtual disk capacity in MiB (defaults to 32 GiB)
    #[arg(long, default_value_t = vkored::hardware::DEFAULT_DISK_MIB)]
    disk_mib: u64,
    /// Defaults to half the physical cores, at least two
    #[arg(long)]
    cpus: Option<u16>,
    /// Defaults to half physical memory, at least 2048 MiB
    #[arg(long)]
    memory_mib: Option<u32>,
    #[arg(long)]
    no_cpu_pinning: bool,
    /// Require native MONITOR/MWAIT (enabled automatically for supported pinned KVM guests)
    #[arg(long = "cpu-pm", conflicts_with_all = ["no_cpu_pinning", "no_cpu_power_management"])]
    cpu_power_management: bool,
    /// Disable automatic native CPU waits
    #[arg(long = "no-cpu-pm")]
    no_cpu_power_management: bool,
    #[arg(long)]
    no_memory_pinning: bool,
    /// Host logical CPU IDs, in guest CPU order
    #[arg(long, value_delimiter = ',')]
    host_cpus: Vec<u32>,
    /// Physical page size; defaults to 2m when a usable pool is available
    #[arg(long, value_parser = ["system", "2m", "1g"])]
    pages: Option<String>,
    /// Use a sparse disk instead of reserving space at first launch
    #[arg(long)]
    sparse: bool,
    /// Pass this host NPU through whole, by PCI address (repeat; see
    /// `vkore hardware`). Its identity is recorded and rechecked at launch,
    /// which asks for authorization to hand the device over.
    #[arg(long, value_name = "PCI_ADDRESS")]
    passthrough: Vec<String>,
    /// Guest RAM layout; single keeps all RAM together for inference
    #[arg(long, value_parser = ["single", "split"], default_value = "single")]
    numa: String,
    /// GPU physical-function PCI address and one-based VF number, e.g. 0000:00:02.0/1
    /// Provision absent VFs using Done/Save hardware in the desktop before launch.
    #[arg(long, value_name = "PF_ADDRESS/VF_NUMBER")]
    gpu_vf: Vec<String>,
    /// The IOMMU vendor [default: this host's, so it can accelerate]
    #[arg(long, value_enum)]
    vendor: Option<VendorArg>,
    #[arg(long, value_enum, default_value = "auto")]
    accel: AccelArg,
    /// The framebuffer mode, WIDTHxHEIGHT
    #[arg(long, default_value = "1280x800")]
    framebuffer: String,
    /// A host project to stage into the terminal's home at each boot
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// The project's directory name in the home [default: the project's own]
    #[arg(long)]
    workspace_name: Option<String>,
    /// The terminal whose home receives the project
    #[arg(long, default_value_t = 0)]
    face: u8,
    /// Names left out at any depth (repeat; replaces the defaults)
    #[arg(long)]
    exclude: Vec<String>,
    #[arg(long, value_enum, default_value = "review")]
    writeback: WritebackArg,
    #[arg(long, default_value_t = 256)]
    max_files: usize,
    #[arg(long, default_value_t = 64 << 20)]
    max_bytes: u64,
    /// UEFI firmware code, when not found through QEMU's descriptors
    #[arg(long, requires = "firmware_vars")]
    firmware_code: Option<PathBuf>,
    #[arg(long, requires = "firmware_code")]
    firmware_vars: Option<PathBuf>,
    #[arg(long, default_value = "raw")]
    firmware_format: String,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Until {
    /// Until the kernel's console is up
    Kernel,
    /// Until the desktop's shell answers
    Shell,
    /// Until applications are seeded and homes exist
    Ready,
}

#[derive(Args)]
struct RunArgs {
    name: String,
    #[arg(long)]
    qemu: Option<PathBuf>,
    /// window (QEMU's own), headless, or vnc=SOCKET
    #[arg(long, default_value = "window")]
    display: String,
    /// Stop the machine this long after it reaches --until
    #[arg(long)]
    stop_after: Option<u64>,
    #[arg(long, value_enum, default_value = "ready")]
    until: Until,
    /// Fail if --until is not reached this long after boot starts
    #[arg(long, default_value_t = 900)]
    timeout: u64,
    /// Seconds the guest has to power off before QEMU is ended
    #[arg(long, default_value_t = 10)]
    grace: u64,
    /// Press PrintScreen in the guest when it reaches --until
    ///
    /// Kore writes the capture after the press, and a stop does not wait for
    /// it: with --stop-after, leave it the time (under TCG, several seconds).
    #[arg(long)]
    screenshot: bool,
    /// Write the guest's screen as PPM to this path when it reaches --until
    #[arg(long)]
    screendump: Option<PathBuf>,
    /// Print events as JSON lines
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct VolumeArgs {
    /// A raw image, a block device (Linux), or @MACHINE
    disk: String,
    /// The terminal whose home relative paths name
    #[arg(long, default_value_t = 0, global = true)]
    face: u8,
    /// Read the disk with this logical block size (512 or 4096)
    #[arg(long, global = true)]
    block_size: Option<u32>,
    #[command(subcommand)]
    command: VolumeCommand,
}

#[derive(Subcommand)]
enum VolumeCommand {
    /// The volume's state
    Info,
    /// A directory's entries, or a file's length
    Ls {
        path: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Everything below a directory
    Tree { path: Option<String> },
    /// A file's bytes, to standard output
    Cat { path: String },
    /// Copy a file or directory out to a host directory
    Get {
        path: String,
        destination: PathBuf,
        #[arg(long)]
        replace: bool,
    },
    /// Install a host file or directory below the terminal's home
    Put {
        source: PathBuf,
        path: String,
        #[arg(long)]
        replace: bool,
    },
    /// Remove a path below the terminal's home
    Rm { path: String },
    /// Format a blank image as Kore's first boot would
    Format,
    /// Kore screen captures in the terminal's home, newest first
    Shots {
        /// Copy them into this host directory
        #[arg(long)]
        get: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

type Failure = String;

fn main() -> ExitCode {
    if let Some(code) = vkore_vm::passthrough::helper_entry() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let cli = Cli::parse();
    match run_cli(cli) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("vkore: {message}");
            ExitCode::FAILURE
        }
    }
}

fn store(state: Option<PathBuf>) -> Result<MachineStore, Failure> {
    let root = state
        .or_else(State::default_root)
        .ok_or("no state directory: set $HOME, $XDG_STATE_HOME or --state")?;
    let state = State::open(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    Ok(MachineStore::new(state))
}

fn qemu(explicit: Option<PathBuf>) -> Result<PathBuf, Failure> {
    explicit
        .or_else(run::default_qemu)
        .ok_or_else(|| "qemu-system-x86_64 is not on PATH (install QEMU, or pass --qemu)".into())
}

fn run_cli(cli: Cli) -> Result<ExitCode, Failure> {
    match cli.command {
        Command::Probe { qemu: path } => probe(qemu(path)?),
        Command::Hardware => {
            let store = store(cli.state)?;
            let host = vkore_vm::hardware::HostResources::probe(store.state().root())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&host).map_err(|e| e.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Iso { iso } => {
            let medium = BootMedium::inspect(&iso).map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&medium).unwrap_or_default()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Machine(command) => machine(store(cli.state)?, command),
        Command::Volume(arguments) => volume(cli.state, arguments),
        Command::Serve { listen } => {
            let interrupted = Arc::new(AtomicBool::new(false));
            let signal = interrupted.clone();
            ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
                .map_err(|e| e.to_string())?;
            let server =
                vkored::inference::Server::start(listen, Arc::new(vkored::inference::Unavailable))?;
            println!("OpenAI-compatible API: http://{}/v1", server.address);
            println!("No guest inference backend is connected; chat requests return HTTP 503.");
            while !interrupted.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(server);
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn probe(qemu: PathBuf) -> Result<ExitCode, Failure> {
    let runtime = vkore_vm::runtime::Runtime::for_qemu(&qemu)
        .map_err(|error| format!("QEMU runtime: {error}"))?;
    let host = HostCaps::probe();
    let vendor = host.native_vendor();
    let accel = host
        .choose(AccelChoice::Auto, vendor)
        .map_err(|error| error.to_string())?;
    let caps = QemuCaps::probe(&qemu, vendor, accel).map_err(|error| error.to_string())?;
    let firmware = runtime
        .map(|runtime| runtime.firmware)
        .or_else(firmware::discover);
    let report = serde_json::json!({
        "qemu": qemu,
        "host": host,
        "vendor": vendor,
        "accel": accel,
        "qemu_caps": caps,
        "firmware": firmware,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    if firmware.is_none() {
        eprintln!("vkore: no bootable UEFI firmware found among QEMU's firmware descriptors");
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

fn machine(store: MachineStore, command: MachineCommand) -> Result<ExitCode, Failure> {
    match command {
        MachineCommand::Images => {
            for image in vkored::catalog::official()?.images() {
                println!(
                    "{}  {} ({})  {:.1} MiB",
                    image.id,
                    image.name,
                    image.version,
                    image.bytes() as f64 / (1024. * 1024.)
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        MachineCommand::Create(arguments) => {
            let source = arguments.iso.clone();
            let image = arguments.image.clone();
            let digest = arguments.sha256.clone();
            let config = create_config(*arguments, store.state().root())?;
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = cancelled.clone();
            ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
                .map_err(|e| e.to_string())?;
            let progress = |progress: vkored::images::Progress| {
                eprintln!(
                    "{}: {} MiB{}",
                    progress.phase.label(),
                    progress.bytes >> 20,
                    progress
                        .total
                        .map(|n| format!(" / {} MiB", n >> 20))
                        .unwrap_or_default()
                );
            };
            let config = if let Some(id) = image {
                vkored::images::create_official(&store, config, &id, None, &cancelled, progress)?
            } else {
                vkored::images::create(
                    &store,
                    config,
                    source.as_deref().expect("clap requires an image source"),
                    digest.as_deref(),
                    &cancelled,
                    progress,
                )?
            };
            println!(
                "created machine {} (disk is created at its first boot)",
                config.name
            );
            Ok(ExitCode::SUCCESS)
        }
        MachineCommand::List => {
            for config in store.list().map_err(|error| error.to_string())? {
                println!(
                    "{:<20} {} CPUs, {} MiB, {}{}",
                    config.name,
                    config.cpus,
                    config.memory_mib,
                    config.iso.display(),
                    config
                        .workspace
                        .as_ref()
                        .map(|workspace| format!(", project {}", workspace.root.display()))
                        .unwrap_or_default()
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        MachineCommand::Show { name } => {
            let config = store.get(&name).map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_default()
            );
            Ok(ExitCode::SUCCESS)
        }
        MachineCommand::Run(arguments) => run_machine(&store, arguments),
        MachineCommand::Changes { name } => {
            let changes = run::collect(&store, &name).map_err(|error| error.to_string())?;
            print_changes(&changes);
            Ok(ExitCode::SUCCESS)
        }
        MachineCommand::Apply { name } => {
            let changes = run::collect(&store, &name).map_err(|error| error.to_string())?;
            if changes.is_empty() {
                println!("nothing to apply");
                return Ok(ExitCode::SUCCESS);
            }
            print_changes(&changes);
            let applied = run::apply(&store, &name, &changes).map_err(|error| error.to_string())?;
            println!(
                "applied: {} written ({}), {} deleted, {} created, {} removed",
                count(applied.written, "file", "files"),
                count(applied.bytes, "byte", "bytes"),
                count(applied.deleted, "file", "files"),
                count(applied.directories_created, "directory", "directories"),
                count(applied.directories_removed, "directory", "directories")
            );
            for kept in applied.directories_kept {
                println!("kept {kept}/: it still holds host files");
            }
            Ok(ExitCode::SUCCESS)
        }
        MachineCommand::Remove { name, yes } => {
            if !yes {
                return Err(format!(
                    "removing {name} deletes its disk and everything on it: pass --yes"
                ));
            }
            store.remove(&name).map_err(|error| error.to_string())?;
            println!("removed machine {name}");
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn create_config(arguments: CreateArgs, state_root: &Path) -> Result<MachineConfig, Failure> {
    let absolute = |path: &Path| {
        std::fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let mut config = MachineConfig::new(arguments.name, "/pending.iso");
    config.disk_mib = arguments.disk_mib;
    let host = vkore_vm::hardware::HostResources::probe(state_root)?;
    let (cpus, memory, _, resources) = vkored::hardware::defaults(&host, config.requirements);
    config.cpus = arguments.cpus.unwrap_or(cpus);
    config.memory_mib = arguments.memory_mib.unwrap_or(memory);
    config.resources = resources;
    config.resources.pin_cpus &= !arguments.no_cpu_pinning;
    config.accel = match arguments.accel {
        AccelArg::Auto => AccelChoice::Auto,
        AccelArg::Kvm => AccelChoice::Kvm,
        AccelArg::Hvf => AccelChoice::Hvf,
        AccelArg::Tcg => AccelChoice::Tcg,
    };
    config.resources.pin_memory &= !arguments.no_memory_pinning;
    config.resources.host_cpus = if arguments.host_cpus.is_empty() {
        host.preferred_cpus(config.cpus)
    } else {
        arguments.host_cpus
    };
    config.resources.reserve_storage = !arguments.sparse;
    config.resources.guest_numa = if arguments.numa == "single" {
        vkore_vm::hardware::GuestNuma::Single
    } else {
        vkore_vm::hardware::GuestNuma::Split
    };
    for value in &arguments.gpu_vf {
        let (address, index) = value
            .split_once('/')
            .and_then(|(address, n)| Some((address, n.parse::<u32>().ok()?.checked_sub(1)?)))
            .ok_or("--gpu-vf requires PF_ADDRESS/VF_NUMBER, numbered from 1")?;
        let gpu = host
            .gpus
            .devices
            .iter()
            .find(|g| g.pci.address == address)
            .ok_or("Selected GPU was not found (see vkore hardware)")?;
        config
            .resources
            .gpu_vfs
            .push(vkore_vm::passthrough::gpu::Assignment::new(gpu, index));
    }
    for address in &arguments.passthrough {
        let selection = host
            .npus
            .devices
            .iter()
            .filter_map(vkore_vm::passthrough::Selection::from_device)
            .find(|selection| selection.address == *address)
            .ok_or_else(|| {
                format!(
                    "--passthrough {address}: no NPU at that PCI address (see `vkore hardware`)"
                )
            })?;
        config.resources.passthrough.push(selection);
    }
    if let Some(page) = arguments.pages {
        config.resources.pages = match page.as_str() {
            "2m" => vkore_vm::hardware::PageSize::Huge2M,
            "1g" => vkore_vm::hardware::PageSize::Huge1G,
            _ => vkore_vm::hardware::PageSize::System,
        };
    }
    config.vendor = arguments.vendor.map(|vendor| match vendor {
        VendorArg::Intel => Vendor::Intel,
        VendorArg::Amd => Vendor::Amd,
    });
    config.resources.cpu_power_management = arguments.cpu_power_management
        || (!arguments.no_cpu_power_management
            && vkored::hardware::recommended_cpu_waits(
                &host,
                config.resources.pin_cpus,
                config.accel,
                config.vendor,
            ));
    vkored::hardware::validate_host(&config, &host)?;
    config.framebuffer = arguments
        .framebuffer
        .split_once('x')
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
        .ok_or_else(|| format!("--framebuffer {}: WIDTHxHEIGHT", arguments.framebuffer))?;
    if let Some(root) = arguments.workspace {
        let root = absolute(&root)?;
        let name = match arguments.workspace_name {
            Some(name) => name,
            None => root
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("name the project's directory with --workspace-name")?
                .to_owned(),
        };
        config.workspace = Some(WorkspaceConfig {
            root,
            name,
            face: arguments.face,
            exclude: if arguments.exclude.is_empty() {
                DEFAULT_EXCLUDES
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect()
            } else {
                arguments.exclude
            },
            writeback: match arguments.writeback {
                WritebackArg::Off => Writeback::Off,
                WritebackArg::Review => Writeback::Review,
                WritebackArg::Automatic => Writeback::Automatic,
            },
            max_files: arguments.max_files,
            max_bytes: arguments.max_bytes,
        });
    }
    if let (Some(code), Some(vars)) = (arguments.firmware_code, arguments.firmware_vars) {
        config.firmware = Some(FirmwareConfig {
            code: absolute(&code)?,
            vars_template: absolute(&vars)?,
            format: arguments.firmware_format,
        });
    }
    Ok(config)
}

fn describe(stage: Stage) -> &'static str {
    match stage {
        Stage::Firmware => "firmware",
        Stage::Kernel => "kernel up",
        Stage::Services => "services founded",
        Stage::Shell => "shell ready: Kore is usable",
        Stage::Ready => "ready: applications seeded, homes created",
        Stage::Degraded => "DEGRADED",
        Stage::Failed => "FAILED",
    }
}

fn print_event(event: &Event, json: bool, started: Instant) {
    if json {
        println!("{}", serde_json::to_string(event).unwrap_or_default());
        return;
    }
    let at = started.elapsed().as_secs_f64();
    let line = match event {
        Event::Preflight(preflight) => format!(
            "preflight: {} ({} MiB, sha256 {}), {:?} paging; QEMU {}.{}.{}, {:?} on {:?}, firmware {}",
            preflight.medium.path.display(),
            preflight.medium.bytes >> 20,
            preflight.medium.sha256,
            preflight.medium.paging,
            preflight.qemu_caps.version.0,
            preflight.qemu_caps.version.1,
            preflight.qemu_caps.version.2,
            preflight.vendor,
            preflight.accel,
            preflight.firmware.code.display()
        ),
        Event::Staged {
            files,
            directories,
            skipped,
            generation,
        } => format!(
            "staged the project: {}, {} ({skipped} left out), volume generation {generation}",
            count(*files, "file", "files"),
            count(*directories, "directory", "directories")
        ),
        Event::Started { pid } => format!("QEMU started (pid {pid})"),
        Event::CpuPowerManagement => {
            "CPU power management: native MONITOR/MWAIT enabled on pinned CPUs".into()
        }
        Event::MemoryBacking { summary } => summary.clone(),
        Event::Console { socket } => format!(
            "console: the shell is on COM2; attach with socat -,raw,echo=0 UNIX-CONNECT:{}",
            socket.display()
        ),
        Event::Stage { stage, .. } => format!("boot: {}", describe(*stage)),
        Event::Qemu { name, data } => format!("qemu: {name} {data}"),
        Event::Stopping => "stopping: pressed the power button".into(),
        Event::Stopped { forced } => {
            format!("stopped{}", if *forced { " (QEMU ended)" } else { "" })
        }
        Event::Exited { code } => format!("QEMU exited ({code:?})"),
        Event::Changes {
            created,
            modified,
            deleted,
            bytes,
        } => format!(
            "the machine changed the project: {created} created, {modified} modified, {deleted} deleted ({bytes} bytes)"
        ),
        Event::Applied(applied) => format!(
            "applied: {} written, {} deleted",
            applied.written, applied.deleted
        ),
        Event::Warning { message } => format!("warning: {message}"),
    };
    println!("[{at:7.1}s] {line}");
}

fn run_machine(store: &MachineStore, arguments: RunArgs) -> Result<ExitCode, Failure> {
    let qemu = qemu(arguments.qemu.clone())?;
    let display = match arguments.display.as_str() {
        "window" => Display::Window,
        "headless" => Display::Headless,
        other => match other.strip_prefix("vnc=") {
            Some(socket) => Display::Vnc(socket.into()),
            None => return Err(format!("--display {other}: window, headless or vnc=SOCKET")),
        },
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = interrupted.clone();
        ctrlc::set_handler(move || interrupted.store(true, Ordering::SeqCst))
            .map_err(|error| format!("cannot handle Ctrl-C: {error}"))?;
    }
    let started = Instant::now();
    let json = arguments.json;
    let mut sink = |event: &Event| print_event(event, json, started);
    let mut session = Session::start(store, &arguments.name, &qemu, display, &mut sink)
        .map_err(|error| error.to_string())?;
    let goal = match arguments.until {
        Until::Kernel => Stage::Kernel,
        Until::Shell => Stage::Shell,
        Until::Ready => Stage::Ready,
    };
    let deadline = started + Duration::from_secs(arguments.timeout);
    let mut reached: Option<Instant> = None;
    let mut failed = false;
    loop {
        let running = session
            .poll(Duration::from_millis(250), &mut sink)
            .map_err(|error| error.to_string())?;
        if !running {
            break;
        }
        let stage = session.stage();
        if matches!(stage, Stage::Failed | Stage::Degraded) {
            failed = true;
            for line in &session.machine().readiness().failures {
                eprintln!("vkore: {line}");
            }
            break;
        }
        if reached.is_none() && stage >= goal && stage <= Stage::Ready {
            reached = Some(Instant::now());
            if let Some(path) = &arguments.screendump {
                match session.machine().screendump(path) {
                    Ok(()) => println!("screen written to {}", path.display()),
                    Err(error) => eprintln!("vkore: screendump: {error}"),
                }
            }
            if arguments.screenshot {
                match session.machine().send_keys(&["print"]) {
                    Ok(()) => println!("pressed PrintScreen in the guest"),
                    Err(error) => eprintln!("vkore: PrintScreen: {error}"),
                }
            }
        }
        if reached.is_none() && Instant::now() > deadline {
            eprintln!(
                "vkore: {} did not reach {} within {} s",
                arguments.name,
                describe(goal),
                arguments.timeout
            );
            failed = true;
            break;
        }
        let due = reached
            .zip(arguments.stop_after)
            .is_some_and(|(at, after)| at.elapsed() >= Duration::from_secs(after));
        if interrupted.load(Ordering::SeqCst) || due {
            break;
        }
    }
    if session.machine().running() {
        session
            .stop(Duration::from_secs(arguments.grace), &mut sink)
            .map_err(|error| error.to_string())?;
    }
    let changes = session
        .finish(store, &mut sink)
        .map_err(|error| error.to_string())?;
    if let Some(changes) = changes.filter(|changes| !changes.is_empty()) {
        print_changes(&changes);
        if store
            .get(&arguments.name)
            .ok()
            .and_then(|config| config.workspace)
            .is_some_and(|workspace| workspace.writeback == Writeback::Review)
        {
            println!("review them, then: vkore machine apply {}", arguments.name);
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn print_changes(changes: &Changes) {
    if changes.is_empty() {
        println!("no changes");
        return;
    }
    for change in &changes.created {
        println!("  created  {} ({} bytes)", change.path, change.len);
    }
    for change in &changes.modified {
        println!("  modified {} ({} bytes)", change.path, change.len);
    }
    for path in &changes.deleted {
        println!("  deleted  {path}");
    }
    for path in &changes.created_directories {
        println!("  created  {path}/");
    }
    for path in &changes.deleted_directories {
        println!("  deleted  {path}/");
    }
}

fn open_disk(state: Option<PathBuf>, name: &str, block_size: Option<u32>) -> Result<Disk, Failure> {
    let disk = match name.strip_prefix('@') {
        Some(machine) => {
            let store = store(state)?;
            store.get(machine).map_err(|error| error.to_string())?;
            Disk::new(store.state().machine(machine).disk)
        }
        None => Disk::new(name),
    }
    .map_err(|error| error.to_string())?;
    match block_size {
        Some(bytes) => disk
            .with_block_size(bytes)
            .map_err(|error| error.to_string()),
        None => Ok(disk),
    }
}

fn volume(state: Option<PathBuf>, arguments: VolumeArgs) -> Result<ExitCode, Failure> {
    let disk = open_disk(state, &arguments.disk, arguments.block_size)?;
    let face = arguments.face;
    let path = |text: Option<&str>| {
        VolPath::parse(face, text.unwrap_or("")).map_err(|error| error.to_string())
    };
    let browse = || Browser::open(&disk).map_err(|error| error.to_string());
    match arguments.command {
        VolumeCommand::Info => {
            let browser = browse()?;
            let status = browser.status();
            println!(
                "{} ({:?}), DagFS generation {}{}; this vKore writes format {}",
                disk.path().display(),
                disk.kind(),
                status.generation,
                if status.used_fallback_root {
                    ", from its fallback root: Kore keeps it read-only"
                } else {
                    ""
                },
                vkore_volume::bounds::FORMAT_VERSION
            );
        }
        VolumeCommand::Ls { path: text, json } => {
            let listing = browse()?
                .list(&path(text.as_deref())?)
                .map_err(|error| error.to_string())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&listing).unwrap_or_default()
                );
                return Ok(ExitCode::SUCCESS);
            }
            match listing {
                Listing::Directory { entries } => {
                    for entry in entries {
                        match entry.kind {
                            EntryKind::Directory => println!("dir  {:>12}  {}/", "", entry.name),
                            EntryKind::File => println!("file {:>12}  {}", entry.len, entry.name),
                        }
                    }
                }
                Listing::File { len } => println!("file {len:>12}  {}", text.unwrap_or_default()),
                Listing::EmptyHome => {
                    println!("(terminal {face}'s home is not created until its first boot)")
                }
            }
        }
        VolumeCommand::Tree { path: text } => {
            for entry in browse()?
                .walk(&path(text.as_deref())?)
                .map_err(|error| error.to_string())?
            {
                let indent = "  ".repeat(entry.depth.saturating_sub(1));
                let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
                match entry.kind {
                    EntryKind::Directory => println!("{indent}{name}/"),
                    EntryKind::File => println!("{indent}{name}  ({} bytes)", entry.len),
                }
            }
        }
        VolumeCommand::Cat { path: text } => {
            let browser = browse()?;
            browser
                .copy_to(&path(Some(&text))?, io::stdout())
                .map_err(|error| error.to_string())?;
            io::stdout().flush().map_err(|error| error.to_string())?;
        }
        VolumeCommand::Get {
            path: text,
            destination,
            replace,
        } => {
            let exported = browse()?
                .export(&path(Some(&text))?, &destination, ExportOptions { replace })
                .map_err(|error| error.to_string())?;
            println!(
                "{}: {}, {}, {}",
                exported.destination.display(),
                count(exported.files, "file", "files"),
                count(exported.directories, "directory", "directories"),
                count(exported.bytes, "byte", "bytes")
            );
        }
        VolumeCommand::Put {
            source,
            path: text,
            replace,
        } => {
            let installed = disk
                .install(&source, &path(Some(&text))?, replace)
                .map_err(|error| error.to_string())?;
            println!(
                "{text}: {}, {}, {} in terminal {face}'s home (generation {}); every file read back identical",
                count(installed.files, "file", "files"),
                count(installed.directories, "directory", "directories"),
                count(installed.bytes, "byte", "bytes"),
                installed.generation
            );
        }
        VolumeCommand::Rm { path: text } => {
            let removed = disk
                .remove(&path(Some(&text))?)
                .map_err(|error| error.to_string())?;
            println!("{text}: removed (generation {})", removed.generation);
        }
        VolumeCommand::Format => {
            let generation = disk.format().map_err(|error| error.to_string())?;
            println!("formatted as Kore's first boot would (generation {generation})");
        }
        VolumeCommand::Shots { get, json } => shots(&disk, face, get, json)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// `n` and its noun in agreement: "1 file", "2 files".
fn count<N: std::fmt::Display + PartialEq + From<u8>>(n: N, one: &str, many: &str) -> String {
    let noun = if n == N::from(1) { one } else { many };
    format!("{n} {noun}")
}

/// Kore's screen captures in terminal `face`'s home, newest first by name
/// (Kore names them by their UTC capture time).
fn shots(disk: &Disk, face: u8, get: Option<PathBuf>, json: bool) -> Result<(), Failure> {
    let browser = Browser::open(disk).map_err(|error| error.to_string())?;
    let folder =
        VolPath::parse(face, vkore_volume::SCREENSHOTS).map_err(|error| error.to_string())?;
    let entries = match browser.list(&folder) {
        Ok(Listing::Directory { entries }) => entries,
        // A home its machine has not created yet, or one without the folder,
        // holds no captures.
        Ok(Listing::EmptyHome) => Vec::new(),
        Err(error) if error.is_not_found() => Vec::new(),
        Ok(Listing::File { .. }) => {
            return Err(format!(
                "{folder}: a file where Kore keeps its captures' folder"
            ));
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut shots = entries
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::File && entry.name.ends_with(".png"))
        .collect::<Vec<_>>();
    shots.sort_by(|a, b| b.name.cmp(&a.name));
    if shots.is_empty() && !json {
        // Nothing to copy, so a destination is not created either.
        println!(
            "no screenshots in terminal {face}'s home ({})",
            vkore_volume::SCREENSHOTS
        );
        return Ok(());
    }
    let mut rows = Vec::new();
    for shot in &shots {
        let path = folder.join(&shot.name).map_err(|error| error.to_string())?;
        // A screenshot's size and record sit ahead of its pixels: its head is
        // all a listing reads.
        let bytes = browser
            .head(&path, preview::PREVIEW_HEAD_BYTES)
            .map_err(|error| error.to_string())?;
        let (width, height, capture) = match preview::preview(&bytes, shot.len) {
            Preview::Image {
                width,
                height,
                capture,
                ..
            } => (width, height, capture),
            _ => (0, 0, None),
        };
        rows.push(serde_json::json!({
            "name": shot.name,
            "bytes": shot.len,
            "width": width,
            "height": height,
            "capture": capture,
        }));
        if !json {
            let capture = capture.map(|capture| *capture).unwrap_or_default();
            println!(
                "{}  {}x{}  {} bytes  {}  build {}  {}",
                shot.name,
                width,
                height,
                shot.len,
                capture.created.as_deref().unwrap_or("(no wall clock)"),
                capture.kernel_build.as_deref().unwrap_or("?"),
                capture.mode.as_deref().unwrap_or("")
            );
        }
        if let Some(destination) = &get {
            browser
                .export(&path, destination, ExportOptions::default())
                .map_err(|error| error.to_string())?;
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
    } else if let Some(destination) = &get {
        println!(
            "copied {} to {}",
            count(shots.len(), "screenshot", "screenshots"),
            destination.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_wait_overrides_are_optional_and_reject_conflicts() {
        let base = ["vkore", "machine", "create", "test", "--iso", "/image.iso"];
        for enabled in [false, true] {
            let Command::Machine(MachineCommand::Create(create)) =
                Cli::try_parse_from(base.into_iter().chain(enabled.then_some("--cpu-pm")))
                    .unwrap()
                    .command
            else {
                panic!("expected machine create")
            };
            assert_eq!(create.cpu_power_management, enabled);
        }
        assert!(
            Cli::try_parse_from(base.into_iter().chain(["--cpu-pm", "--no-cpu-pinning"])).is_err()
        );
        assert!(Cli::try_parse_from(base.into_iter().chain(["--cpu-pm", "--no-cpu-pm"])).is_err());
        let Command::Machine(MachineCommand::Create(create)) =
            Cli::try_parse_from(base.into_iter().chain(["--no-cpu-pm"]))
                .unwrap()
                .command
        else {
            panic!("expected machine create")
        };
        assert!(create.no_cpu_power_management);
        assert!(!create.cpu_power_management);
    }

    #[test]
    fn official_and_custom_sources_are_mutually_exclusive() {
        let base = ["vkore", "machine", "create", "test"];
        assert!(Cli::try_parse_from(base).is_err());
        assert!(Cli::try_parse_from(base.into_iter().chain(["--image", "release-1"])).is_ok());
        for extra in [
            vec!["--image", "release-1", "--iso", "/image.iso"],
            vec!["--image", "release-1", "--sha256", "digest"],
        ] {
            assert!(Cli::try_parse_from(base.into_iter().chain(extra)).is_err());
        }
    }

    #[test]
    fn disk_default_matches_new_configs_and_allows_explicit_capacity() {
        for (extra, expected) in [
            (None, vkored::hardware::DEFAULT_DISK_MIB),
            (Some("49152"), 49152),
        ] {
            let mut args = vec![
                "vkore",
                "machine",
                "create",
                "inference",
                "--iso",
                "/image.iso",
            ];
            if let Some(value) = extra {
                args.extend(["--disk-mib", value]);
            }
            let Command::Machine(MachineCommand::Create(create)) =
                Cli::try_parse_from(args).unwrap().command
            else {
                panic!("expected machine create");
            };
            assert_eq!(create.disk_mib, expected);
        }
        assert_eq!(
            MachineConfig::new("inference", "/image.iso").disk_mib,
            32 * 1024
        );
    }
}
