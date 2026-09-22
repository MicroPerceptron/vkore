# Optional supervisor native acceptance

Status: gate implemented; native execution still pending. Host unit tests do
not certify this gate. Use the coordinated plan in Kore
[`res/agent-supervisor/implementation-plan.md`](https://github.com/MicroPerceptron/kore/issues/1053).

## Repeated native restart gate

`kore_runtime::native_supervisor_gate::three_supervisor_restarts_preserve_the_native_vm`
starts a real configured Kore VM and llama.cpp server, waits for authenticated
versioned supervisor readiness, then requests three supervisor retire/relaunch
cycles. It asserts exact successor generations, the same live QEMU PID, one
VM boot, and an unparked interactive desktop. It never substitutes a simulated
guest or deterministic inference backend. Its final teardown stops its own VM.

Run only with exclusive QEMU use and a dedicated vKore state directory. Back
up fixtures first. The application settings next to `project_root` must select
the candidate enabled ISO, real model/server and bounded runtime resources.
Use the launcher's current firmware/device arguments, with writable disks,
firmware variable stores and serial logs inside the gate's own directory.
Do not point it at a running launcher's disk or the user's manual state.

Create a JSON configuration with these fields:

- `project_root`: the dedicated state's projects directory.
- `qemu_executable`: the actual QEMU executable path.
- `qemu_arguments`: the launcher's argument array, excluding `-cdrom`, QMP,
  and HostLink arguments (the runtime adds those); include a retained serial log.
- `iso_blake3`: exact BLAKE3 digest of the configured candidate ISO. Record its
  SHA-256 and all source revisions in the accompanying evidence manifest too.

Then run from the vKore checkout, preserving stdout/stderr and the serial log:

```sh
VKORE_SUPERVISOR_GATE_CONFIG=/absolute/path/to/gate.json \
CARGO_INCREMENTAL=0 cargo test -p vkored --lib \
  kore_runtime::native_supervisor_gate::three_supervisor_restarts_preserve_the_native_vm \
  -- --ignored --exact --nocapture
```

During development, pass the complete native-owner Cargo override before
`test`. Release acceptance must use the committed compatible dependency pins.
Missing configuration, mismatched ISO bytes, timeout, stale readiness or a VM
replacement fails the gate; there is no skip-success path.

This idle lifecycle gate does not establish desktop input responsiveness,
in-flight tool cancellation, unknown-write handling, per-resource reclamation
accounting or process isolation. Those remain distinct native/UI acceptance
scenarios in the coordinated plan. Save failed runs as well as successful ones.

## Payload configuration

Committed Kore payloads belong to the ordinary disabled configuration. Building
the enabled candidate must rebuild the user/service payloads, stamp their
digests and then build the kernel/ISO with the same selectors. vKore's
`make artifacts` therefore uses `prepare-boot-gate-x86`, rather than invoking
only the final ISO recipe. The enabled build may leave generated payload and
digest changes in the Kore checkout; the candidate lock must record their
exact diff hash as well as the ISO hash. Do not commit enabled service payloads
as the ordinary disabled baseline or treat the old baseline lock as compatible.
