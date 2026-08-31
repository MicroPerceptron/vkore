//! Reproducible evaluation policy and sequential schedule for HostLink #538.
//!
//! This is experiment metadata, not a HostLink wire schema.  External serial
//! and ivshmem adapters exercise the same HostLink frames and emit one bounded
//! [`Observation`] per process invocation.

use kore_hostlink::{SHMEM_QUEUE_CAPACITY, SHMEM_SLOT_PAYLOAD_BYTES};
use kore_hostlink_shmem::{ABI as SHMEM_ABI, Layout as ShmemLayout};
use serde::{Deserialize, Serialize};

pub const REPORT_SCHEMA: u32 = 2;
pub const OBSERVATION_SCHEMA: u32 = 2;
pub const STREAMS: u16 = 16;
pub const PAYLOAD_SWEEP: [u32; 5] = [64, 256, 4_096, 65_536, 1_048_576];
pub const BULK_START: usize = 3;
pub const MIN_THROUGHPUT_RATIO: f64 = 2.0;
pub const MIN_CPU_REDUCTION: f64 = 0.25;
pub const MAX_CONTROL_REGRESSION: f64 = 0.10;
pub const MAX_CANCEL_REGRESSION: f64 = 0.10;

pub const MAX_WARMUP_BLOCKS: u8 = 8;
pub const MIN_SAMPLE_BLOCKS: u8 = 15;
pub const MAX_SAMPLE_BLOCKS: u8 = 32;
pub const MIN_SAMPLE_OBSERVATIONS_PER_VARIANT: usize = MIN_SAMPLE_BLOCKS as usize * 2;
pub const MAX_RUNS: usize = (MAX_WARMUP_BLOCKS as usize + MAX_SAMPLE_BLOCKS as usize) * 4;
pub const MAX_ARGS: usize = 128;
pub const MAX_ARG_BYTES: usize = 4_096;
pub const MAX_PROGRAM_BYTES: usize = 4_096;
pub const MAX_SHA_BYTES: usize = 64;
pub const MAX_TIMEOUT_SECONDS: u64 = 3_600;
pub const MAX_OBSERVATION_BYTES: u64 = 1 << 20;
pub const MAX_STDERR_BYTES: u64 = 4 << 10;
pub const MAX_SHMEM_WINDOW_BYTES: usize = 256 << 20;
pub const MEASUREMENT_ARM_PREFIX: &[u8] = b"kore.hostlink.measure-arm.v1\0";
pub const MEASUREMENT_FENCE_PREFIX: &[u8] = b"kore.hostlink.measure-fence.v1\0";
pub const MEASUREMENT_ENABLED_PREFIX: &[u8] = b"kore.hostlink.measure-enabled.v1\0";
pub const MEASUREMENT_VARIANT_SERIAL: u8 = 0;
pub const MEASUREMENT_VARIANT_IVSHMEM: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    Serial,
    Ivshmem,
}

impl Variant {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Serial => "serial",
            Self::Ivshmem => "ivshmem",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Warmup,
    Sample,
}

impl Phase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Sample => "sample",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub kore_sha: String,
    pub host_sha: String,
    pub qemu_sha: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VariantCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub qemu_args: Vec<String>,
    pub working_directory: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema: u32,
    pub provenance: Provenance,
    pub warmup_blocks: u8,
    pub sample_blocks: u8,
    pub order_seed: u64,
    pub timeout_seconds: u64,
    pub shmem_capacity: u32,
    pub serial: VariantCommand,
    pub ivshmem: VariantCommand,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShmemGeometry {
    pub abi: u32,
    pub capacity: u32,
    pub maximum_payload_bytes: u32,
    pub window_bytes: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunSpec {
    pub ordinal: usize,
    pub phase: Phase,
    pub block: u8,
    pub position: u8,
    pub variant: Variant,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadMetrics {
    pub payload_bytes: u32,
    pub latency_p50_us: f64,
    pub latency_p99_us: f64,
    pub throughput_mib_per_second: f64,
    pub cpu_ns_per_byte: f64,
    pub host_cpu_ns_per_byte: f64,
    pub guest_elapsed_ns_per_byte: f64,
    pub qemu_cpu_ns_per_byte: f64,
    /// Explicit adapter copies only. `None` means the transport path does not
    /// expose a trustworthy count; it is never inferred from socket syscalls.
    pub adapter_copy_operations: Option<u64>,
    /// Successful host transport I/O calls, a wake/syscall proxy rather than
    /// a claim about hidden QEMU/chardev copies.
    pub host_io_calls: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementProvenance {
    pub adapter_blake3: String,
    pub image_blake3: String,
    pub qemu_blake3: String,
    pub qemu_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceWitness {
    pub streams: u16,
    pub interleaved: bool,
    pub control_under_bulk: bool,
    pub cancel_under_bulk: bool,
    pub saturation: bool,
    pub partial_io: bool,
    pub disconnect: bool,
    pub reconnect: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestWorkloadWitness {
    pub streams: u16,
    pub interleaved: bool,
    pub control_under_bulk: bool,
    pub cancel_under_bulk: bool,
    /// Guest-observed typed local mailbox/queue backpressure.
    pub local_backpressure: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostTransportMetrics {
    pub frames_in: u64,
    pub frames_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub backpressure: u64,
    pub disconnects: u64,
    pub shmem_publications: u64,
    pub shmem_records_received: u64,
    pub shmem_copies: u64,
    pub shmem_wake_frames: u64,
    pub socket_read_calls: u64,
    pub socket_write_calls: u64,
    pub partial_write_calls: u64,
}

/// Mandatory in-guest portion of one measurement. The adapter supplies host
/// CPU, QEMU CPU, transport counters, binary hashes, and resolved arguments.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestMeasurement {
    pub schema: u32,
    pub variant: Variant,
    pub control_p50_us: f64,
    pub control_p99_us: f64,
    pub cancel_p50_us: f64,
    pub cancel_p99_us: f64,
    pub workload: GuestWorkloadWitness,
    /// Fresh 256-bit identity created inside this boot, lowercase hex.
    pub boot_id: String,
    pub payloads: Vec<GuestPayloadMetrics>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuestPayloadMetrics {
    pub payload_bytes: u32,
    pub transferred_bytes: u64,
    pub latency_p50_us: f64,
    pub latency_p99_us: f64,
    pub throughput_mib_per_second: f64,
    /// Guest-observed elapsed work/cycle-derived time, not process CPU.
    pub guest_elapsed_ns_per_byte: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub schema: u32,
    pub variant: Variant,
    pub control_p50_us: f64,
    pub control_p99_us: f64,
    pub cancel_p50_us: f64,
    pub cancel_p99_us: f64,
    pub payloads: Vec<PayloadMetrics>,
    pub acceptance: AcceptanceWitness,
    pub host_transport: HostTransportMetrics,
    pub provenance: MeasurementProvenance,
    pub guest_boot_id: String,
    /// Host-derived negotiated epochs, including the post-fault epoch.
    pub link_epochs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunRecord {
    pub spec: RunSpec,
    pub wall_time_ms: u128,
    pub stderr: String,
    pub observation: Observation,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Thresholds {
    pub minimum_bulk_throughput_ratio: f64,
    pub minimum_cpu_per_byte_reduction: f64,
    pub maximum_control_p99_regression: f64,
    pub maximum_cancel_p99_regression: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            minimum_bulk_throughput_ratio: MIN_THROUGHPUT_RATIO,
            minimum_cpu_per_byte_reduction: MIN_CPU_REDUCTION,
            maximum_control_p99_regression: MAX_CONTROL_REGRESSION,
            maximum_cancel_p99_regression: MAX_CANCEL_REGRESSION,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Decision {
    pub retain_shmem: bool,
    pub throughput_gate_passed: bool,
    pub cpu_gate_passed: bool,
    pub control_gate_passed: bool,
    pub cancel_gate_passed: bool,
    pub fresh_boot_gate_passed: bool,
    pub worst_bulk_throughput_ratio: f64,
    pub worst_bulk_cpu_reduction: f64,
    pub control_p99_regression: f64,
    pub cancel_p99_regression: f64,
    pub sample_observations_per_variant: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub schema: u32,
    pub started_unix_ms: u128,
    pub completed: bool,
    pub error: Option<String>,
    pub plan: Plan,
    pub streams: u16,
    pub payload_sweep: [u32; PAYLOAD_SWEEP.len()],
    pub shmem_geometry: ShmemGeometry,
    pub thresholds: Thresholds,
    pub schedule: Vec<RunSpec>,
    pub runs: Vec<RunRecord>,
    pub decision: Option<Decision>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    WrongSchema,
    BadWarmups,
    BadSamples,
    BadOrderSeed,
    BadSchedule,
    BadTimeout,
    BadSha(&'static str),
    BadCommand(Variant),
    BadArgs(Variant),
    BadWorkingDirectory(Variant),
    BadShmemGeometry,
    WrongObservationVariant,
    WrongPayloadSweep,
    InvalidMetric,
    MissingSamples,
}

impl Plan {
    pub fn validate(&self) -> Result<ShmemGeometry, ValidationError> {
        if self.schema != REPORT_SCHEMA {
            return Err(ValidationError::WrongSchema);
        }
        if self.warmup_blocks > MAX_WARMUP_BLOCKS {
            return Err(ValidationError::BadWarmups);
        }
        if !(MIN_SAMPLE_BLOCKS..=MAX_SAMPLE_BLOCKS).contains(&self.sample_blocks) {
            return Err(ValidationError::BadSamples);
        }
        if self.order_seed == 0 {
            return Err(ValidationError::BadOrderSeed);
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > MAX_TIMEOUT_SECONDS {
            return Err(ValidationError::BadTimeout);
        }
        validate_sha(&self.provenance.kore_sha).map_err(|_| ValidationError::BadSha("kore_sha"))?;
        validate_sha(&self.provenance.host_sha).map_err(|_| ValidationError::BadSha("host_sha"))?;
        validate_sha(&self.provenance.qemu_sha).map_err(|_| ValidationError::BadSha("qemu_sha"))?;
        validate_command(&self.serial, Variant::Serial)?;
        validate_command(&self.ivshmem, Variant::Ivshmem)?;
        if self.shmem_capacity != SHMEM_QUEUE_CAPACITY {
            return Err(ValidationError::BadShmemGeometry);
        }
        let layout = ShmemLayout::new(self.shmem_capacity, SHMEM_SLOT_PAYLOAD_BYTES)
            .map_err(|_| ValidationError::BadShmemGeometry)?;
        if layout.window_bytes() > MAX_SHMEM_WINDOW_BYTES {
            return Err(ValidationError::BadShmemGeometry);
        }
        Ok(ShmemGeometry {
            abi: SHMEM_ABI,
            capacity: layout.capacity(),
            maximum_payload_bytes: layout.payload_bytes(),
            window_bytes: layout.window_bytes(),
        })
    }
}

fn validate_sha(sha: &str) -> Result<(), ()> {
    if (7..=MAX_SHA_BYTES).contains(&sha.len()) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_command(command: &VariantCommand, variant: Variant) -> Result<(), ValidationError> {
    if command.program.is_empty()
        || command.program.len() > MAX_PROGRAM_BYTES
        || command.program.contains('\0')
    {
        return Err(ValidationError::BadCommand(variant));
    }
    if command.args.len() > MAX_ARGS
        || command.qemu_args.len() > MAX_ARGS
        || command
            .args
            .iter()
            .chain(command.qemu_args.iter())
            .any(|arg| arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
    {
        return Err(ValidationError::BadArgs(variant));
    }
    if command.working_directory.as_ref().is_some_and(|directory| {
        directory.is_empty() || directory.len() > MAX_PROGRAM_BYTES || directory.contains('\0')
    }) {
        return Err(ValidationError::BadWorkingDirectory(variant));
    }
    Ok(())
}

/// Produce deterministically randomized ABBA/BAAB blocks.
///
/// A=serial and B=ivshmem. Phases never mix, variants never run concurrently,
/// and the seed plus expanded returned order make the randomization replayable.
pub fn schedule(
    warmup_blocks: u8,
    sample_blocks: u8,
    order_seed: u64,
) -> Result<Vec<RunSpec>, ValidationError> {
    if warmup_blocks > MAX_WARMUP_BLOCKS {
        return Err(ValidationError::BadWarmups);
    }
    if !(MIN_SAMPLE_BLOCKS..=MAX_SAMPLE_BLOCKS).contains(&sample_blocks) {
        return Err(ValidationError::BadSamples);
    }
    if order_seed == 0 {
        return Err(ValidationError::BadOrderSeed);
    }
    let mut runs = Vec::with_capacity((warmup_blocks as usize + sample_blocks as usize) * 4);
    let mut random = order_seed;
    append_blocks(&mut runs, Phase::Warmup, warmup_blocks, &mut random);
    append_blocks(&mut runs, Phase::Sample, sample_blocks, &mut random);
    debug_assert!(runs.len() <= MAX_RUNS);
    Ok(runs)
}

fn append_blocks(output: &mut Vec<RunSpec>, phase: Phase, blocks: u8, random: &mut u64) {
    const ABBA: [Variant; 4] = [
        Variant::Serial,
        Variant::Ivshmem,
        Variant::Ivshmem,
        Variant::Serial,
    ];
    const BAAB: [Variant; 4] = [
        Variant::Ivshmem,
        Variant::Serial,
        Variant::Serial,
        Variant::Ivshmem,
    ];
    for block in 0..blocks {
        let order = if splitmix64(random) & 1 == 0 {
            ABBA
        } else {
            BAAB
        };
        for (position, variant) in order.into_iter().enumerate() {
            output.push(RunSpec {
                ordinal: output.len(),
                phase,
                block,
                position: position as u8,
                variant,
            });
        }
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// Validate an expanded counterbalanced schedule independently of its plan.
pub fn validate_schedule(runs: &[RunSpec]) -> Result<(), ValidationError> {
    if runs.is_empty() || runs.len() > MAX_RUNS || !runs.len().is_multiple_of(4) {
        return Err(ValidationError::BadSchedule);
    }
    let abba = [
        Variant::Serial,
        Variant::Ivshmem,
        Variant::Ivshmem,
        Variant::Serial,
    ];
    let baab = [
        Variant::Ivshmem,
        Variant::Serial,
        Variant::Serial,
        Variant::Ivshmem,
    ];
    let mut warmup_blocks = 0u8;
    let mut sample_blocks = 0u8;
    let mut sample_started = false;
    for (block_index, block) in runs.as_chunks::<4>().0.iter().enumerate() {
        let phase = block[0].phase;
        if block.iter().any(|run| run.phase != phase)
            || block.iter().any(|run| run.block != block[0].block)
            || block[0].ordinal != block_index * 4
            || block
                .iter()
                .enumerate()
                .any(|(position, run)| run.position as usize != position)
            || block
                .iter()
                .enumerate()
                .any(|(position, run)| run.ordinal != block[0].ordinal + position)
        {
            return Err(ValidationError::BadSchedule);
        }
        let order = [
            block[0].variant,
            block[1].variant,
            block[2].variant,
            block[3].variant,
        ];
        if order != abba && order != baab {
            return Err(ValidationError::BadSchedule);
        }
        match phase {
            Phase::Warmup if !sample_started && block[0].block == warmup_blocks => {
                warmup_blocks += 1;
            }
            Phase::Sample if block[0].block == sample_blocks => {
                sample_started = true;
                sample_blocks += 1;
            }
            _ => return Err(ValidationError::BadSchedule),
        }
    }
    if warmup_blocks > MAX_WARMUP_BLOCKS
        || !(MIN_SAMPLE_BLOCKS..=MAX_SAMPLE_BLOCKS).contains(&sample_blocks)
    {
        return Err(ValidationError::BadSchedule);
    }
    Ok(())
}

pub fn validate_observation(
    observation: &Observation,
    expected: Variant,
) -> Result<(), ValidationError> {
    if observation.schema != OBSERVATION_SCHEMA {
        return Err(ValidationError::WrongSchema);
    }
    if observation.variant != expected {
        return Err(ValidationError::WrongObservationVariant);
    }
    let witness = observation.acceptance;
    if witness.streams < STREAMS
        || !witness.interleaved
        || !witness.control_under_bulk
        || !witness.cancel_under_bulk
        || !witness.saturation
        || !witness.partial_io
        || !witness.disconnect
        || !witness.reconnect
    {
        return Err(ValidationError::InvalidMetric);
    }
    if !lower_hex_exact(&observation.guest_boot_id, 64)
        || observation.link_epochs.len() < 2
        || observation.link_epochs.len() > 8
        || observation
            .link_epochs
            .iter()
            .any(|epoch| !lower_hex_exact(epoch, 32))
        || observation
            .link_epochs
            .windows(2)
            .any(|pair| pair[0] == pair[1])
    {
        return Err(ValidationError::InvalidMetric);
    }
    if validate_sha(&observation.provenance.adapter_blake3).is_err()
        || validate_sha(&observation.provenance.image_blake3).is_err()
        || validate_sha(&observation.provenance.qemu_blake3).is_err()
        || observation.provenance.qemu_args.is_empty()
        || observation.provenance.qemu_args.len() > MAX_ARGS
        || observation
            .provenance
            .qemu_args
            .iter()
            .any(|arg| arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
    {
        return Err(ValidationError::InvalidMetric);
    }
    if observation.payloads.len() != PAYLOAD_SWEEP.len()
        || observation
            .payloads
            .iter()
            .map(|metrics| metrics.payload_bytes)
            .ne(PAYLOAD_SWEEP)
    {
        return Err(ValidationError::WrongPayloadSweep);
    }
    let control = [
        observation.control_p50_us,
        observation.control_p99_us,
        observation.cancel_p50_us,
        observation.cancel_p99_us,
    ];
    if control.into_iter().any(|value| !positive_finite(value)) {
        return Err(ValidationError::InvalidMetric);
    }
    for metrics in &observation.payloads {
        let positive = [
            metrics.latency_p50_us,
            metrics.latency_p99_us,
            metrics.throughput_mib_per_second,
            metrics.cpu_ns_per_byte,
            metrics.guest_elapsed_ns_per_byte,
        ];
        let nonnegative = [metrics.host_cpu_ns_per_byte, metrics.qemu_cpu_ns_per_byte];
        if positive.into_iter().any(|value| !positive_finite(value))
            || nonnegative
                .into_iter()
                .any(|value| !value.is_finite() || value < 0.0)
        {
            return Err(ValidationError::InvalidMetric);
        }
    }
    Ok(())
}

fn lower_hex_exact(value: &str, bytes: usize) -> bool {
    value.len() == bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

/// Apply the predeclared retain-shmem policy to sample observations only.
///
/// Each metric is reduced to the median within a variant.  The bulk gates use
/// the worst ratio across 64 KiB and 1 MiB, so one favorable size cannot hide
/// a regression at the other.
pub fn evaluate(schedule: &[RunSpec], runs: &[RunRecord]) -> Result<Decision, ValidationError> {
    validate_schedule(schedule)?;
    if schedule.len() != runs.len()
        || schedule
            .iter()
            .zip(runs)
            .any(|(expected, run)| expected != &run.spec)
    {
        return Err(ValidationError::MissingSamples);
    }
    for run in runs {
        validate_observation(&run.observation, run.spec.variant)?;
    }
    let serial: Vec<&Observation> = runs
        .iter()
        .filter(|run| run.spec.phase == Phase::Sample && run.spec.variant == Variant::Serial)
        .map(|run| &run.observation)
        .collect();
    let shmem: Vec<&Observation> = runs
        .iter()
        .filter(|run| run.spec.phase == Phase::Sample && run.spec.variant == Variant::Ivshmem)
        .map(|run| &run.observation)
        .collect();
    if serial.len() < MIN_SAMPLE_OBSERVATIONS_PER_VARIANT || serial.len() != shmem.len() {
        return Err(ValidationError::MissingSamples);
    }

    let mut worst_throughput = f64::INFINITY;
    let mut worst_cpu_reduction = f64::INFINITY;
    for payload_index in BULK_START..PAYLOAD_SWEEP.len() {
        let serial_throughput = median(
            serial
                .iter()
                .map(|observation| observation.payloads[payload_index].throughput_mib_per_second),
        );
        let shmem_throughput = median(
            shmem
                .iter()
                .map(|observation| observation.payloads[payload_index].throughput_mib_per_second),
        );
        worst_throughput = worst_throughput.min(shmem_throughput / serial_throughput);

        let serial_cpu = median(
            serial
                .iter()
                .map(|observation| observation.payloads[payload_index].cpu_ns_per_byte),
        );
        let shmem_cpu = median(
            shmem
                .iter()
                .map(|observation| observation.payloads[payload_index].cpu_ns_per_byte),
        );
        worst_cpu_reduction = worst_cpu_reduction.min(1.0 - shmem_cpu / serial_cpu);
    }

    let serial_control = median(serial.iter().map(|observation| observation.control_p99_us));
    let shmem_control = median(shmem.iter().map(|observation| observation.control_p99_us));
    let control_regression = shmem_control / serial_control - 1.0;
    let serial_cancel = median(serial.iter().map(|observation| observation.cancel_p99_us));
    let shmem_cancel = median(shmem.iter().map(|observation| observation.cancel_p99_us));
    let cancel_regression = shmem_cancel / serial_cancel - 1.0;

    let throughput_gate = worst_throughput >= MIN_THROUGHPUT_RATIO;
    let cpu_gate = worst_cpu_reduction >= MIN_CPU_REDUCTION;
    // Compare the original quantities at the inclusive boundary; deriving a
    // regression first would make an exact 10% case vulnerable to binary
    // floating-point subtraction rounding just above 0.10.
    let control_gate = shmem_control <= serial_control * (1.0 + MAX_CONTROL_REGRESSION);
    let cancel_gate = shmem_cancel <= serial_cancel * (1.0 + MAX_CANCEL_REGRESSION);
    let mut boot_ids: Vec<&str> = runs
        .iter()
        .filter(|run| run.spec.phase == Phase::Sample)
        .map(|run| run.observation.guest_boot_id.as_str())
        .collect();
    boot_ids.sort_unstable();
    let fresh_boot_gate = !boot_ids.windows(2).any(|pair| pair[0] == pair[1]);
    Ok(Decision {
        retain_shmem: (throughput_gate || cpu_gate)
            && control_gate
            && cancel_gate
            && fresh_boot_gate,
        throughput_gate_passed: throughput_gate,
        cpu_gate_passed: cpu_gate,
        control_gate_passed: control_gate,
        cancel_gate_passed: cancel_gate,
        fresh_boot_gate_passed: fresh_boot_gate,
        worst_bulk_throughput_ratio: worst_throughput,
        worst_bulk_cpu_reduction: worst_cpu_reduction,
        control_p99_regression: control_regression,
        cancel_p99_regression: cancel_regression,
        sample_observations_per_variant: serial.len(),
    })
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut values: Vec<f64> = values.collect();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// The A/B runner requires Linux because QEMU ivshmem is not available in the
/// supported macOS setup.  Evaluation of an existing report remains portable.
pub fn require_ivshmem_platform(is_linux: bool) -> Result<(), &'static str> {
    if is_linux {
        Ok(())
    } else {
        Err("ivshmem A/B runs require Linux; serial-only data is not a valid comparison")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> VariantCommand {
        VariantCommand {
            program: "adapter".into(),
            args: vec!["--measure".into()],
            qemu_args: vec!["-machine".into(), "q35".into()],
            working_directory: None,
        }
    }

    fn plan() -> Plan {
        Plan {
            schema: REPORT_SCHEMA,
            provenance: Provenance {
                kore_sha: "0123456789abcdef".into(),
                host_sha: "123456789abcdef0".into(),
                qemu_sha: "23456789abcdef01".into(),
            },
            warmup_blocks: 1,
            sample_blocks: MIN_SAMPLE_BLOCKS,
            order_seed: 0x0538_ABBA_5EED,
            timeout_seconds: 60,
            shmem_capacity: 8,
            serial: command(),
            ivshmem: command(),
        }
    }

    fn observation(variant: Variant, throughput_ratio: f64, cpu_ratio: f64) -> Observation {
        Observation {
            schema: OBSERVATION_SCHEMA,
            variant,
            control_p50_us: 8.0,
            control_p99_us: 10.0,
            cancel_p50_us: 8.0,
            cancel_p99_us: 10.0,
            payloads: PAYLOAD_SWEEP
                .into_iter()
                .map(|payload_bytes| PayloadMetrics {
                    payload_bytes,
                    latency_p50_us: 10.0,
                    latency_p99_us: 12.0,
                    throughput_mib_per_second: 100.0 * throughput_ratio,
                    cpu_ns_per_byte: 4.0 * cpu_ratio,
                    host_cpu_ns_per_byte: 2.0 * cpu_ratio,
                    guest_elapsed_ns_per_byte: 2.0 * cpu_ratio,
                    qemu_cpu_ns_per_byte: 3.0 * cpu_ratio,
                    adapter_copy_operations: Some(2),
                    host_io_calls: 2,
                })
                .collect(),
            acceptance: AcceptanceWitness {
                streams: STREAMS,
                interleaved: true,
                control_under_bulk: true,
                cancel_under_bulk: true,
                saturation: true,
                partial_io: true,
                disconnect: true,
                reconnect: true,
            },
            host_transport: HostTransportMetrics::default(),
            provenance: MeasurementProvenance {
                adapter_blake3: "a".repeat(64),
                image_blake3: "b".repeat(64),
                qemu_blake3: "c".repeat(64),
                qemu_args: vec!["-machine".into(), "q35".into()],
            },
            guest_boot_id: "d".repeat(64),
            link_epochs: vec!["1".repeat(32), "2".repeat(32)],
        }
    }

    fn records(
        specs: &[RunSpec],
        shmem_throughput: f64,
        shmem_cpu: f64,
        control: f64,
        cancel: f64,
    ) -> Vec<RunRecord> {
        specs
            .iter()
            .copied()
            .enumerate()
            .map(|(ordinal, spec)| {
                let (throughput, cpu) = match spec.variant {
                    Variant::Serial => (1.0, 1.0),
                    Variant::Ivshmem => (shmem_throughput, shmem_cpu),
                };
                let mut observation = observation(spec.variant, throughput, cpu);
                if spec.variant == Variant::Ivshmem {
                    observation.control_p99_us *= control;
                    observation.cancel_p99_us *= cancel;
                }
                observation.guest_boot_id = format!("{ordinal:064x}");
                RunRecord {
                    spec,
                    wall_time_ms: 1,
                    stderr: String::new(),
                    observation,
                }
            })
            .collect()
    }

    #[test]
    fn schedule_is_seeded_counterbalanced_and_separates_phases() {
        let seed = 0x0538_ABBA_5EED;
        let specs = schedule(1, MIN_SAMPLE_BLOCKS, seed).unwrap();
        assert_eq!(specs.len(), (1 + MIN_SAMPLE_BLOCKS as usize) * 4);
        for block in specs.as_chunks::<4>().0 {
            let order = [
                block[0].variant,
                block[1].variant,
                block[2].variant,
                block[3].variant,
            ];
            let abba = [
                Variant::Serial,
                Variant::Ivshmem,
                Variant::Ivshmem,
                Variant::Serial,
            ];
            let baab = [
                Variant::Ivshmem,
                Variant::Serial,
                Variant::Serial,
                Variant::Ivshmem,
            ];
            assert!(order == abba || order == baab);
            assert!(block.iter().all(|spec| spec.phase == block[0].phase));
        }
        assert!(specs[..4].iter().all(|spec| spec.phase == Phase::Warmup));
        assert!(specs[4..].iter().all(|spec| spec.phase == Phase::Sample));
        assert_eq!(validate_schedule(&specs), Ok(()));
        assert_eq!(
            validate_schedule(&specs[..4]),
            Err(ValidationError::BadSchedule)
        );
        assert_eq!(specs, schedule(1, MIN_SAMPLE_BLOCKS, seed).unwrap());
        assert_ne!(specs, schedule(1, MIN_SAMPLE_BLOCKS, seed + 1).unwrap());
        assert_eq!(
            schedule(0, MIN_SAMPLE_BLOCKS - 1, seed),
            Err(ValidationError::BadSamples)
        );
        assert_eq!(
            schedule(0, MIN_SAMPLE_BLOCKS, 0),
            Err(ValidationError::BadOrderSeed)
        );
    }

    #[test]
    fn plan_uses_the_reviewed_shared_memory_geometry() {
        let geometry = plan().validate().unwrap();
        assert_eq!(geometry.abi, SHMEM_ABI);
        assert_eq!(geometry.capacity, 8);
        assert_eq!(geometry.maximum_payload_bytes, SHMEM_SLOT_PAYLOAD_BYTES);
        assert!(geometry.window_bytes <= MAX_SHMEM_WINDOW_BYTES);
    }

    #[test]
    fn throughput_or_cpu_can_retain_shmem() {
        let specs = schedule(0, MIN_SAMPLE_BLOCKS, 1).unwrap();
        let throughput = evaluate(&specs, &records(&specs, 2.0, 1.0, 1.10, 1.10)).unwrap();
        assert!(throughput.retain_shmem);
        assert!(throughput.throughput_gate_passed);
        assert!(!throughput.cpu_gate_passed);
        assert_eq!(throughput.sample_observations_per_variant, 30);

        let cpu = evaluate(&specs, &records(&specs, 1.0, 0.75, 1.0, 1.0)).unwrap();
        assert!(cpu.retain_shmem);
        assert!(!cpu.throughput_gate_passed);
        assert!(cpu.cpu_gate_passed);
    }

    #[test]
    fn control_or_cancel_regression_vetoes_bulk_gain() {
        let specs = schedule(0, MIN_SAMPLE_BLOCKS, 1).unwrap();
        let control = evaluate(&specs, &records(&specs, 3.0, 0.5, 1.101, 1.0)).unwrap();
        assert!(!control.retain_shmem);
        assert!(!control.control_gate_passed);
        let cancel = evaluate(&specs, &records(&specs, 3.0, 0.5, 1.0, 1.101)).unwrap();
        assert!(!cancel.retain_shmem);
        assert!(!cancel.cancel_gate_passed);
    }

    #[test]
    fn repeated_boot_identity_vetoes_restart_evidence() {
        let specs = schedule(0, MIN_SAMPLE_BLOCKS, 1).unwrap();
        let mut runs = records(&specs, 3.0, 0.5, 1.0, 1.0);
        runs[1].observation.guest_boot_id = runs[0].observation.guest_boot_id.clone();
        let decision = evaluate(&specs, &runs).unwrap();
        assert!(!decision.fresh_boot_gate_passed);
        assert!(!decision.retain_shmem);
    }

    #[test]
    fn every_bulk_size_must_meet_the_selected_gain() {
        let specs = schedule(0, MIN_SAMPLE_BLOCKS, 1).unwrap();
        let mut runs = records(&specs, 2.0, 1.0, 1.0, 1.0);
        for run in &mut runs {
            if run.spec.variant == Variant::Ivshmem {
                run.observation.payloads[BULK_START].throughput_mib_per_second = 199.0;
            }
        }
        let decision = evaluate(&specs, &runs).unwrap();
        assert!(!decision.retain_shmem);
        assert!(!decision.throughput_gate_passed);
    }

    #[test]
    fn malformed_sweep_and_non_linux_execution_refuse() {
        let mut bad = observation(Variant::Serial, 1.0, 1.0);
        bad.payloads.swap(0, 1);
        assert_eq!(
            validate_observation(&bad, Variant::Serial),
            Err(ValidationError::WrongPayloadSweep)
        );
        assert!(require_ivshmem_platform(false).is_err());
        assert!(require_ivshmem_platform(true).is_ok());
    }
}
