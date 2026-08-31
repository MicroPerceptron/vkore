// Linux QEMU adapter for the frozen HostLink serial/ivshmem A/B workload.
//
// Guest timing/CPU witnesses are mandatory. Host/QEMU CPU and transport
// counters are measured here; absent instrumentation is an error, never zero.

use std::collections::VecDeque;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hostlinkd::backend::BackendError;
use hostlinkd::{
    BackendEvent, Config, DeterministicBackend, HostLinkDaemon, HybridError, HybridHostLink,
    InferenceBackend, IvshmemBulkHost, SequenceIdentity,
};
use kore_hostlink::{RequestId, RouteId};
use kore_hostlink_ab::{
    AcceptanceWitness, GuestMeasurement, GuestPayloadMetrics, HostTransportMetrics,
    MEASUREMENT_ARM_PREFIX, MEASUREMENT_ENABLED_PREFIX, MEASUREMENT_FENCE_PREFIX,
    MEASUREMENT_VARIANT_IVSHMEM, MEASUREMENT_VARIANT_SERIAL, MeasurementProvenance,
    OBSERVATION_SCHEMA, Observation, PAYLOAD_SWEEP, PayloadMetrics, Variant, validate_observation,
};

const BEGIN: &str = "HOSTLINK_AB_PAYLOAD_BEGIN ";
const END: &str = "HOSTLINK_AB_PAYLOAD_END ";
const RESULT: &str = "HOSTLINK_AB_RESULT ";
const FAULT: &str = "HOSTLINK_AB_FAULT ";
const MAX_QEMU_OUTPUT: usize = 4 << 20;
const MAX_LINE: usize = 1 << 20;
const MAX_QEMU_ARGS: usize = 128;
const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
const MAX_TIMEOUT_SECONDS: u64 = 3_600;
const MEASUREMENT_CONTROL_WRITE_CHUNK: usize = 64;

fn main() {
    if let Err(error) = entry() {
        eprintln!("hostlink-measure-adapter: {error}");
        std::process::exit(2);
    }
}

fn entry() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("measurement execution requires Linux".into());
    }
    let cli = Cli::parse()?;
    let qemu_args = load_qemu_args(cli.qemu_arg_file.as_deref())?;
    let runtime = RuntimeDirectory::create()?;
    let paths = RuntimePaths::new(runtime.path());
    let resolved = resolve_qemu_args(&qemu_args, &paths, &cli.image, cli.transport)?;

    let provenance = MeasurementProvenance {
        adapter_blake3: hash_file(&env::current_exe().map_err(|e| e.to_string())?)?,
        image_blake3: hash_file(&cli.image)?,
        qemu_blake3: hash_file(&cli.qemu)?,
        qemu_args: resolved.clone(),
    };
    let phase_gate = Arc::new(AtomicU8::new(0));
    let traffic = Arc::new(PhaseTraffic::new());
    let mut engine = Engine::create(
        cli.transport,
        &paths,
        Arc::clone(&phase_gate),
        Arc::clone(&traffic),
    )?;
    let mut command = Command::new(&cli.qemu);
    command
        .args(&resolved)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGuard::spawn(command)?;
    let qemu_pid = child.id();
    let stdout = child.take_stdout()?;
    let stderr = child.take_stderr()?;
    let (line_tx, line_rx) = sync_channel(64);
    let stdout_reader = thread::spawn(move || read_lines(stdout, line_tx));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_QEMU_OUTPUT));

    let timeout = Duration::from_secs(cli.timeout_seconds);
    let started = Instant::now();
    let mut capture = Capture::new(phase_gate, traffic);
    loop {
        engine.tick()?;
        drain_lines(&line_rx, &mut capture, &mut engine, qemu_pid)?;
        if capture.is_complete() {
            child.terminate()?;
            // Termination closes QEMU's pipes, but the bounded reader may be
            // blocked behind queued suffix lines. Drain to channel closure
            // before joining it.
            drain_lines_until_closed(&line_rx, &mut capture, &mut engine, qemu_pid)?;
            break;
        }
        if let Some(status) = child.try_wait()? {
            drain_lines_until_closed(&line_rx, &mut capture, &mut engine, qemu_pid)?;
            if capture.is_complete() {
                break;
            }
            return Err(format!(
                "QEMU exited with {status} before complete measurement testimony"
            ));
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "QEMU measurement exceeded {} seconds",
                cli.timeout_seconds
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }

    let stdout_result = stdout_reader
        .join()
        .map_err(|_| "QEMU stdout reader panicked".to_owned())?
        .map_err(|e| format!("cannot read QEMU stdout: {e}"))?;
    if stdout_result > MAX_QEMU_OUTPUT {
        return Err("QEMU stdout exceeded the bounded capture".into());
    }
    let stderr = stderr_reader
        .join()
        .map_err(|_| "QEMU stderr reader panicked".to_owned())?
        .map_err(|e| format!("cannot read QEMU stderr: {e}"))?;
    if stderr.overflowed {
        return Err("QEMU stderr exceeded the bounded capture".into());
    }
    if !stderr.bytes.is_empty() {
        eprintln!("{}", String::from_utf8_lossy(&stderr.bytes).trim_end());
    }

    let guest = capture
        .guest
        .take()
        .ok_or_else(|| "missing mandatory HOSTLINK_AB_RESULT guest telemetry".to_owned())?;
    let observation = capture.finish(guest, engine.metrics(), provenance, cli.transport)?;
    validate_observation(&observation, cli.transport.variant())
        .map_err(|e| format!("guest measurement refused: {e:?}"))?;
    emit_gate_markers(observation.acceptance);
    serde_json::to_writer(std::io::stdout().lock(), &observation)
        .map_err(|e| format!("cannot encode observation: {e}"))?;
    println!();
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Transport {
    Serial,
    Ivshmem,
}

impl Transport {
    const fn variant(self) -> Variant {
        match self {
            Self::Serial => Variant::Serial,
            Self::Ivshmem => Variant::Ivshmem,
        }
    }
}

struct Cli {
    transport: Transport,
    qemu: PathBuf,
    image: PathBuf,
    qemu_arg_file: Option<PathBuf>,
    timeout_seconds: u64,
}

impl Cli {
    fn parse() -> Result<Self, String> {
        let mut transport = None;
        let mut qemu = None;
        let mut image = None;
        let mut qemu_arg_file = None;
        let mut timeout_seconds = env::var("KORE_HOSTLINK_AB_TIMEOUT_SECONDS")
            .ok()
            .map(|value| value.parse())
            .transpose()
            .map_err(|_| "invalid KORE_HOSTLINK_AB_TIMEOUT_SECONDS")?
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
        let mut args = env::args_os().skip(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--transport") => {
                    transport = Some(
                        match args.next().and_then(|v| v.into_string().ok()).as_deref() {
                            Some("serial") => Transport::Serial,
                            Some("ivshmem") => Transport::Ivshmem,
                            _ => return Err("--transport must be serial or ivshmem".into()),
                        },
                    );
                }
                Some("--qemu") => qemu = args.next().map(PathBuf::from),
                Some("--image") => image = args.next().map(PathBuf::from),
                Some("--qemu-arg-file") => qemu_arg_file = args.next().map(PathBuf::from),
                Some("--timeout-seconds") => {
                    timeout_seconds = args
                        .next()
                        .and_then(|v| v.into_string().ok())
                        .and_then(|v| v.parse().ok())
                        .ok_or("invalid --timeout-seconds")?;
                }
                _ => return Err(Self::usage()),
            }
        }
        if timeout_seconds == 0 || timeout_seconds > MAX_TIMEOUT_SECONDS {
            return Err("timeout must be in 1..=3600 seconds".into());
        }
        Ok(Self {
            transport: transport.ok_or_else(Self::usage)?,
            qemu: qemu.ok_or_else(Self::usage)?,
            image: image.ok_or_else(Self::usage)?,
            qemu_arg_file,
            timeout_seconds,
        })
    }

    fn usage() -> String {
        "usage: hostlink-measure-adapter --transport serial|ivshmem --qemu PATH --image PATH [--qemu-arg-file JSON] [--timeout-seconds N]".into()
    }
}

fn load_qemu_args(file: Option<&Path>) -> Result<Vec<String>, String> {
    let bytes = match file {
        Some(path) => fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        None => env::var("KORE_HOSTLINK_AB_QEMU_ARGS_JSON")
            .map_err(|_| "KORE_HOSTLINK_AB_QEMU_ARGS_JSON or --qemu-arg-file is required")?
            .into_bytes(),
    };
    if bytes.len() > MAX_QEMU_OUTPUT {
        return Err("QEMU argument JSON is too large".into());
    }
    let args: Vec<String> =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid QEMU argument JSON: {e}"))?;
    if args.is_empty()
        || args.len() > MAX_QEMU_ARGS
        || args
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('\0'))
    {
        return Err("QEMU argument vector is empty or outside bounds".into());
    }
    Ok(args)
}

struct RuntimePaths {
    control: PathBuf,
    bulk: PathBuf,
    shmem: PathBuf,
}

impl RuntimePaths {
    fn new(root: &Path) -> Self {
        Self {
            control: root.join("control.sock"),
            bulk: root.join("bulk.sock"),
            shmem: root.join("bulk.mem"),
        }
    }
}

fn resolve_qemu_args(
    input: &[String],
    paths: &RuntimePaths,
    image: &Path,
    transport: Transport,
) -> Result<Vec<String>, String> {
    let values = [
        ("@HOSTLINK_CONTROL_SOCKET@", paths.control.as_os_str()),
        ("@HOSTLINK_BULK_SOCKET@", paths.bulk.as_os_str()),
        ("@HOSTLINK_SHMEM_PATH@", paths.shmem.as_os_str()),
        ("@KORE_IMAGE@", image.as_os_str()),
    ];
    let mut seen = [false; 4];
    let mut size_seen = false;
    let mut output = Vec::with_capacity(input.len());
    for arg in input {
        let mut resolved = arg.clone();
        for (index, (token, value)) in values.iter().enumerate() {
            let value = value.to_str().ok_or("non-UTF-8 runtime path")?;
            if value.contains([',', '\n', '\r', '\0']) {
                return Err("runtime path is unsafe in a QEMU option".into());
            }
            if resolved.contains(token) {
                seen[index] = true;
                resolved = resolved.replace(token, value);
            }
        }
        if resolved.contains("@HOSTLINK_SHMEM_SIZE@") {
            size_seen = true;
            resolved = resolved.replace(
                "@HOSTLINK_SHMEM_SIZE@",
                &hostlinkd::IVSHMEM_BACKING_BYTES.to_string(),
            );
        }
        if resolved.contains("@HOSTLINK_") || resolved.contains("@KORE_IMAGE@") {
            return Err("QEMU arguments contain an unknown/unresolved runtime token".into());
        }
        output.push(resolved);
    }
    if !seen[0]
        || !seen[1]
        || !seen[3]
        || (transport == Transport::Ivshmem && (!seen[2] || !size_seen))
    {
        return Err("QEMU args are missing required runtime path tokens".into());
    }
    Ok(output)
}

struct RuntimeDirectory(PathBuf);

impl RuntimeDirectory {
    fn create() -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        for suffix in 0..16u8 {
            let path = env::temp_dir().join(format!(
                "kore-hostlink-ab-{}-{nonce}-{suffix}",
                std::process::id()
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot create private runtime directory: {e}")),
            }
        }
        Err("cannot allocate unique runtime directory".into())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for RuntimeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

struct OwnedListener {
    listener: UnixListener,
    path: PathBuf,
    dev: u64,
    ino: u64,
}

impl OwnedListener {
    fn bind(path: &Path) -> Result<Self, String> {
        let listener =
            UnixListener::bind(path).map_err(|e| format!("cannot bind {}: {e}", path.display()))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        Ok(Self {
            listener,
            path: path.to_owned(),
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    fn accept(&self) -> Result<Option<UnixStream>, String> {
        match self.listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(format!("socket accept failed: {e}")),
        }
    }
}

impl Drop for OwnedListener {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.dev() == self.dev
            && metadata.ino() == self.ino
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct EngineMetrics {
    transport: HostTransportMetrics,
    explicit_copies: Option<u64>,
    io_calls: u64,
}

struct PendingArm {
    sequence: SequenceIdentity,
    request: RequestId,
    payload: Vec<u8>,
    tick: u64,
    phase: u8,
}

struct PendingFence {
    sequence: SequenceIdentity,
    request: RequestId,
    payload: Vec<u8>,
    tick: u64,
    phase: u8,
}

struct MeasuredRequest {
    route: RouteId,
    request: RequestId,
    phase: usize,
    route_bit: u8,
}

struct EnabledReply {
    route: RouteId,
    request: RequestId,
    phase: u8,
}

struct PhaseTraffic {
    masks: [AtomicU8; PAYLOAD_SWEEP.len()],
}

impl PhaseTraffic {
    fn new() -> Self {
        Self {
            masks: std::array::from_fn(|_| AtomicU8::new(0)),
        }
    }

    fn mark(&self, phase: usize, route_bit: u8) {
        if let Some(mask) = self.masks.get(phase) {
            mask.fetch_or(route_bit, Ordering::Release);
        }
    }

    fn mask(&self, phase: usize) -> u8 {
        self.masks
            .get(phase)
            .map_or(0, |mask| mask.load(Ordering::Acquire))
    }
}

struct MeasurementBackend {
    inner: DeterministicBackend,
    gate: Arc<AtomicU8>,
    waiting: VecDeque<PendingArm>,
    fences: VecDeque<PendingFence>,
    variant: Transport,
    traffic: Arc<PhaseTraffic>,
    routes: Vec<(SequenceIdentity, u8)>,
    measured: Vec<MeasuredRequest>,
    enabled_replies: Vec<EnabledReply>,
}

impl MeasurementBackend {
    fn new(gate: Arc<AtomicU8>, variant: Transport, traffic: Arc<PhaseTraffic>) -> Self {
        Self {
            inner: DeterministicBackend::new(2),
            gate,
            waiting: VecDeque::with_capacity(PAYLOAD_SWEEP.len()),
            fences: VecDeque::with_capacity(PAYLOAD_SWEEP.len()),
            variant,
            traffic,
            routes: Vec::with_capacity(3),
            measured: Vec::with_capacity(hostlinkd::backend::PENDING_TOTAL_MAX),
            enabled_replies: Vec::with_capacity(PAYLOAD_SWEEP.len() * 2),
        }
    }

    fn reserved_phase(&self, payload: &[u8], prefix: &[u8]) -> Result<Option<u8>, BackendError> {
        if !payload.starts_with(prefix) {
            return Ok(None);
        }
        if payload.len() != prefix.len() + 2 {
            return Err(BackendError::InvalidEncoding);
        }
        let variant = payload[prefix.len()];
        let expected_variant = match self.variant {
            Transport::Serial => MEASUREMENT_VARIANT_SERIAL,
            Transport::Ivshmem => MEASUREMENT_VARIANT_IVSHMEM,
        };
        let phase = payload[prefix.len() + 1];
        if variant != expected_variant || (phase as usize) >= PAYLOAD_SWEEP.len() {
            return Err(BackendError::InvalidEncoding);
        }
        Ok(Some(phase))
    }
}

impl InferenceBackend for MeasurementBackend {
    fn reset(&mut self) {
        self.inner.reset();
        self.waiting.clear();
        self.fences.clear();
        self.routes.clear();
        self.measured.clear();
        self.enabled_replies.clear();
    }

    fn open(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError> {
        self.inner.open(sequence)?;
        let role_count = self
            .routes
            .iter()
            .filter(|(bound, _)| bound.role == sequence.role)
            .count();
        let bit = match (sequence.role, role_count) {
            (hostlinkd::BackendRole::Broker, 0) => 1,
            (hostlinkd::BackendRole::Agent, 0) => 2,
            (hostlinkd::BackendRole::Agent, 1) => 4,
            _ => return Err(BackendError::RoleCapacity),
        };
        self.routes.push((sequence, bit));
        Ok(())
    }

    fn close(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError> {
        self.waiting.retain(|pending| pending.sequence != sequence);
        self.fences.retain(|pending| pending.sequence != sequence);
        self.routes.retain(|(bound, _)| *bound != sequence);
        self.measured
            .retain(|pending| pending.route != sequence.route);
        self.enabled_replies
            .retain(|pending| pending.route != sequence.route);
        self.inner.close(sequence)
    }

    fn submit(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        let arm_phase = self.reserved_phase(payload, MEASUREMENT_ARM_PREFIX)?;
        let fence_phase = self.reserved_phase(payload, MEASUREMENT_FENCE_PREFIX)?;
        if let Some(phase) = fence_phase {
            let is_broker = self
                .routes
                .iter()
                .any(|(bound, route_bit)| *bound == sequence && *route_bit == 1);
            if !is_broker {
                return Err(BackendError::InvalidRole);
            }
            if self
                .fences
                .iter()
                .any(|pending| pending.sequence == sequence && pending.request == request)
            {
                return Err(BackendError::Duplicate);
            }
            if self.traffic.mask(phase as usize) == 0b111 {
                return self.inner.submit(sequence, request, payload, tick);
            }
            if self.fences.len() == PAYLOAD_SWEEP.len() {
                return Err(BackendError::Full);
            }
            self.fences.push_back(PendingFence {
                sequence,
                request,
                payload: payload.to_vec(),
                tick,
                phase,
            });
            return Ok(());
        }
        let Some(phase) = arm_phase else {
            let measured = self
                .gate
                .load(Ordering::Acquire)
                .checked_sub(1)
                .map(usize::from)
                .filter(|phase| *phase < PAYLOAD_SWEEP.len())
                .and_then(|phase| {
                    self.routes
                        .iter()
                        .find(|(bound, _)| *bound == sequence)
                        .map(|(_, bit)| (phase, *bit))
                });
            let result = self.inner.submit(sequence, request, payload, tick);
            if result.is_ok()
                && let Some((phase, route_bit)) = measured
            {
                self.measured.push(MeasuredRequest {
                    route: sequence.route,
                    request,
                    phase,
                    route_bit,
                });
            }
            return result;
        };
        if self
            .waiting
            .iter()
            .any(|pending| pending.sequence == sequence && pending.request == request)
        {
            return Err(BackendError::Duplicate);
        }
        if self.gate.load(Ordering::Acquire) > phase {
            return self.inner.submit(sequence, request, payload, tick);
        }
        if self.waiting.len() == PAYLOAD_SWEEP.len() {
            return Err(BackendError::Full);
        }
        self.waiting.push_back(PendingArm {
            sequence,
            request,
            payload: payload.to_vec(),
            tick,
            phase,
        });
        Ok(())
    }

    fn cancel(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        tick: u64,
    ) -> Result<BackendEvent, BackendError> {
        if let Some(index) = self
            .waiting
            .iter()
            .position(|pending| pending.sequence == sequence && pending.request == request)
        {
            let pending = self.waiting.remove(index).expect("located pending arm");
            return Ok(BackendEvent::Cancelled {
                sequence,
                request,
                submitted_tick: pending.tick,
                turn: 0,
            });
        }
        if let Some(index) = self
            .fences
            .iter()
            .position(|pending| pending.sequence == sequence && pending.request == request)
        {
            let pending = self.fences.remove(index).expect("located pending fence");
            return Ok(BackendEvent::Cancelled {
                sequence,
                request,
                submitted_tick: pending.tick,
                turn: 0,
            });
        }
        let event = self.inner.cancel(sequence, request, tick)?;
        self.measured
            .retain(|pending| pending.route != sequence.route || pending.request != request);
        self.enabled_replies
            .retain(|pending| pending.route != sequence.route || pending.request != request);
        Ok(event)
    }

    fn poll(&mut self, tick: u64) -> Option<BackendEvent> {
        if let Some(index) = self
            .waiting
            .iter()
            .position(|pending| self.gate.load(Ordering::Acquire) > pending.phase)
        {
            let pending = self.waiting.remove(index).expect("located released arm");
            match self.inner.submit(
                pending.sequence,
                pending.request,
                &pending.payload,
                pending.tick,
            ) {
                Ok(()) => self.enabled_replies.push(EnabledReply {
                    route: pending.sequence.route,
                    request: pending.request,
                    phase: pending.phase,
                }),
                Err(BackendError::Full) => self.waiting.insert(index, pending),
                Err(_) => {}
            }
        }
        if let Some(index) = self
            .fences
            .iter()
            .position(|pending| self.traffic.mask(pending.phase as usize) == 0b111)
        {
            let pending = self.fences.remove(index).expect("located released fence");
            match self.inner.submit(
                pending.sequence,
                pending.request,
                &pending.payload,
                pending.tick,
            ) {
                Ok(()) => self.enabled_replies.push(EnabledReply {
                    route: pending.sequence.route,
                    request: pending.request,
                    phase: pending.phase,
                }),
                Err(BackendError::Full) => self.fences.insert(index, pending),
                Err(_) => {}
            }
        }
        let mut event = self.inner.poll(tick)?;
        if let BackendEvent::Completed {
            sequence,
            request,
            payload,
            ..
        } = &mut event
            && let Some(index) = self
                .enabled_replies
                .iter()
                .position(|pending| pending.route == sequence.route && pending.request == *request)
        {
            let enabled = self.enabled_replies.remove(index);
            payload.clear();
            payload.extend_from_slice(MEASUREMENT_ENABLED_PREFIX);
            payload.push(match self.variant {
                Transport::Serial => MEASUREMENT_VARIANT_SERIAL,
                Transport::Ivshmem => MEASUREMENT_VARIANT_IVSHMEM,
            });
            payload.push(enabled.phase);
        }
        if let BackendEvent::Completed {
            sequence, request, ..
        } = &event
            && let Some(index) = self
                .measured
                .iter()
                .position(|pending| pending.route == sequence.route && pending.request == *request)
        {
            let measured = self.measured.remove(index);
            self.traffic.mark(measured.phase, measured.route_bit);
        }
        Some(event)
    }

    fn metrics(&self) -> hostlinkd::BackendMetrics {
        self.inner.metrics()
    }

    fn sequence_metrics(&self, sequence: SequenceIdentity) -> Option<hostlinkd::SequenceMetrics> {
        self.inner.sequence_metrics(sequence)
    }
}

enum Engine {
    Serial(Box<HostLinkDaemon<MeasurementBackend>>),
    Ivshmem(Box<HybridEngine>),
}

impl Engine {
    fn create(
        transport: Transport,
        paths: &RuntimePaths,
        gate: Arc<AtomicU8>,
        traffic: Arc<PhaseTraffic>,
    ) -> Result<Self, String> {
        match transport {
            Transport::Serial => HostLinkDaemon::bind_with_write_chunks(
                Config {
                    control_socket: paths.control.clone(),
                    bulk_socket: paths.bulk.clone(),
                },
                MeasurementBackend::new(Arc::clone(&gate), Transport::Serial, Arc::clone(&traffic)),
                (),
                MEASUREMENT_CONTROL_WRITE_CHUNK,
                usize::MAX,
            )
            .map(Box::new)
            .map(Self::Serial)
            .map_err(|e| format!("cannot bind serial host: {e:?}")),
            Transport::Ivshmem => HybridEngine::new(paths, gate, traffic)
                .map(Box::new)
                .map(Self::Ivshmem),
        }
    }

    fn tick(&mut self) -> Result<(), String> {
        match self {
            Self::Serial(daemon) => daemon.tick().map_err(|e| format!("serial host: {e:?}")),
            Self::Ivshmem(engine) => engine.tick(),
        }
    }

    fn metrics(&self) -> EngineMetrics {
        match self {
            Self::Serial(daemon) => {
                let m = daemon.core().metrics();
                EngineMetrics {
                    transport: HostTransportMetrics {
                        frames_in: m.control_frames_in + m.bulk_frames_in,
                        frames_out: m.frames_out,
                        bytes_in: m.bytes_in,
                        bytes_out: m.bytes_out,
                        backpressure: m.backpressure,
                        disconnects: m.disconnects,
                        socket_read_calls: m.io_read_calls,
                        socket_write_calls: m.io_write_calls,
                        partial_write_calls: m.partial_write_calls,
                        ..HostTransportMetrics::default()
                    },
                    explicit_copies: None,
                    io_calls: m.io_read_calls + m.io_write_calls,
                }
            }
            Self::Ivshmem(engine) => engine.metrics(),
        }
    }

    fn epoch(&self) -> Option<kore_hostlink::LinkEpoch> {
        match self {
            Self::Serial(daemon) => daemon.core().epoch(),
            Self::Ivshmem(engine) => engine.link.as_ref()?.core().epoch(),
        }
    }

    fn force_disconnect(&mut self) -> Result<(), String> {
        match self {
            Self::Serial(daemon) => {
                daemon.force_disconnect();
                Ok(())
            }
            Self::Ivshmem(engine) => engine.force_disconnect(),
        }
    }
}

struct HybridEngine {
    control_listener: OwnedListener,
    bulk_listener: OwnedListener,
    serial_bulk_peer: Option<UnixStream>,
    link: Option<HybridHostLink<MeasurementBackend>>,
    bulk: Option<IvshmemBulkHost>,
    retired: EngineMetrics,
    gate: Arc<AtomicU8>,
    traffic: Arc<PhaseTraffic>,
}

impl HybridEngine {
    fn new(
        paths: &RuntimePaths,
        gate: Arc<AtomicU8>,
        traffic: Arc<PhaseTraffic>,
    ) -> Result<Self, String> {
        Ok(Self {
            control_listener: OwnedListener::bind(&paths.control)?,
            bulk_listener: OwnedListener::bind(&paths.bulk)?,
            serial_bulk_peer: None,
            link: None,
            bulk: Some(
                IvshmemBulkHost::create(&paths.shmem).map_err(|e| format!("ivshmem: {e:?}"))?,
            ),
            retired: EngineMetrics {
                explicit_copies: Some(0),
                ..EngineMetrics::default()
            },
            gate,
            traffic,
        })
    }

    fn tick(&mut self) -> Result<(), String> {
        if self.serial_bulk_peer.is_none() {
            self.serial_bulk_peer = self.bulk_listener.accept()?;
        }
        if self.link.is_none()
            && let Some(stream) = self.control_listener.accept()?
        {
            let bulk = self.bulk.take().ok_or("ivshmem mapping is unavailable")?;
            self.link = Some(
                HybridHostLink::new_with_control_write_chunk(
                    stream,
                    bulk,
                    MeasurementBackend::new(
                        Arc::clone(&self.gate),
                        Transport::Ivshmem,
                        Arc::clone(&self.traffic),
                    ),
                    (),
                    MEASUREMENT_CONTROL_WRITE_CHUNK,
                )
                .map_err(|e| format!("hybrid link: {e:?}"))?,
            );
        }
        let Some(link) = &mut self.link else {
            return Ok(());
        };
        match link.tick() {
            Ok(()) => Ok(()),
            Err(HybridError::Disconnected) => {
                link.disconnect();
                self.retire_link()
            }
            Err(e) => Err(format!("hybrid host: {e:?}")),
        }
    }

    fn force_disconnect(&mut self) -> Result<(), String> {
        let Some(link) = &mut self.link else {
            // A routed refusal may already have retired both planes. The
            // evaluator's explicit fault action is idempotent in that state.
            return Ok(());
        };
        link.disconnect();
        self.retire_link()
    }

    fn retire_link(&mut self) -> Result<(), String> {
        let link = self.link.as_ref().ok_or("hybrid link is absent")?;
        let mut current = hybrid_metrics(link);
        // The mapping survives reconnect and its counters are cumulative.
        // Retire only per-LinkCore/socket counters here; the live mapping
        // contributes shared counters once.
        current.transport.shmem_publications = 0;
        current.transport.shmem_records_received = 0;
        current.transport.shmem_copies = 0;
        current.explicit_copies = Some(0);
        add_engine_metrics(&mut self.retired, current);
        let link = self.link.take().expect("link was present");
        self.bulk = Some(link.into_bulk());
        self.serial_bulk_peer = None;
        Ok(())
    }

    fn metrics(&self) -> EngineMetrics {
        let mut metrics = self.retired;
        if let Some(link) = &self.link {
            add_engine_metrics(&mut metrics, hybrid_metrics(link));
        } else if let Some(bulk) = &self.bulk {
            let m = bulk.metrics();
            metrics.transport.shmem_publications = m.publications;
            metrics.transport.shmem_records_received = m.records_received;
            metrics.transport.shmem_copies = m.copies;
            metrics.explicit_copies = Some(m.copies);
        }
        metrics
    }
}

fn hybrid_metrics(link: &HybridHostLink<MeasurementBackend>) -> EngineMetrics {
    let core = link.core().metrics();
    let hybrid = link.metrics();
    let bulk = link.bulk().metrics();
    EngineMetrics {
        transport: HostTransportMetrics {
            frames_in: core.control_frames_in + core.bulk_frames_in,
            frames_out: core.frames_out,
            bytes_in: core.bytes_in,
            bytes_out: core.bytes_out,
            backpressure: core.backpressure,
            disconnects: core.disconnects,
            shmem_publications: bulk.publications,
            shmem_records_received: bulk.records_received,
            shmem_copies: bulk.copies,
            shmem_wake_frames: hybrid.wake_frames.saturating_add(hybrid.wake_frames_in),
            socket_read_calls: hybrid.control_reads,
            socket_write_calls: hybrid.control_writes,
            partial_write_calls: hybrid.partial_control_writes,
        },
        explicit_copies: Some(bulk.copies),
        io_calls: hybrid.control_reads + hybrid.control_writes,
    }
}

fn add_engine_metrics(target: &mut EngineMetrics, value: EngineMetrics) {
    macro_rules! add {
        ($field:ident) => {
            target.transport.$field = target
                .transport
                .$field
                .saturating_add(value.transport.$field)
        };
    }
    add!(frames_in);
    add!(frames_out);
    add!(bytes_in);
    add!(bytes_out);
    add!(backpressure);
    add!(disconnects);
    add!(shmem_publications);
    add!(shmem_records_received);
    add!(shmem_copies);
    add!(shmem_wake_frames);
    add!(socket_read_calls);
    add!(socket_write_calls);
    add!(partial_write_calls);
    target.io_calls = target.io_calls.saturating_add(value.io_calls);
    target.explicit_copies = match (target.explicit_copies, value.explicit_copies) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        _ => None,
    };
}

struct Capture {
    phase: Option<PhaseStart>,
    phases: Vec<MeasuredPhase>,
    guest: Option<GuestMeasurement>,
    fault: FaultCapture,
    phase_gate: Arc<AtomicU8>,
    traffic: Arc<PhaseTraffic>,
}

#[derive(Default)]
struct FaultCapture {
    before: Option<kore_hostlink::LinkEpoch>,
    after: Option<kore_hostlink::LinkEpoch>,
    disconnect_caused: bool,
    guest_resumed: bool,
}

struct PhaseStart {
    payload: u32,
    process_cpu_ns: u64,
    qemu_cpu_ns: u64,
    metrics: EngineMetrics,
}

struct MeasuredPhase {
    guest: GuestPayloadMetrics,
    host_cpu_ns_per_byte: f64,
    qemu_cpu_ns_per_byte: f64,
    copies: Option<u64>,
    io_calls: u64,
}

impl Capture {
    fn new(phase_gate: Arc<AtomicU8>, traffic: Arc<PhaseTraffic>) -> Self {
        Self {
            phase: None,
            phases: Vec::with_capacity(PAYLOAD_SWEEP.len()),
            guest: None,
            fault: FaultCapture::default(),
            phase_gate,
            traffic,
        }
    }

    fn is_complete(&self) -> bool {
        self.guest.is_some()
            && self.phase.is_none()
            && self.phases.len() == PAYLOAD_SWEEP.len()
            && self.fault.disconnect_caused
            && self.fault.guest_resumed
    }

    fn line(&mut self, line: &str, engine: &mut Engine, qemu_pid: u32) -> Result<(), String> {
        if let Some(value) = line.strip_prefix(BEGIN) {
            if self.phase.is_some() {
                return Err("nested payload measurement phase".into());
            }
            let payload = value
                .trim()
                .parse()
                .map_err(|_| "invalid payload begin marker")?;
            let phase = self.phases.len();
            if PAYLOAD_SWEEP.get(phase) != Some(&payload) {
                return Err("payload begin marker is outside the ordered frozen sweep".into());
            }
            self.phase = Some(PhaseStart {
                payload,
                process_cpu_ns: process_cpu_ns()?,
                qemu_cpu_ns: qemu_cpu_ns(qemu_pid)?,
                metrics: engine.metrics(),
            });
            // Release only after both host and QEMU counters have been
            // sampled. The backend withholds the phase-arm inference reply
            // until this store, and the guest starts traffic only on reply.
            self.phase_gate.store((phase + 1) as u8, Ordering::Release);
        } else if let Some(value) = line.strip_prefix(END) {
            let guest: GuestPayloadMetrics = serde_json::from_str(value)
                .map_err(|e| format!("invalid payload end telemetry: {e}"))?;
            let begin = self.phase.take().ok_or("payload end without begin")?;
            let phase = self.phases.len();
            if begin.payload != guest.payload_bytes || guest.transferred_bytes == 0 {
                return Err("payload begin/end mismatch or zero transfer".into());
            }
            if self.traffic.mask(phase) != 0b111 {
                return Err(
                    "measured phase lacks host-observed Broker plus agent slots 0 and 1 traffic".into(),
                );
            }
            let host_end = process_cpu_ns()?;
            let qemu_end = qemu_cpu_ns(qemu_pid)?;
            let end = engine.metrics();
            self.phases.push(MeasuredPhase {
                host_cpu_ns_per_byte: host_end.saturating_sub(begin.process_cpu_ns) as f64
                    / guest.transferred_bytes as f64,
                qemu_cpu_ns_per_byte: qemu_end.saturating_sub(begin.qemu_cpu_ns) as f64
                    / guest.transferred_bytes as f64,
                copies: delta_option(end.explicit_copies, begin.metrics.explicit_copies),
                io_calls: end.io_calls.saturating_sub(begin.metrics.io_calls),
                guest,
            });
        } else if let Some(value) = line.strip_prefix(RESULT) {
            if self.guest.is_some() {
                return Err("duplicate guest result marker".into());
            }
            self.guest = Some(
                serde_json::from_str(value)
                    .map_err(|e| format!("invalid guest result telemetry: {e}"))?,
            );
        } else if let Some(value) = line.strip_prefix(FAULT) {
            match value.trim() {
                "disconnect" => {
                    if self.fault.disconnect_caused {
                        return Err("duplicate disconnect fault marker".into());
                    }
                    let before = engine
                        .epoch()
                        .ok_or("disconnect fault requested before active link")?;
                    engine.force_disconnect()?;
                    self.fault.before = Some(before);
                    self.fault.disconnect_caused = true;
                }
                "resumed" => {
                    let before = self
                        .fault
                        .before
                        .ok_or("resumed marker before disconnect")?;
                    let after = engine
                        .epoch()
                        .ok_or("guest resumed before host renegotiated")?;
                    if after == before {
                        return Err("reconnect reused the retired LinkEpoch".into());
                    }
                    self.fault.after = Some(after);
                    self.fault.guest_resumed = true;
                }
                _ => return Err("unknown HOSTLINK_AB_FAULT marker".into()),
            }
        }
        Ok(())
    }

    fn finish(
        self,
        guest: GuestMeasurement,
        metrics: EngineMetrics,
        provenance: MeasurementProvenance,
        transport: Transport,
    ) -> Result<Observation, String> {
        if self.phase.is_some() || self.phases.len() != PAYLOAD_SWEEP.len() {
            return Err("incomplete payload phase telemetry".into());
        }
        if guest.schema != OBSERVATION_SCHEMA
            || guest.variant != transport.variant()
            || guest.payloads.len() != PAYLOAD_SWEEP.len()
        {
            return Err("guest result schema/variant/sweep mismatch".into());
        }
        let mut payloads = Vec::with_capacity(PAYLOAD_SWEEP.len());
        for (index, phase) in self.phases.into_iter().enumerate() {
            let expected = PAYLOAD_SWEEP[index];
            let summary = guest.payloads[index];
            if phase.guest != summary || summary.payload_bytes != expected {
                return Err("guest summary differs from phase telemetry".into());
            }
            payloads.push(PayloadMetrics {
                payload_bytes: summary.payload_bytes,
                latency_p50_us: summary.latency_p50_us,
                latency_p99_us: summary.latency_p99_us,
                throughput_mib_per_second: summary.throughput_mib_per_second,
                host_cpu_ns_per_byte: phase.host_cpu_ns_per_byte,
                guest_elapsed_ns_per_byte: summary.guest_elapsed_ns_per_byte,
                qemu_cpu_ns_per_byte: phase.qemu_cpu_ns_per_byte,
                cpu_ns_per_byte: phase.host_cpu_ns_per_byte + phase.qemu_cpu_ns_per_byte,
                adapter_copy_operations: phase.copies,
                host_io_calls: phase.io_calls,
            });
        }
        Ok(Observation {
            schema: OBSERVATION_SCHEMA,
            variant: transport.variant(),
            control_p50_us: guest.control_p50_us,
            control_p99_us: guest.control_p99_us,
            cancel_p50_us: guest.cancel_p50_us,
            cancel_p99_us: guest.cancel_p99_us,
            payloads,
            acceptance: AcceptanceWitness {
                streams: guest.workload.streams,
                interleaved: guest.workload.interleaved,
                control_under_bulk: guest.workload.control_under_bulk,
                cancel_under_bulk: guest.workload.cancel_under_bulk,
                saturation: saturation_witness(
                    guest.workload.local_backpressure,
                    metrics.transport.backpressure,
                ),
                partial_io: metrics.transport.partial_write_calls != 0,
                disconnect: self.fault.disconnect_caused,
                reconnect: self.fault.guest_resumed,
            },
            host_transport: metrics.transport,
            provenance,
            guest_boot_id: guest.boot_id,
            link_epochs: vec![
                epoch_hex(self.fault.before.ok_or("missing pre-fault epoch")?),
                epoch_hex(self.fault.after.ok_or("missing post-fault epoch")?),
            ],
        })
    }
}

fn delta_option(end: Option<u64>, start: Option<u64>) -> Option<u64> {
    Some(end?.saturating_sub(start?))
}

fn saturation_witness(local_backpressure: bool, host_backpressure: u64) -> bool {
    local_backpressure && host_backpressure != 0
}

fn epoch_hex(epoch: kore_hostlink::LinkEpoch) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(32);
    for byte in epoch.0 {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn drain_lines(
    receiver: &Receiver<String>,
    capture: &mut Capture,
    engine: &mut Engine,
    qemu_pid: u32,
) -> Result<(), String> {
    loop {
        match receiver.try_recv() {
            Ok(line) => capture.line(&line, engine, qemu_pid)?,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

fn drain_lines_until_closed(
    receiver: &Receiver<String>,
    capture: &mut Capture,
    engine: &mut Engine,
    qemu_pid: u32,
) -> Result<(), String> {
    while let Ok(line) = receiver.recv() {
        capture.line(&line, engine, qemu_pid)?;
    }
    Ok(())
}

fn read_lines(mut input: impl Read, sender: SyncSender<String>) -> io::Result<usize> {
    let mut reader = BufReader::new(&mut input);
    let mut total = 0usize;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(total);
        }
        total = total.saturating_add(bytes);
        if total > MAX_QEMU_OUTPUT || line.len() > MAX_LINE {
            return Ok(total);
        }
        let line = line.trim_end_matches(['\r', '\n']).to_owned();
        if sender.send(line).is_err() {
            return Ok(total);
        }
    }
}

struct Capped {
    bytes: Vec<u8>,
    overflowed: bool,
}

fn read_capped(mut input: impl Read, maximum: usize) -> io::Result<Capped> {
    let mut bytes = Vec::new();
    let mut overflowed = false;
    let mut chunk = [0; 8192];
    loop {
        let count = input.read(&mut chunk)?;
        if count == 0 {
            return Ok(Capped { bytes, overflowed });
        }
        let retain = count.min(maximum.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&chunk[..retain]);
        overflowed |= retain != count;
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    fn spawn(mut command: Command) -> Result<Self, String> {
        command
            .spawn()
            .map(Self)
            .map_err(|e| format!("cannot start QEMU: {e}"))
    }
    fn id(&self) -> u32 {
        self.0.id()
    }
    fn take_stdout(&mut self) -> Result<impl Read + Send + 'static, String> {
        self.0
            .stdout
            .take()
            .ok_or("QEMU stdout pipe missing".into())
    }
    fn take_stderr(&mut self) -> Result<impl Read + Send + 'static, String> {
        self.0
            .stderr
            .take()
            .ok_or("QEMU stderr pipe missing".into())
    }
    fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, String> {
        self.0
            .try_wait()
            .map_err(|e| format!("cannot poll QEMU: {e}"))
    }

    fn terminate(&mut self) -> Result<(), String> {
        self.0
            .kill()
            .map_err(|e| format!("cannot terminate measured QEMU: {e}"))?;
        self.0
            .wait()
            .map_err(|e| format!("cannot reap measured QEMU: {e}"))?;
        Ok(())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn hash_file(path: &Path) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let mut file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher).map_err(|e| format!("cannot hash {}: {e}", path.display()))?;
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(target_os = "linux")]
fn process_cpu_ns() -> Result<u64, String> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `value` is a valid writable timespec and the process clock has
    // no lifetime or aliasing requirements.
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut value) } != 0 {
        return Err(format!(
            "process CPU clock failed: {}",
            io::Error::last_os_error()
        ));
    }
    u64::try_from(value.tv_sec)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|seconds| {
            u64::try_from(value.tv_nsec)
                .ok()
                .and_then(|nanos| seconds.checked_add(nanos))
        })
        .ok_or("process CPU clock overflow".into())
}

#[cfg(not(target_os = "linux"))]
fn process_cpu_ns() -> Result<u64, String> {
    Err("process CPU measurement requires Linux".into())
}

#[cfg(target_os = "linux")]
fn qemu_cpu_ns(pid: u32) -> Result<u64, String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|e| format!("cannot read QEMU CPU counters: {e}"))?;
    let close = stat.rfind(')').ok_or("malformed QEMU /proc stat")?;
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let user: u64 = fields
        .get(11)
        .ok_or("QEMU /proc stat lacks utime")?
        .parse()
        .map_err(|_| "invalid QEMU utime")?;
    let system: u64 = fields
        .get(12)
        .ok_or("QEMU /proc stat lacks stime")?
        .parse()
        .map_err(|_| "invalid QEMU stime")?;
    // SAFETY: sysconf has no memory preconditions.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        return Err("cannot determine Linux clock ticks".into());
    }
    (user.saturating_add(system))
        .checked_mul(1_000_000_000)
        .map(|value| value / ticks as u64)
        .ok_or("QEMU CPU counter overflow".into())
}

#[cfg(not(target_os = "linux"))]
fn qemu_cpu_ns(_: u32) -> Result<u64, String> {
    Err("QEMU CPU measurement requires Linux".into())
}

fn emit_gate_markers(witness: AcceptanceWitness) {
    eprintln!("HOSTLINK_GATE streams={} interleaved=ok", witness.streams);
    eprintln!("HOSTLINK_GATE control-under-bulk=ok cancel-under-bulk=ok");
    eprintln!("HOSTLINK_GATE saturation=ok partial=ok");
    eprintln!(
        "HOSTLINK_GATE disconnect=ok reconnect=ok boot-id-recorded=ok aggregate-restart=pending"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use kore_hostlink::TaskGeneration;

    #[test]
    fn token_resolution_is_per_run_and_transport_checked() {
        let paths = RuntimePaths {
            control: "/tmp/private/control.sock".into(),
            bulk: "/tmp/private/bulk.sock".into(),
            shmem: "/tmp/private/bulk.mem".into(),
        };
        let args = vec![
            "@HOSTLINK_CONTROL_SOCKET@".into(),
            "@HOSTLINK_BULK_SOCKET@".into(),
            "@HOSTLINK_SHMEM_PATH@".into(),
            "@HOSTLINK_SHMEM_SIZE@".into(),
            "@KORE_IMAGE@".into(),
        ];
        let output = resolve_qemu_args(
            &args,
            &paths,
            Path::new("/tmp/kore.iso"),
            Transport::Ivshmem,
        )
        .unwrap();
        assert_eq!(output[0], "/tmp/private/control.sock");
        assert_eq!(output[2], "/tmp/private/bulk.mem");
        assert_eq!(output[3], hostlinkd::IVSHMEM_BACKING_BYTES.to_string());
        assert!(
            resolve_qemu_args(
                &args[1..],
                &paths,
                Path::new("/tmp/kore.iso"),
                Transport::Serial
            )
            .is_err()
        );
    }

    #[test]
    fn phase_arm_is_variant_bound_and_withheld_until_gate() {
        use hostlinkd::BackendRole;

        let gate = Arc::new(AtomicU8::new(0));
        let traffic = Arc::new(PhaseTraffic::new());
        let mut backend =
            MeasurementBackend::new(Arc::clone(&gate), Transport::Serial, Arc::clone(&traffic));
        let route = SequenceIdentity {
            route: RouteId([1; 16]),
            generation: TaskGeneration::new(1).unwrap(),
            role: BackendRole::Agent,
        };
        let request = RequestId([2; 16]);
        backend.open(route).unwrap();
        let mut arm = MEASUREMENT_ARM_PREFIX.to_vec();
        arm.extend_from_slice(&[MEASUREMENT_VARIANT_SERIAL, 0]);
        backend.submit(route, request, &arm, 0).unwrap();
        assert!(backend.poll(10).is_none());
        gate.store(1, Ordering::Release);
        assert!(backend.poll(1).is_none());
        let mut expected = MEASUREMENT_ENABLED_PREFIX.to_vec();
        expected.extend_from_slice(&[MEASUREMENT_VARIANT_SERIAL, 0]);
        assert!(matches!(
            backend.poll(2),
            Some(BackendEvent::Completed { request: id, payload, .. })
                if id == request && payload == expected
        ));

        let broker = SequenceIdentity {
            route: RouteId([4; 16]),
            generation: TaskGeneration::new(1).unwrap(),
            role: BackendRole::Broker,
        };
        let agent1 = SequenceIdentity {
            route: RouteId([5; 16]),
            generation: TaskGeneration::new(1).unwrap(),
            role: BackendRole::Agent,
        };
        backend.open(broker).unwrap();
        backend.open(agent1).unwrap();
        backend
            .submit(route, RequestId([6; 16]), b"agent0", 3)
            .unwrap();
        backend
            .submit(broker, RequestId([7; 16]), b"broker", 3)
            .unwrap();
        backend
            .submit(agent1, RequestId([8; 16]), b"agent1", 3)
            .unwrap();
        let fence_request = RequestId([9; 16]);
        let mut fence = MEASUREMENT_FENCE_PREFIX.to_vec();
        fence.extend_from_slice(&[MEASUREMENT_VARIANT_SERIAL, 0]);
        assert_eq!(
            backend.submit(route, RequestId([10; 16]), &fence, 3),
            Err(BackendError::InvalidRole)
        );
        backend.submit(broker, fence_request, &fence, 3).unwrap();
        assert_eq!(traffic.mask(0), 0);
        for _ in 0..3 {
            assert!(!matches!(
                backend.poll(5),
                Some(BackendEvent::Completed { request, .. }) if request == fence_request
            ));
        }
        assert_eq!(traffic.mask(0), 0b111);
        assert!(matches!(
            backend.poll(5),
            Some(BackendEvent::Completed { request, payload, .. })
                if request == fence_request && payload == expected
        ));

        let mut wrong_variant = MEASUREMENT_ARM_PREFIX.to_vec();
        wrong_variant.extend_from_slice(&[MEASUREMENT_VARIANT_IVSHMEM, 1]);
        assert_eq!(
            backend.submit(route, RequestId([3; 16]), &wrong_variant, 3),
            Err(BackendError::InvalidEncoding)
        );
    }

    #[test]
    fn bounded_stdout_reader_finishes_when_suffix_is_drained() {
        let input = (0..100)
            .map(|index| format!("diagnostic-{index}\n"))
            .collect::<String>();
        let input_len = input.len();
        let (sender, receiver) = sync_channel(4);
        let reader = thread::spawn(move || read_lines(input.as_bytes(), sender));
        let mut lines = 0;
        while receiver.recv().is_ok() {
            lines += 1;
        }
        assert_eq!(lines, 100);
        assert_eq!(reader.join().unwrap().unwrap(), input_len);
    }

    #[test]
    fn saturation_requires_local_and_host_evidence() {
        assert!(!saturation_witness(false, 0));
        assert!(!saturation_witness(true, 0));
        assert!(!saturation_witness(false, 1));
        assert!(saturation_witness(true, 1));
    }
}
