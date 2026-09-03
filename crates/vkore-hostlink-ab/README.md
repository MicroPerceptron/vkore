# HostLink serial versus ivshmem A/B runner

This host-only tool schedules the two complete implementations sequentially in
seeded, counterbalanced ABBA or BAAB blocks. It never embeds a HostLink frame or
defines a second wire schema. The checked-in `hostlink-measure-adapter` owns the
host endpoint, launches QEMU, resolves per-run socket/backing paths, and emits
one observation JSON object on stdout. The plan records the seed and the raw
report records the fully expanded order. At least 15 sample blocks provide 30
observations per variant.

The run command is Linux-only because the supported QEMU build on macOS has no
ivshmem device. It refuses before running even the serial warmup when invoked on
another OS. Existing raw reports can be evaluated on any host.

```sh
cargo run -p vkore-hostlink-ab -- run crates/vkore-hostlink-ab/plan.example.json raw.json
cargo run -p vkore-hostlink-ab -- evaluate raw.json
```

The plan records kore, host-adapter, and QEMU Git SHAs. Every raw observation
also records BLAKE3 hashes of the actual adapter, image, and QEMU binaries plus
the exact resolved QEMU argv. The adapter and QEMU are invoked directly, never
through a shell. Per-run context is supplied through `KORE_HOSTLINK_AB_*`.

The adapter can also run one standalone gate:

```sh
target/release/hostlink-measure-adapter \
  --transport serial \
  --qemu /usr/bin/qemu-system-x86_64 \
  --image build/kore-x86_64.iso \
  --qemu-arg-file /path/to/qemu-args.json
```

`--qemu-arg-file` is a JSON string array and takes precedence over
`KORE_HOSTLINK_AB_QEMU_ARGS_JSON`. It must contain
`@HOSTLINK_CONTROL_SOCKET@`, `@HOSTLINK_BULK_SOCKET@`, and `@KORE_IMAGE@`;
ivshmem additionally requires `@HOSTLINK_SHMEM_PATH@` and
`@HOSTLINK_SHMEM_SIZE@`; the latter is replaced with the fixed 32 MiB backing
size. Each invocation creates a private
0700 runtime directory, create-new 0600 shared backing, and listeners before
launching QEMU. Existing backing files are never truncated or reused.

Guest/debugcon stdout must provide newline-delimited measurement testimony:

```text
HOSTLINK_AB_PAYLOAD_BEGIN 65536
HOSTLINK_AB_PAYLOAD_END {"payload_bytes":65536,"transferred_bytes":1048576,"latency_p50_us":10.0,"latency_p99_us":12.0,"throughput_mib_per_second":100.0,"guest_elapsed_ns_per_byte":2.0}
HOSTLINK_AB_FAULT disconnect
HOSTLINK_AB_FAULT resumed
HOSTLINK_AB_RESULT {"schema":2,"variant":"serial","boot_id":"<64 lowercase hex>",...}
```

There is exactly one begin/end pair for each fixed payload, in order. The final
guest workload record must witness at least 16 interleaved streams, control and
cancel latency under saturated bulk, and typed local backpressure. Overall
saturation passes only when the host also counted a backpressure refusal.
Partial I/O is host-derived
from the adapter's deliberately bounded 64-byte control writes. Disconnect is
not trusted from that record: the first fault marker makes the adapter sever
the real link. After the guest observes deterministic disconnect status and
reopens all routes, `resumed` is accepted only if the host negotiated a distinct
LinkEpoch. Each invocation launches a fresh QEMU, and report evaluation requires
unique 256-bit guest boot identities across all sample runs as restart evidence.
Missing elapsed telemetry or any witness fails closed. Adapter stdout is
reserved for the final observation JSON; accepted gate markers and QEMU
diagnostics are written to stderr.

Before each BEGIN, Broker and bootstrap agent slots 0 and 1 each submit an ordinary inference
request whose payload is exactly
`b"kore.hostlink.measure-arm.v1\0" + variant:u8 + phase:u8` (`serial=0`,
`ivshmem=1`, phases `0..4` in sweep order). The host backend withholds each
ordinary correlated reply until BEGIN has been read and counters sampled; the
guest starts measured traffic only after its arm RequestId completes. The host
binds route bits from exact `(RouteId, TaskGeneration, role)` sequence identity
at route-open time and rejects END unless it
observed non-arm traffic on Broker and agent slots 0 and 1 during that phase.
After its measured bulk completes, Broker additionally submits
`b"kore.hostlink.measure-fence.v1\0" + variant:u8 + phase:u8`. This request is
excluded from the Broker traffic bit and its correlated reply is withheld until
all three host-bound completion bits are set. Broker must await that reply
before emitting END.

One invocation emits a bounded schema-2 shape containing:

```json
{
  "schema": 2,
  "variant": "serial",
  "control_p50_us": 8.0,
  "control_p99_us": 10.0,
  "cancel_p50_us": 8.0,
  "cancel_p99_us": 10.0,
  "payloads": [
    {
      "payload_bytes": 64,
      "latency_p50_us": 10.0,
      "latency_p99_us": 12.0,
      "throughput_mib_per_second": 100.0,
      "cpu_ns_per_byte": 4.0,
      "host_cpu_ns_per_byte": 2.0,
      "guest_elapsed_ns_per_byte": 2.0,
      "qemu_cpu_ns_per_byte": 3.0,
      "adapter_copy_operations": null,
      "host_io_calls": 12
    }
  ],
  "acceptance": {
    "streams": 16,
    "interleaved": true,
    "control_under_bulk": true,
    "cancel_under_bulk": true,
    "saturation": true,
    "partial_io": true,
    "disconnect": true,
    "reconnect": true
  },
  "host_transport": { "...": "bounded host counters" },
  "provenance": { "...": "binary hashes and resolved QEMU argv" }
}
```

`payloads` must contain exactly `64, 256, 4096, 65536, 1048576` in order. The
example above abbreviates arrays/objects only for documentation. Serial
chardev-internal copies are not observable, so `adapter_copy_operations` is
`null` for serial rather than a fabricated count. Socket I/O calls are kept as
a separate wake/syscall proxy. ivshmem reports only explicit adapter slot
copies and serial wake frames. Host daemon CPU and QEMU process CPU are recorded
separately; `cpu_ns_per_byte` is their sum for the retention decision. Guest
elapsed/cycle-derived work is retained independently and is never presented as
process CPU.

The predeclared decision uses sample medians, excluding warmups. Shared memory
is retained only when every bulk point (64 KiB and 1 MiB) has either at least
2x throughput or at least 25% lower CPU/byte, and both control p99 and
cancellation p99 regress by no more than 10%. Raw warmups and samples remain in
the checkpointed report; no aggregate can erase them.
