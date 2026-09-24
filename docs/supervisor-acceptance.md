# Optional supervisor native acceptance

Candidate identities and observed checks are recorded in
[the candidate evidence](supervisor-candidate-validation.md).

Status: the current unaccepted candidate includes Kore's projection timeout
fix at `c6dd5fe714183c71475e5fa4372dfde9a7747c1d`. The enabled x86 image
reproduces byte-for-byte from the committed baseline, launcher preflight passes,
and `projection-enabled-files-01` passes the real-model create/read/missing-file/
arithmetic sequence with independent host-byte verification. Its boot installs
all 35 applications, 3 libraries and 2 data packages. SDK and host native pins
are unchanged. The exact source, image, clean payload diff and gate evidence are
in the candidate ledger linked above.

Previous containment, recovery, authority and desktop results remain tied to
their recorded images. They have not all been rerun on this image. Native Mac
computer-use checks now pass core prompts, Retry, resizing and project draft
isolation, but expose cancellation-confirmation and blocking folder-picker
failures; see [the UI report](native-ui-acceptance-20260924.md). UI acceptance
and hardware CI acceptance remain open. The default
artifact lock is not promoted. Readback still adds prose, and a correct missing-
file refusal still receives Failed status; neither is a projection regression.
Use the coordinated plan in Kore
[`res/agent-supervisor/implementation-plan.md`](https://github.com/MicroPerceptron/kore/issues/1053).

## Repeated native restart gate

`kore_runtime::native_supervisor_gate::native_frontend_prompt_before_recovery`
uses the same isolated configuration to check a real arithmetic prompt on a
fresh supervisor, including response receipt and confirmed worker retirement.
This separates startup/admission defects from failures caused by recovery.

`kore_runtime::native_supervisor_gate::three_supervisor_restarts_preserve_the_native_vm`
starts a real configured Kore VM and llama.cpp server, waits for authenticated
versioned supervisor readiness, then requests three supervisor retire/relaunch
cycles. It asserts exact successor generations and fresh broker routes, the same live QEMU PID, one
VM boot, and an unparked interactive desktop. It then severs HostLink while
idle, during an observed real inference request, and requests supervisor stop
after a real arithmetic response but before acknowledging its receipt. Each
boundary must actually be observed; timeout fails the gate. Recovery requires
the exact next generation, a fresh route and the same live VM, with no stale
response or automatic prompt replay. It never substitutes a simulated guest
or deterministic inference backend. Its final teardown stops its own VM.

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
Planning intents and authority-update text must fit the current native
`TASK_INTENT_MAX` of 128 UTF-8 bytes. Oversized test text fails before model
planning and cannot exercise the intended native operation.

This lifecycle gate does not establish desktop input responsiveness,
in-flight filesystem tool cancellation, unknown-write handling, per-resource reclamation
accounting or process isolation. Those remain distinct native/UI acceptance
scenarios in the coordinated plan. Save failed runs as well as successful ones.

## Desktop fault probe

`native_supervisor_faults_preserve_desktop` runs the same native restart/fault
sequence and requires an additional `desktop_probe` command array in the gate
JSON. The test appends a unique boundary name and pumps HostLink while the
probe runs. Any failed or timed-out probe fails the test; the original lifecycle
gate makes no desktop-input claim.

Use Kore's `tools/keytest_supervisor_desktop.py` with an additional dedicated
QMP socket in `qemu_arguments` (keep the runtime's own QMP socket). For example:

```json
"desktop_probe": ["python3", "/path/to/kore/tools/keytest_supervisor_desktop.py",
                  "/tmp/supervisor-desktop.qmp", "/path/to/evidence/desktop"]
```

Create the output directory before running. The probe opens Power, decodes its
controls, requires Escape to close the frame, then runs fresh `clear`, `time`
and `status exec`, `status mem`, and `status fs` commands through Terminal.
Ready-state probes now compare filesystem session, client-pair, private-view
and pinned-view occupancy against the initial snapshot and fail on growth.
They preserve the failed snapshot before reporting the verdict. MM supply
counters remain cumulative and are not treated as live occupancy.

Saved framebuffers and JSON retain observed task, supply and filesystem-client
counts at each boundary. Every provider must return a fresh available snapshot.
MM child-supply totals are cumulative successful grants, not outstanding
leases; do not interpret their growth alone as a leak. Filesystem client
counts and physical allocator usage are occupancy observations. Compare
equivalent ready boundaries; these observations do not alone prove that every
kernel mapping was reclaimed. The added MM/filesystem observations
require a guest containing Kore `52493011e` or its telemetry correction. The first native run passes twelve desktop probes but fails final supervisor
readiness after the response fault. It is not an accepted recovery result.
A diagnostic rerun passes all thirteen probes and generations 1–7. Preserve
both results; this does not establish the timeout's cause. The focused
`native_response_stop_preserves_desktop` test isolates the final fault on a
fresh supervisor. Both desktop tests also verify authenticated readiness after
each post-recovery probe, so a probe cannot hide a new disconnection.

## Payload configuration

Committed Kore payloads belong to the ordinary disabled configuration. Building
the enabled candidate must rebuild the user/service payloads, stamp their
digests and then build the kernel/ISO with the same selectors. vKore's
`make artifacts` therefore uses `prepare-boot-gate-x86`, rather than invoking
only the final ISO recipe. The enabled build may leave generated payload and
digest changes in the Kore checkout; the candidate lock must record their
exact diff hash as well as the ISO hash. Do not commit enabled service payloads
as the ordinary disabled baseline or treat the old baseline lock as compatible.

## Candidate build and preflight

`config/supervisor-candidate.lock.mk` is an **unaccepted** integration candidate.
The default `config/real-stack.lock.mk` remains the previous pinned baseline;
do not mix its image identity with the optional-supervisor host protocol.
The candidate records the committed ordinary payload base, the frozen source
identity, the enabled ISO digest and the generated payload diff digest.

Use a Kore checkout at the candidate's exact commit and a dedicated state
with the real model/server and workspace configured. Machine-local paths
belong in the ignored `Makefile.local` or command-line variables, not the lock.
Build and verify with:

```sh
CARGO_INCREMENTAL=0 make artifacts \
  VKORE_REAL_STACK_LOCK=config/supervisor-candidate.lock.mk \
  KORE_ROOT=/absolute/path/to/candidate-kore

make dev-check \
  VKORE_REAL_STACK_LOCK=config/supervisor-candidate.lock.mk \
  KORE_ROOT=/absolute/path/to/candidate-kore \
  VKORE_STATE=/absolute/path/to/dedicated-state \
  VKORE_PROJECT=current-kore-integration
```

Use Kore's pinned toolchain for the build. `make artifacts` deliberately
regenerates enabled payloads from the committed disabled baseline. Preserve
the generated diff; preflight verifies it. It must contain only the expected
payload/digest changes, not unrecorded implementation edits.

After existing VM sessions are stopped, launch the same configuration with
`make dev-real` and the same variables. The launcher must be the only active
owner of its VM, disks, HostLink socket and listen port. Do not run the ignored
native restart gate concurrently with the UI launcher. Promoting the default
lock requires the native boot, containment, restart and UI evidence described
in the coordinated plan, not merely a successful build or preflight.

### Native file roundtrip gate

The ignored `kore_runtime::native_supervisor_gate::native_frontend_file_roundtrip`
uses the real application runtime and a configured project with automatic
writeback. In addition to the ordinary gate configuration, set
`file_project_id` and `fixture_backup` to an isolated project and its completed
fixture backup. Its separate transaction-state directory must already exist,
as it does for a project created through `ProjectStore`. `test.txt` and `missing-file.txt` must be absent before launch;
the test never deletes preexisting files. It runs create, read, missing-file,
and arithmetic prompts, verifies the exact host bytes `hello`, and requires
the arithmetic response `42`. A correct missing-file error may have failed
prompt status. This is native integration evidence, not computer-use evidence.

### Uncertain native transaction outcomes

`native_transaction_reply_faults_preserve_effects_and_vm` uses the same isolated
project configuration. Its test-only backend hook pauses an actual successful
host transaction reply after the filesystem effect and durable journal record
exist. The guest tools and model remain real. It then disconnects or requests
supervisor retirement before releasing that original reply. The gate requires
the next generation in the same VM, preserved host bytes, unchanged transaction
journal during recovery, and no stale frontend result. A fresh create after
both faults must also succeed through the replacement supervisor. It never retries the
interrupted prompt. This gate is not a replacement for live authority or UI
acceptance.

`native_no_project_refuses_files_and_answers_arithmetic` requires a separately
pinned image built with `KORE_AGENT_SUPERVISOR_PROJECT=0`. It requires an explicit
validated broker file-action refusal with an actual empty filesystem catalogue,
then `42` and confirmed receipt/retirement in the same VM without tool calls.
The existing frontend may classify that valid refusal as Failed; the gate
requires the actual accepted refusal proposal rather than an arbitrary failure.
Do not run it with the ordinary project-enabled candidate or count compilation
as proof of the absent-authority boundary.

`native_live_authority_revocation_preserves_vm` requires a backed-up
`authority.txt` containing `authority fixture`. It begins a multi-read task,
waits for a real endowed worker's inference input, then submits an authenticated
update revoking the exact file grant while retaining the read executable.
KBR1 returns one action per update; this gate does not claim revocation of all
tool grants. Passing requires native
Installed and Revoked observations, an Applied update receipt after resume,
unchanged fixture bytes, and a subsequent `42` prompt in the same VM. An update
that loses the completion race does not pass this gate.

### Existing-name native acceptance

`native_existing_file_collision_preserves_bytes` takes the same gate JSON as
file roundtrip, but requires both the dedicated workspace and its backup to
contain `test.txt` with exactly `hello`. It asks for a real create, requires the
actual tool result to report the existing name, verifies unchanged host bytes,
and follows with exact `42` in the same VM. Never prepare this by overwriting
a user file; create a separate fixture. Native `sequence-collision-03` passes on its recorded diagnostic image;
see the candidate evidence for exact identities and remaining final-image gates.


### Checks available while macOS computer use is paused

A locked-use pause after local physical input requires a manual unlock before
native Mac interaction resumes. Headless checks remain available: run the
workspace UI tests, the separate reference-render comparison, and guest QMP
acceptance gates serially with isolated disks and exact ISO hashes. Preserve
screenshots, host-byte proofs and failures. Fixture renders do not establish
live Mac focus, input delivery, project selection or real guest UI behavior.
See the locked-Mac checkpoint in [the evidence ledger](supervisor-candidate-validation.md)
for passing headless UI/render checks, init/focus validation, and the locally
reproduced provisioning timeout. After unlock, the four core prompts and the
cancellation/folder-picker regressions were exercised in the native Mac UI;
see [the local fixes report](native-ui-fixes-20260924.md). The locked checkpoint
remains historical evidence, not the latest UI result.
