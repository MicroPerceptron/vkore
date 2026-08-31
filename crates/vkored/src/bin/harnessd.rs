use hostlinkd::{
    BackendHarness, BackendLimits, DeterministicBackend, DeterministicGuest, HarnessHttpServer,
    HarnessService, LlamaServerBackend, LlamaServerConfig, TaskRuntime,
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
    let mut token = None;
    let mut state = PathBuf::from(".vkore-state");
    let mut llama_server = None;
    let mut llama_model = None;
    let mut llama_port = 8_189u16;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().and_then(|value| value.parse().ok()).ok_or("invalid --listen")?,
            "--token" => token = Some(args.next().ok_or("missing --token")?),
            "--state" => state = PathBuf::from(args.next().ok_or("missing --state")?),
            "--llama-server" => llama_server = Some(PathBuf::from(args.next().ok_or("missing --llama-server")?)),
            "--llama-model" => llama_model = Some(PathBuf::from(args.next().ok_or("missing --llama-model")?)),
            "--llama-port" => llama_port = args.next().and_then(|value| value.parse().ok()).ok_or("invalid --llama-port")?,
            _ => return Err("usage: vkore-harnessd [--listen 127.0.0.1:8787] --token TOKEN [--state PATH] [--llama-server PATH --llama-model PATH [--llama-port N]]".into()),
        }
    }
    let token = token.ok_or("--token is required")?;
    match (llama_server, llama_model) {
        (None, None) => run_server(
            listen,
            token,
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
                executable_args: Vec::new(),
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
                token,
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
    token: String,
    state: PathBuf,
    runtime: impl TaskRuntime,
    backend_name: &str,
) -> Result<(), String> {
    let service = HarnessService::start(runtime, &token, state.join("transcripts"));
    let server =
        HarnessHttpServer::bind(listen, service.handle()).map_err(|error| error.to_string())?;
    eprintln!("vKore interactive harness: http://{}", server.local_addr());
    eprintln!("backend: {backend_name}");
    loop {
        server
            .poll(Duration::from_millis(250))
            .map_err(|error| error.to_string())?;
    }
}
