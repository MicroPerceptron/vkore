use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kore_hostlink_ab::{
    MAX_OBSERVATION_BYTES, MAX_STDERR_BYTES, Observation, PAYLOAD_SWEEP, Plan, REPORT_SCHEMA,
    Report, RunRecord, RunSpec, STREAMS, Thresholds, Variant, evaluate, require_ivshmem_platform,
    schedule, validate_observation,
};

const MAX_PLAN_BYTES: u64 = 256 << 10;
const MAX_REPORT_BYTES: u64 = 8 << 20;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn main() {
    if let Err(error) = entry() {
        eprintln!("hostlink-ab: {error}");
        std::process::exit(1);
    }
}

fn entry() -> Result<(), String> {
    let arguments: Vec<String> = env::args().collect();
    match arguments.as_slice() {
        [_, command, plan, output] if command == "run" => run(Path::new(plan), Path::new(output)),
        [_, command, report] if command == "evaluate" => evaluate_report(Path::new(report)),
        _ => Err(format!(
            "usage: {} run PLAN.json RAW.json | evaluate RAW.json",
            arguments
                .first()
                .map_or("vkore-hostlink-ab", String::as_str)
        )),
    }
}

fn run(plan_path: &Path, output_path: &Path) -> Result<(), String> {
    let plan_bytes = read_bounded(plan_path, MAX_PLAN_BYTES)?;
    let plan: Plan = serde_json::from_slice(&plan_bytes)
        .map_err(|error| format!("invalid plan JSON: {error}"))?;
    let geometry = plan
        .validate()
        .map_err(|error| format!("invalid plan: {error:?}"))?;

    // Refuse before running serial: a partial serial-only run is not the A/B
    // experiment and must not be mistaken for one on macOS or Windows.
    require_ivshmem_platform(cfg!(target_os = "linux")).map_err(str::to_owned)?;

    let run_schedule = schedule(plan.warmup_blocks, plan.sample_blocks, plan.order_seed)
        .map_err(|error| format!("invalid schedule: {error:?}"))?;
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_millis();
    let mut report = Report {
        schema: REPORT_SCHEMA,
        started_unix_ms,
        completed: false,
        error: None,
        plan,
        streams: STREAMS,
        payload_sweep: PAYLOAD_SWEEP,
        shmem_geometry: geometry,
        thresholds: Thresholds::default(),
        schedule: run_schedule.clone(),
        runs: Vec::with_capacity(run_schedule.len()),
        decision: None,
    };

    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output_path)
        .map_err(|error| {
            format!(
                "refusing to overwrite output {}: {error}",
                output_path.display()
            )
        })?;
    persist(&mut output, &report)?;

    for spec in &run_schedule {
        match run_one(&report, *spec) {
            Ok(record) => report.runs.push(record),
            Err(error) => {
                report.error = Some(error.clone());
                persist(&mut output, &report)?;
                return Err(error);
            }
        }
        // The raw file is a durable checkpoint after every sequential child.
        persist(&mut output, &report)?;
    }

    let decision = evaluate(&report.schedule, &report.runs)
        .map_err(|error| format!("evaluation refused: {error:?}"))?;
    report.decision = Some(decision);
    report.completed = true;
    persist(&mut output, &report)?;
    Ok(())
}

fn run_one(report: &Report, spec: RunSpec) -> Result<RunRecord, String> {
    let variant_command = match spec.variant {
        Variant::Serial => &report.plan.serial,
        Variant::Ivshmem => &report.plan.ivshmem,
    };
    let qemu_args_json = serde_json::to_string(&variant_command.qemu_args)
        .map_err(|error| format!("cannot encode QEMU arguments: {error}"))?;
    let payloads = PAYLOAD_SWEEP
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let mut command = Command::new(&variant_command.program);
    command
        .args(&variant_command.args)
        .env("KORE_HOSTLINK_AB_VARIANT", spec.variant.as_str())
        .env("KORE_HOSTLINK_AB_PHASE", spec.phase.as_str())
        .env("KORE_HOSTLINK_AB_BLOCK", spec.block.to_string())
        .env("KORE_HOSTLINK_AB_POSITION", spec.position.to_string())
        .env("KORE_HOSTLINK_AB_STREAMS", STREAMS.to_string())
        .env(
            "KORE_HOSTLINK_AB_TIMEOUT_SECONDS",
            report.plan.timeout_seconds.to_string(),
        )
        .env("KORE_HOSTLINK_AB_PAYLOADS", payloads)
        .env(
            "KORE_HOSTLINK_AB_SHMEM_ABI",
            report.shmem_geometry.abi.to_string(),
        )
        .env(
            "KORE_HOSTLINK_AB_SHMEM_CAPACITY",
            report.shmem_geometry.capacity.to_string(),
        )
        .env(
            "KORE_HOSTLINK_AB_SHMEM_WINDOW_BYTES",
            report.shmem_geometry.window_bytes.to_string(),
        )
        .env("KORE_HOSTLINK_AB_QEMU_ARGS_JSON", qemu_args_json)
        .env(
            "KORE_HOSTLINK_AB_KORE_SHA",
            &report.plan.provenance.kore_sha,
        )
        .env(
            "KORE_HOSTLINK_AB_HOST_SHA",
            &report.plan.provenance.host_sha,
        )
        .env(
            "KORE_HOSTLINK_AB_QEMU_SHA",
            &report.plan.provenance.qemu_sha,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = &variant_command.working_directory {
        command.current_dir(directory);
    }

    let started = Instant::now();
    let mut child = command.spawn().map_err(|error| {
        format!(
            "cannot start {} adapter {:?}: {error}",
            spec.variant.as_str(),
            variant_command.program
        )
    })?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "child stdout pipe was not installed".to_owned())?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| "child stderr pipe was not installed".to_owned())?;
    // These readers drain only the one active adapter. They bound retained
    // bytes and prevent a full pipe from deadlocking the sequential child.
    let stdout_reader = thread::spawn(move || read_capped(child_stdout, MAX_OBSERVATION_BYTES));
    let stderr_reader = thread::spawn(move || read_capped(child_stderr, MAX_STDERR_BYTES));
    let timeout = Duration::from_secs(report.plan.timeout_seconds);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("cannot poll child {}: {error}", child.id()));
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break Err(format!(
                "{} {:?} block {} position {} exceeded {} seconds",
                spec.phase.as_str(),
                spec.variant.as_str(),
                spec.block,
                spec.position,
                report.plan.timeout_seconds
            ));
        }
        thread::sleep(POLL_INTERVAL);
    };
    let wall_time_ms = started.elapsed().as_millis();
    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;
    let status = status?;
    if stdout.overflowed {
        return Err(format!(
            "{} adapter stdout exceeded {} bytes",
            spec.variant.as_str(),
            MAX_OBSERVATION_BYTES
        ));
    }
    if stderr.overflowed {
        return Err(format!(
            "{} adapter stderr exceeded {} bytes",
            spec.variant.as_str(),
            MAX_STDERR_BYTES
        ));
    }
    let stderr = String::from_utf8(stderr.bytes)
        .map_err(|_| format!("{} adapter stderr is not UTF-8", spec.variant.as_str()))?;
    if !status.success() {
        return Err(format!(
            "{} adapter exited with {status}: {}",
            spec.variant.as_str(),
            stderr.trim()
        ));
    }
    let observation: Observation = serde_json::from_slice(&stdout.bytes).map_err(|error| {
        format!(
            "{} adapter emitted invalid observation JSON: {error}",
            spec.variant.as_str()
        )
    })?;
    validate_observation(&observation, spec.variant)
        .map_err(|error| format!("invalid {} observation: {error:?}", spec.variant.as_str()))?;
    Ok(RunRecord {
        spec,
        wall_time_ms,
        stderr,
        observation,
    })
}

fn evaluate_report(path: &Path) -> Result<(), String> {
    let bytes = read_bounded(path, MAX_REPORT_BYTES)?;
    let report: Report =
        serde_json::from_slice(&bytes).map_err(|error| format!("invalid report JSON: {error}"))?;
    if report.schema != REPORT_SCHEMA {
        return Err(format!("unsupported report schema {}", report.schema));
    }
    report
        .plan
        .validate()
        .map_err(|error| format!("invalid recorded plan: {error:?}"))?;
    let expected_schedule = schedule(
        report.plan.warmup_blocks,
        report.plan.sample_blocks,
        report.plan.order_seed,
    )
    .map_err(|error| format!("invalid recorded schedule: {error:?}"))?;
    if expected_schedule != report.schedule {
        return Err("recorded order does not match the recorded randomization seed".to_owned());
    }
    let decision = evaluate(&report.schedule, &report.runs)
        .map_err(|error| format!("evaluation refused: {error:?}"))?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &decision)
        .map_err(|error| format!("cannot write decision: {error}"))?;
    println!();
    Ok(())
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.len() > maximum {
        return Err(format!(
            "{} is {} bytes; maximum is {maximum}",
            path.display(),
            metadata.len()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(bytes)
}

fn persist(output: &mut File, report: &Report) -> Result<(), String> {
    output
        .seek(SeekFrom::Start(0))
        .and_then(|_| output.set_len(0))
        .map_err(|error| format!("cannot reset raw report: {error}"))?;
    serde_json::to_writer_pretty(&mut *output, report)
        .map_err(|error| format!("cannot encode raw report: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|_| output.sync_all())
        .map_err(|error| format!("cannot persist raw report: {error}"))
}

struct CappedOutput {
    bytes: Vec<u8>,
    overflowed: bool,
}

fn read_capped(mut reader: impl Read, maximum: u64) -> std::io::Result<CappedOutput> {
    let maximum = maximum as usize;
    let mut bytes = Vec::with_capacity(maximum.min(64 << 10));
    let mut overflowed = false;
    let mut chunk = [0u8; 8 << 10];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let remaining = maximum.saturating_sub(bytes.len());
        let retain = remaining.min(read);
        bytes.extend_from_slice(&chunk[..retain]);
        overflowed |= retain != read;
    }
    Ok(CappedOutput { bytes, overflowed })
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<CappedOutput>>,
    name: &str,
) -> Result<CappedOutput, String> {
    reader
        .join()
        .map_err(|_| format!("{name} reader panicked"))?
        .map_err(|error| format!("cannot read child {name}: {error}"))
}
