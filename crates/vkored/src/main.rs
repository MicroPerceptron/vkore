#[cfg(unix)]
use hostlinkd::Config;
use hostlinkd::{
    BackendLimits, HostLinkDaemon, InferenceBackend, LlamaServerBackend, LlamaServerConfig,
    TcpConfig, WorkspaceBackend,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use vkore_vm::QemuConfig;

fn main() {
    if let Err(error) = run() {
        eprintln!("vkored: {error}");
        std::process::exit(2)
    }
}

enum ListenerConfig {
    #[cfg(unix)]
    Unix(Config),
    Tcp(TcpConfig),
}

fn run() -> Result<(), String> {
    let mut control = None;
    let mut bulk = None;
    let mut control_tcp = None;
    let mut bulk_tcp = None;
    let mut transcript = None;
    let mut ui_listen = None;
    let mut print_qemu = false;
    let mut qemu = None;
    let mut kore_iso = None;
    let mut qemu_arguments = Vec::new();
    let mut tick_ms = 1u64;
    let mut max_ticks = None;
    let mut workspace_root = None;
    let mut workspace_state = None;
    let mut workspace_write = false;
    let mut llama_server = None;
    let mut llama_model = None;
    let mut llama_port = 8_189u16;
    let mut llama_context = 32_768u32;
    let mut llama_broker_slots = 1usize;
    let mut llama_agent_slots = 2usize;
    let mut llama_max_output_tokens = 256u32;
    let mut llama_threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(u16::MAX as usize) as u16;
    let mut llama_trace = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--control-socket") => {
                control = Some(PathBuf::from(args.next().ok_or("missing control path")?));
            }
            Some("--bulk-socket") => {
                bulk = Some(PathBuf::from(args.next().ok_or("missing bulk path")?));
            }
            Some("--control-tcp") => {
                control_tcp = Some(
                    args.next()
                        .and_then(|value| value.to_str()?.parse::<SocketAddr>().ok())
                        .ok_or("invalid control TCP address")?,
                );
            }
            Some("--bulk-tcp") => {
                bulk_tcp = Some(
                    args.next()
                        .and_then(|value| value.to_str()?.parse::<SocketAddr>().ok())
                        .ok_or("invalid bulk TCP address")?,
                );
            }
            Some("--transcript") => {
                transcript = Some(PathBuf::from(args.next().ok_or("missing transcript path")?));
            }
            Some("--ui-listen") => {
                ui_listen = Some(
                    args.next()
                        .and_then(|value| value.to_str()?.parse::<SocketAddr>().ok())
                        .ok_or("invalid UI listen address")?,
                );
            }
            Some("--workspace-root") => {
                workspace_root = Some(PathBuf::from(args.next().ok_or("missing workspace root")?));
            }
            Some("--workspace-state") => {
                workspace_state = Some(PathBuf::from(
                    args.next().ok_or("missing workspace state directory")?,
                ));
            }
            Some("--workspace-write") => workspace_write = true,
            Some("--print-qemu-args") => print_qemu = true,
            Some("--qemu") => {
                qemu = Some(PathBuf::from(args.next().ok_or("missing QEMU executable")?));
            }
            Some("--kore-iso") => {
                kore_iso = Some(PathBuf::from(args.next().ok_or("missing Kore ISO path")?));
            }
            Some("--qemu-arg") => {
                qemu_arguments.push(args.next().ok_or("missing QEMU argument")?);
            }
            Some("--tick-ms") => tick_ms = parse_arg(args.next(), "tick-ms")?,
            Some("--max-ticks") => max_ticks = Some(parse_arg(args.next(), "max-ticks")?),
            Some("--llama-server") => {
                llama_server = Some(PathBuf::from(
                    args.next().ok_or("missing llama-server path")?,
                ));
            }
            Some("--llama-model") => {
                llama_model = Some(PathBuf::from(
                    args.next().ok_or("missing llama model path")?,
                ));
            }
            Some("--llama-port") => llama_port = parse_arg(args.next(), "llama-port")?,
            Some("--llama-context") => {
                llama_context = parse_arg(args.next(), "llama-context")?;
            }
            Some("--llama-broker-slots") => {
                llama_broker_slots = parse_arg(args.next(), "llama-broker-slots")?;
            }
            Some("--llama-agent-slots") => {
                llama_agent_slots = parse_arg(args.next(), "llama-agent-slots")?;
            }
            Some("--llama-max-output-tokens") => {
                llama_max_output_tokens = parse_arg(args.next(), "llama-max-output-tokens")?;
            }
            Some("--llama-threads") => {
                llama_threads = parse_arg(args.next(), "llama-threads")?;
            }
            Some("--llama-trace") => llama_trace = true,
            _ => return Err(usage()),
        }
    }

    let listener = match (control, bulk, control_tcp, bulk_tcp) {
        #[cfg(unix)]
        (Some(control), Some(bulk), None, None) => ListenerConfig::Unix(Config {
            control_socket: control,
            bulk_socket: bulk,
        }),
        (None, None, Some(control), Some(bulk)) => ListenerConfig::Tcp(TcpConfig { control, bulk }),
        _ => return Err("select exactly one complete Unix-socket or TCP listener pair".into()),
    };
    if print_qemu {
        for arg in hostlink_qemu_arguments(&listener)? {
            println!("{arg}");
        }
        return Ok(());
    }
    let vm = match (qemu, kore_iso) {
        (Some(executable), Some(kore_iso)) => Some(QemuConfig {
            executable,
            kore_iso,
            arguments: qemu_arguments,
            hostlink_arguments: hostlink_qemu_arguments(&listener)?,
        }),
        (None, None) if qemu_arguments.is_empty() => None,
        _ => return Err("--qemu and --kore-iso must be supplied together".into()),
    };

    if let Some(root) = workspace_root {
        if workspace_write && workspace_state.is_none() {
            return Err(
                "--workspace-write requires a separately selected --workspace-state directory"
                    .into(),
            );
        }
        if let Some(executable) = llama_server {
            let model = llama_model.ok_or("--llama-model is required with --llama-server")?;
            let inference = start_llama_backend(
                executable,
                model,
                llama_port,
                llama_context,
                llama_threads,
                llama_broker_slots,
                llama_agent_slots,
                llama_max_output_tokens,
                llama_trace,
            )?;
            let backend = WorkspaceBackend::open_with_inference(
                &root,
                workspace_state.as_deref(),
                workspace_write,
                inference,
            )
            .map_err(|error| format!("{error:?}"))?;
            serve(
                listener, backend, transcript, ui_listen, tick_ms, max_ticks, vm,
            )
        } else {
            Err("live inference requires --llama-server and --llama-model; no test backend is available".into())
        }
    } else if let Some(executable) = llama_server {
        if workspace_write || workspace_state.is_some() {
            return Err("--workspace-write/--workspace-state require --workspace-root".into());
        }
        let model = llama_model.ok_or("--llama-model is required with --llama-server")?;
        let backend = start_llama_backend(
            executable,
            model,
            llama_port,
            llama_context,
            llama_threads,
            llama_broker_slots,
            llama_agent_slots,
            llama_max_output_tokens,
            llama_trace,
        )?;
        serve(
            listener, backend, transcript, ui_listen, tick_ms, max_ticks, vm,
        )
    } else {
        if workspace_write || workspace_state.is_some() {
            return Err("--workspace-write/--workspace-state require --workspace-root".into());
        }
        if llama_model.is_some() {
            return Err("--llama-model requires --llama-server".into());
        }
        Err("live inference requires --llama-server and --llama-model; no test backend is available".into())
    }
}

fn hostlink_qemu_arguments(listener: &ListenerConfig) -> Result<Vec<String>, String> {
    match listener {
        #[cfg(unix)]
        ListenerConfig::Unix(config) => {
            vkore_vm::arguments(&config.control_socket, &config.bulk_socket)
        }
        ListenerConfig::Tcp(config) => vkore_vm::tcp_arguments(config.control, config.bulk),
    }
    .map_err(|error| format!("invalid QEMU HostLink topology: {error:?}"))
}

#[allow(clippy::too_many_arguments)]
fn start_llama_backend(
    executable: PathBuf,
    model: PathBuf,
    port: u16,
    context: u32,
    threads: u16,
    broker_slots: usize,
    agent_slots: usize,
    max_output_tokens: u32,
    trace: bool,
) -> Result<LlamaServerBackend, String> {
    let sequences = broker_slots
        .checked_add(agent_slots)
        .ok_or("llama slot count overflow")?;
    let limits = BackendLimits {
        sequences,
        brokers: broker_slots,
        agents: agent_slots,
        max_output_tokens,
        ..BackendLimits::default()
    };
    LlamaServerBackend::start(LlamaServerConfig {
        executable,
        executable_args: Vec::new(),
        model,
        port,
        total_context_tokens: context,
        threads,
        startup_timeout: Duration::from_secs(600),
        io_timeout: Duration::from_millis(25),
        limits,
        trace,
    })
    .map_err(|error| format!("llama-server startup failed: {error:?}"))
}

fn serve<B: InferenceBackend>(
    config: ListenerConfig,
    backend: B,
    transcript: Option<PathBuf>,
    ui_listen: Option<SocketAddr>,
    tick_ms: u64,
    max_ticks: Option<u64>,
    vm: Option<QemuConfig>,
) -> Result<(), String> {
    let mut daemon = match config {
        #[cfg(unix)]
        ListenerConfig::Unix(config) => HostLinkDaemon::bind(config, backend, ()),
        ListenerConfig::Tcp(config) => HostLinkDaemon::bind_tcp(config, backend, ()),
    }
    .map_err(|error| format!("{error:?}"))?;
    let mut ui = ui_listen
        .map(hostlinkd::LiveUi::bind)
        .transpose()
        .map_err(|error| format!("UI bind failed: {error:?}"))?;
    if let Some(ui) = ui.as_ref() {
        eprintln!(
            "vkored live UI: http://{}",
            ui.local_addr().map_err(|error| error.to_string())?
        );
    }
    let mut vm = vm
        .map(QemuConfig::spawn)
        .transpose()
        .map_err(|error| format!("QEMU startup failed: {error}"))?;
    if let Some(vm) = vm.as_ref() {
        eprintln!("vkored Kore VM: QEMU pid {}", vm.id());
    }
    let mut ticks = 0u64;
    let mut vm_failure = None;
    while max_ticks.is_none_or(|limit| ticks < limit) {
        daemon.tick().map_err(|error| format!("{error:?}"))?;
        while let Some(observation) = daemon.take_inference_observation() {
            if let Some(ui) = ui.as_mut() {
                ui.record(observation);
            }
        }
        while let Some(observation) = daemon.take_authority_observation() {
            if let Some(ui) = ui.as_mut() {
                ui.record_authority_observation(observation);
            }
        }
        if let Some(ui) = ui.as_mut() {
            ui.poll().map_err(|error| format!("UI error: {error:?}"))?;
        }
        if let Some(status) = vm
            .as_mut()
            .map(vkore_vm::QemuProcess::try_wait)
            .transpose()
            .map_err(|error| format!("QEMU status failed: {error}"))?
            .flatten()
        {
            if !status.success() {
                vm_failure = Some(format!("QEMU exited with {status}"));
            }
            break;
        }
        ticks = ticks.saturating_add(1);
        thread::sleep(Duration::from_millis(tick_ms));
    }
    if let Some(path) = transcript {
        daemon
            .write_transcript(&path)
            .map_err(|error| error.to_string())?;
    }
    vm_failure.map_or(Ok(()), Err)
}

fn parse_arg<T: std::str::FromStr>(
    value: Option<std::ffi::OsString>,
    name: &str,
) -> Result<T, String> {
    value
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("invalid {name}"))
}

fn usage() -> String {
    "usage: vkored (--control-socket PATH --bulk-socket PATH | --control-tcp ADDR --bulk-tcp ADDR) [--qemu PATH --kore-iso PATH [--qemu-arg ARG]...] [--workspace-root PATH [--workspace-state PATH --workspace-write]] [--llama-server PATH --llama-model PATH [--llama-port N] [--llama-context N] [--llama-threads N] [--llama-broker-slots N] [--llama-agent-slots N] [--llama-max-output-tokens N] [--llama-trace]] [--transcript PATH] [--ui-listen 127.0.0.1:PORT] [--tick-ms N] [--max-ticks N] [--print-qemu-args]".into()
}
