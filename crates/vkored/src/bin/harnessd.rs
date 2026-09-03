use hostlinkd::{
    BackendHarness, BackendLimits, DeterministicBackend, DeterministicGuest, HarnessHttpServer,
    HarnessService, KoreVmRuntime, KoreVmRuntimeConfig, LlamaServerBackend, LlamaServerConfig,
    TaskRuntime,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("vkore-harnessd: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut listen: SocketAddr = "127.0.0.1:8787".parse().unwrap();
    let mut state = PathBuf::from(".vkore-state");
    let mut llama_server = None;
    let mut llama_model = None;
    let mut llama_port = 8_189u16;
    let mut llama_args = Vec::new();
    let mut qemu = None;
    let mut qemu_args = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().and_then(|value| value.parse().ok()).ok_or("invalid --listen")?,
            "--state" => state = PathBuf::from(args.next().ok_or("missing --state")?),
            "--llama-server" => llama_server = Some(PathBuf::from(args.next().ok_or("missing --llama-server")?)),
            "--llama-model" => llama_model = Some(PathBuf::from(args.next().ok_or("missing --llama-model")?)),
            "--llama-port" => llama_port = args.next().and_then(|value| value.parse().ok()).ok_or("invalid --llama-port")?,
            "--llama-arg" => llama_args.push(args.next().ok_or("missing --llama-arg")?.into()),
            "--qemu" => qemu = Some(PathBuf::from(args.next().ok_or("missing --qemu")?)),
            "--qemu-arg" => qemu_args.push(args.next().ok_or("missing --qemu-arg")?.into()),
            _ => return Err("usage: vkore-harnessd [--listen 127.0.0.1:8787] [--state PATH] [--qemu PATH [--qemu-arg ARG]...] [--llama-server PATH --llama-model PATH [--llama-port N] [--llama-arg ARG]...]".into()),
        }
    }
    if let Some(qemu_executable) = qemu {
        if llama_server.is_some() || llama_model.is_some() || !llama_args.is_empty() {
            return Err("real Kore mode reads llama.cpp settings from the selected project".into());
        }
        let runtime = KoreVmRuntime::new(KoreVmRuntimeConfig {
            qemu_executable,
            qemu_arguments: qemu_args,
            project_root: state.join("projects"),
        })?;
        return run_server(listen, state, runtime, "real Kore VM over HostLink");
    }
    match (llama_server, llama_model) {
        (None, None) => run_server(
            listen,
            state,
            BackendHarness::new(DeterministicBackend::new(2), DeterministicGuest::new()),
            "deterministic Kore security oracle",
        ),
        (Some(executable), Some(model)) => {
            let limits = BackendLimits {
                sequences: 3,
                brokers: 1,
                agents: 2,
                max_output_tokens: 512,
                ..BackendLimits::default()
            };
            let backend = LlamaServerBackend::start(LlamaServerConfig {
                executable,
                executable_args: llama_args,
                model,
                port: llama_port,
                total_context_tokens: 32_768,
                threads: std::thread::available_parallelism()
                    .map_or(1, usize::from)
                    .min(u16::MAX as usize) as u16,
                startup_timeout: Duration::from_secs(600),
                io_timeout: Duration::from_millis(25),
                limits,
                trace: false,
            })
            .map_err(|error| format!("llama-server startup failed: {error:?}"))?;
            run_server(
                listen,
                state,
                BackendHarness::new(backend, DeterministicGuest::new()),
                "caller-supplied llama.cpp",
            )
        }
        _ => Err("--llama-server and --llama-model must be supplied together".into()),
    }
}

fn run_server(
    listen: SocketAddr,
    state: PathBuf,
    runtime: impl TaskRuntime,
    backend_name: &str,
) -> Result<(), String> {
    let service = HarnessService::start(runtime, state.join("transcripts"));
    let server =
        HarnessHttpServer::bind_with_state(listen, service.handle(), state.join("projects"))
            .map_err(|error| error.to_string())?;
    eprintln!("vKore interactive harness: http://{}/", server.local_addr());
    eprintln!("backend: {backend_name}");
    loop {
        server
            .poll(Duration::from_millis(250))
            .map_err(|error| error.to_string())?;
    }
}
