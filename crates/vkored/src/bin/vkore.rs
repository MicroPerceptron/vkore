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
    /// Inspect a Kore ISO: its SHA-256 and the paging mode it boots
    Iso { iso: PathBuf },
    /// Kore machines
    #[command(subcommand)]
    Machine(MachineCommand),
    /// A Kore volume: a raw disk image, a disk's block device (Linux), or @MACHINE
    Volume(VolumeArgs),
}

#[derive(Subcommand)]
enum MachineCommand {
    /// Create a machine that boots ISO
    Create(CreateArgs),
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
    /// The Kore x86_64 ISO the machine boots
    #[arg(long)]
    iso: PathBuf,
    #[arg(long, default_value_t = 4096)]
    disk_mib: u64,
    #[arg(long, default_value_t = 4)]
    cpus: u16,
    #[arg(long, default_value_t = 4096)]
    memory_mib: u32,
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
    }
}

fn probe(qemu: PathBuf) -> Result<ExitCode, Failure> {
    let host = HostCaps::probe();
    let vendor = host.native_vendor();
    let accel = host
        .choose(AccelChoice::Auto, vendor)
        .map_err(|error| error.to_string())?;
    let caps = QemuCaps::probe(&qemu, vendor, accel).map_err(|error| error.to_string())?;
    let firmware = firmware::discover();
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
        MachineCommand::Create(arguments) => {
            let config = create_config(arguments)?;
            store.create(&config).map_err(|error| error.to_string())?;
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
                "applied: {} written ({} bytes), {} deleted, {} directories created, {} removed",
                applied.written,
                applied.bytes,
                applied.deleted,
                applied.directories_created,
                applied.directories_removed
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

fn create_config(arguments: CreateArgs) -> Result<MachineConfig, Failure> {
    let absolute = |path: &Path| {
        std::fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let mut config = MachineConfig::new(arguments.name, absolute(&arguments.iso)?);
    config.disk_mib = arguments.disk_mib;
    config.cpus = arguments.cpus;
    config.memory_mib = arguments.memory_mib;
    config.vendor = arguments.vendor.map(|vendor| match vendor {
        VendorArg::Intel => Vendor::Intel,
        VendorArg::Amd => Vendor::Amd,
    });
    config.accel = match arguments.accel {
        AccelArg::Auto => AccelChoice::Auto,
        AccelArg::Kvm => AccelChoice::Kvm,
        AccelArg::Hvf => AccelChoice::Hvf,
        AccelArg::Tcg => AccelChoice::Tcg,
    };
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
        Stage::Shell => "shell ready: the desktop is usable",
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
            "staged the project: {files} files, {directories} directories ({skipped} left out), volume generation {generation}"
        ),
        Event::Started { pid } => format!("QEMU started (pid {pid})"),
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
                "{}: {} files, {} directories, {} bytes",
                exported.destination.display(),
                exported.files,
                exported.directories,
                exported.bytes
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
                "{text}: {} files, {} directories, {} bytes in terminal {face}'s home (generation {}); every file read back identical",
                installed.files, installed.directories, installed.bytes, installed.generation
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
            "copied {} screenshot(s) to {}",
            shots.len(),
            destination.display()
        );
    }
    Ok(())
}
