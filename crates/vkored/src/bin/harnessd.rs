use hostlinkd::{
    HarnessHttpServer, HarnessService, KoreVmRuntime, KoreVmRuntimeConfig, TaskRuntime,
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
    let mut qemu = None;
    let mut qemu_args = Vec::new();
    let mut headless_park = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().and_then(|value| value.parse().ok()).ok_or("invalid --listen")?,
            "--state" => state = PathBuf::from(args.next().ok_or("missing --state")?),
            "--llama-server" | "--llama-model" | "--llama-port" | "--llama-arg" => return Err("configure the application llama.cpp profile in settings.json; the production harness requires real Kore via --qemu".into()),
            "--qemu" => qemu = Some(PathBuf::from(args.next().ok_or("missing --qemu")?)),
            "--require-real" => {}, // Compatibility: real execution is now unconditional.
            "--headless-park" => headless_park = true,
            "--qemu-arg" => qemu_args.push(args.next().ok_or("missing --qemu-arg")?.into()),
            _ => return Err("usage: vkore-harnessd --qemu PATH [--listen 127.0.0.1:8787] [--state PATH] [--headless-park] [--qemu-arg ARG]...".into()),
        }
    }
    let qemu_executable = qemu.ok_or("--qemu is required; simulated guests are forbidden")?;
    let runtime = KoreVmRuntime::new(KoreVmRuntimeConfig {
        qemu_executable,
        qemu_arguments: qemu_args,
        project_root: state.join("projects"),
        require_real: true,
        headless_park,
    })?;
    run_server(
        listen,
        state,
        runtime,
        "real Kore VM over HostLink + live llama.cpp",
    )
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
