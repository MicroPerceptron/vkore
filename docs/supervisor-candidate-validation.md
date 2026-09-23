# Optional supervisor acceptance — 2026-09-23

**Not merge-ready.** Native fresh-worker arithmetic and response retirement now
pass. Recovery passes three idle restarts, idle disconnect and disconnect
during real inference in the same VM. Termination after a published response and before receipt also passes.
The enabled artifact reproduces exactly and the four original native file/model
prompts pass functionally. Committed-write disconnect and stop now pass on the
workspace-rebind repair, including a subsequent fresh tool effect in the same
VM. Native two-terminal isolation and no-project refusal/arithmetic also pass.
Live file-grant revocation also passes, including denied reads and a usable
follow-up prompt in the same VM. Existing-name collision also passes with
unchanged host bytes and a successful follow-up. Recycled ordinary desktop now
passes with repaired pointer delivery; final artifacts, failure-time desktop and
UI acceptance remain pending. The default real-stack lock has not been promoted.

## Current source refresh

Kore repair `05fef507f` retires and reacquires the workspace view on supervisor
replacement. This application-only change does not change the exported native
SDK closure. Its diagnostic native fault gate passes; refresh the final source
and artifact lock before promotion.

Kore merged main `515ed58c4` in `9c9c08637461a9986abdf3841ee738d5ddf96a3f`.
Native owner pins now select `2dc4cf1e04436b0a0aeb89ba9f60cbdcef7b01fb`;
its exact SDK export is `3a3f1ddf3d4dd594e1cd22976baf9888a9acbd2a`. The full
Kore host/architecture matrix, SDK matrix, actual-Kore composition including
mixed-owner rejection, and updated vKore workspace tests passed.

The native fresh-worker refusal came from removing DERIVE on the supervisor's
exec wake endpoint; child connection minting needs that authority. The repaired
`wake-repair-fresh-01` returns exactly `42` and becomes ready after receipt.
The separate recovery refusal was a workspace `list page` refresh after attach
request IDs reset to 1. The affine connection now retains its request sequence
and rejects future replies while draining older terminal control replies.
The reattachment regression fails with the original ID reuse restored.
`sequence-recovery-01` passed through supervisor generation 7 in QEMU PID
26289, with one boot and no VM parking, including an actual inference disconnect
and termination after response publication before receipt. These are diagnostic images with exact saved source/diffs, not
promoted release artifacts. Existing failed runs remain below for comparison.

The current guest consumer freeze `580a7c0ce446fd24eb62c2dbb52ec00a7c8c1b9c`
pins SDK `3a3f1ddf`. Its ordinary disabled production image passes
`sequence-default-boot-01` (four CPUs, 4096 MiB, macOS QEMU TCG); ISO SHA-256
`329769455e81ce77a306a949a0d4e3f9a0978b42fcc689ffe33e90dad9dd1dcf`.
Artifact source, generated diff and build log are preserved in Kore's
`target/agent-supervisor-evidence/sequence-default/`. The separate
kernel boot test also passes (`sequence-boottest-01`), ISO SHA-256
`e135d5df4f6c136d738acbc1eb241b96da5db60445eb753aee7b7001698c7884`.
Earlier recycled desktop attempts failed; run 04 passes after the pointer driver repair, as detailed below.

`sequence-files-01` failed before agent admission: the hand-prepared acceptance
project lacked its separate transaction-state directory. Normal `ProjectStore`
creation supplies it. The fixture was corrected, the gate now checks this
precondition before VM startup, and `sequence-files-02` subsequently exposed the focus issue below. No file
operation pass is inferred from the failed setup.

`sequence-files-02` creates the file through a native tool and verifies exact
host bytes `hello`, then fails the read prompt. Personal discovery replaced the
focus folder with hashed read-only aliases, removing `/project/test.txt` from
the worker's view. The host now retains the selected focus root whenever a
search result belongs to it. External-only and virtual-project discovery stay
read-only. The focused discovery regression and the native retest pass.

The unaccepted candidate lock now selects disabled artifact base
`de6206941aad8e06616cd2b5290ebfb093118c7a`, source freeze `580a7c0ce`, and the
new enabled image SHA-256
`6a98af4aa9eff4005c08af874995145b33328b7ff9e04a3d107be9169f4608d3`.
Its exact generated payload diff SHA-256 is
`ed75af931c78f9408b44630e84cd09e71d2dde0f339dd087b6a0b73e8383cb3b`.
Reproduction from the restored committed payload baseline PASSES with the
exact ISO and generated diff. The release host/UI build and `make dev-check`
also pass using the dedicated UI state. `sequence-files-03` passes on this image with the focus fix; the full vKore
workspace suite also passes after that fix. The subsequent uncertain-transaction results are recorded below.

`sequence-files-03` completed all four original prompts through the real
application runtime, guest tools and Qwen model. Create returned success and
host `test.txt` stayed exactly `b"hello"` throughout. The read tool returned
`hello`; the assistant added explanatory prose (strict answer formatting is
not a pass). Missing-file text correctly reported absence, with the known
failed-status chip. Arithmetic returned exactly `42`. Prompt elapsed times
were 269.3, 121.9, 117.7 and 90.3 seconds on this unparked TCG/CPU-model run.
The corrected focus attachment kept QEMU PID 73657 across prompts 1–4.
Evidence: `sequence-files-03/results.json`, complete events in `gate.log`,
source patch, pinned ISO digest, fixture backup and host file. The test exited
0 and reaped its owned processes. This is native integration, not computer use.

Computer-use acceptance is blocked because the Mac is locked and CUA automatic
unlock failed. The user has been asked to unlock it. A separate fresh UI state
is prepared at `~/.local/state/vkore-supervisor-ui-20260923/`; `make dev-check`
passed. Automated native desktop/fault/authority gates continue independently.


### Native results and remaining failures

- `sequence-transaction-faults-01` and `-02` reach the real committed-create
  boundary, withhold its original host reply, disconnect and recover in the
  same VM at generation 2. File bytes and the durable journal are unchanged
  by recovery. The next prompt fails during workspace discovery with
  `tool founding refused`; those runs **failed**. A native
  diagnostic preserves the underlying exec refusal instead of dropping it:
  `sequence-transaction-faults-04` reproduces `Denied` on ISO SHA-256
  `0b52d48cf472c4dbc1042dabecf5133c2e39614cdb3a2378f4bf2665283451f8`.
  Investigation found that interrupted committed creation can reimport and
  revoke the workspace grant, while the launcher retains its old bound view
  across supervisor generations. Retiring and rebinding that view passes
  native validation in `sequence-transaction-faults-05`: **PASS**, 518.46 s,
  ISO SHA-256 `5af7bb6cee59f3e955a0a62eff725d14f8e3f5f5511e2aa9ef1364ee51d38971`.
  Both faults preserve bytes/journal without replay, generations 1–3 retain
  QEMU PID 71090 unparked, and a fresh create succeeds after the final fault.
  The repaired source still needs final pin/artifact refresh.
- `sequence-desktop-01`: ordinary final disabled image; editor persistence and
  all 12 terminal/Power recycling cycles pass. Power does not present when
  Terminal plus Aftershock, Skylilt and Files are already active. Full gate
  exits 2; 106 captures are preserved in `frames.tar.gz`.
  Fresh capacity-only `sequence-capacity-01` PASSES on the same ISO in 127.46 s:
  actual Power controls render and `window list` confirms all five live
  presentations. This rules out a fixed four-window ceiling; it does not
  erase the earlier recycled-session failure. Full rerun `sequence-desktop-02` reproduces the failure in 695.26 s.
  The terminal renderer echoes keys, but `window list` never returns.
  The compositor still renders pointer/chrome. This is a shell/controller
  stall, not merely an unrecognized Power window. All 110 captures are
  retained in the verified `frames.tar.gz`; duplicate loose copies were
  removed only after byte-for-byte archive verification. A diagnostic shell
  uses the standard native panic reporter instead of silently spinning.
- `sequence-two-terminal-08`: **PASS**, 432.15 s, disabled userland-gate ISO
  `9ba9f188c8c76b896c0f64bf0c23c4eff2581518a88a65dac2abca34809cc2ed`.
  Actual task-owned keyboards/framebuffers prove private roots, explicit live
  read-only sharing, denied write/directory/move operations, unsharing,
  independent cwd and executable namespaces. The corrected driver is Kore
  `76d220e85`; all 47 focused driver/returning-main decoder tests pass.
  Earlier retries exposed test-driver problems: day-only pixel decoding,
  a pointer obscuring terminal cells, then an obsolete `help shutdown`
  expectation. Actual catalogue power control is `pwctl`; shutdown/reboot are
  arguments, not standalone commands. Modern `hello` returns silently, so
  post-refusal liveness now uses real `time` output.
- `sequence-no-project-02`: **PASS**, 180.44 s, PROJECT=0 ISO
  `f13d6e1d3c10d311e3e97b148405f3ac1fa9b1d07cae7130b7757652c9bd8a2e`.
  The actual broker catalogue has no filesystem grants or filesystem tools;
  creation yields an accepted `refused` proposal. Arithmetic then returns
  exactly `42`, with no tool calls, VM replacement or parking. Run 01 stopped
  on the known frontend `Failed` classification of that valid refusal; the
  corrected gate requires the validated refusal and actual empty filesystem
  authority rather than accepting an arbitrary failure status.
- CI run `35828436910` initially fails AMD and Intel 4k57 provisioning with
  `APP_PROJECTION_LINK` (74). The exact CI image SHA-256
  `8fad587979f45036b29905cf77444e413a18845d14bf5f943f15338afdf977e0`
  passes `sequence-ci-4k57-01` under macOS QEMU TCG (4 CPU, 4096 MiB,
  `PAGING_PROFILE=4k57`), including all 35 app seeds and package manifest.
  Intel and AMD production reruns both pass (attempts 2 and 3).
  These passes do not erase the original intermittent native hardware failures.

An attempted fixture-before-preparation setup failed the runtime precondition
before starting a VM (`sequence-transaction-faults-03`). It was reverted; gates
retain the application preparation followed by explicit project attachment.
Existing-name collision
coverage is implemented as `native_existing_file_collision_preserves_bytes`,
with backed-up `test.txt = hello`, required native create refusal, unchanged
host bytes, and a subsequent arithmetic prompt in the same VM. Both native gates have now run and exposed the failures recorded below.

`sequence-collision-01` stops before model planning because the test's 157-byte
intent exceeds the existing 128-byte `TASK_INTENT_MAX`. It preserves fixture
bytes but cannot certify a collision. The test now uses a 111-byte equivalent
intent and checks the protocol bound before VM startup. The authority-update prompt was also shortened to the existing bound.

`sequence-collision-02` reaches real model planning but the broker selects the
existing file object, not the directory authority required by create. Native
admission correctly refuses `Backend(Filesystem)`; no create tool runs and
fixture bytes are preserved. This does not certify collision handling. The
next prompt explicitly requests the project directory capability (122 bytes);
no native rights have been widened. Native `sequence-collision-03` PASSES
in 336.22 seconds on the same `8c6f4f2e` diagnostic image: real create returns
`create: already exists`, the assistant accurately reports the collision,
host bytes stay exactly `hello`, and the next prompt returns `42` in the same
unparked QEMU PID 90884. The broker also selects an unnecessary write tool;
no write is executed.

`sequence-authority-01` admits the exact read-only file object and executes
five reads, but the live update fails during planning. Source inspection found
that live-update planning treated bulk upload acknowledgments as terminal
failure, unlike initial planning. The fix retains the same revision/request/
offers for `BULK_READY` and `BULK_ACCEPTED`. All 121 runner and 3 supervisor
tests pass. Native `sequence-authority-02` PASSES in 506.24 seconds on diagnostic ISO SHA-256
`8c6f4f2e70e48a76d15efa69cdd860a61b7b3b5c8738766e33a0701493cec6ef`;
source base `fa628da409ab99765fd4371f049f2704576f86bc`, exact source/payload
patch and build log are retained in Kore `target/agent-supervisor-evidence/
live-update-progress/`. The file object is revoked; all five subsequent native reads refuse with an
empty resource list. A follow-up prompt returns exactly `42` in the same
unparked QEMU PID 80990; the fixture is unchanged. Captured events were
independently checked in `revocation-observations.json`. The broker selected
one `revoke_grant` operation, so removal of every tool grant is not claimed.
The gate now also asserts the post-revocation refusal directly; that additional
assertion compiles in the collision run, and will run at final acceptance.

Before the main refresh, disabled production boot and release boottest passed
at source freeze `1562f5b9b` (`repaired-default-boot-01`, `repaired-boottest-02`).
The desktop gate failed because its driver clicked the dock before dismissing
the welcome surface; the saved frame verifies this precondition error. Its
Return-key correction still needs native window-lifecycle acceptance.

`repaired-native-faults-02` uses enabled ISO SHA-256
`06734e66e7cf7a3630dfd465080290e73c550519698fa7e2263a3df48b7b248c`,
artifact base `c3a3dcd26`, source `1562f5b9b`, and exact generated patch SHA-256
`14cd37ac19d266f1efe0dc7f2b47e87af742f6b08294d312994793d3e0b0e075`.
It passes three idle restarts and idle disconnect in the same live VM, then
fails with `Workspace admission could not be completed` before the requested
inference fault. The failure and predecessor timeout remain preserved. These
results must not be described as passing in-flight fault coverage.

The earlier checkpoint below records how startup was repaired; it is not the
current final source/artifact tuple.

## Current source checkpoint

| Component | Revision |
| --- | --- |
| Native fixes / host owner pin | `b7fefa3156ad9f2bcbcb057403becb29e618d50d` |
| Exact SDK export | `fcf434490304802be7987b2b7c4460960169310a` |

The capacity repair is Kore `9702a67b6`; the next commit preserves pending exec
construction across transient observation errors and adds launch diagnostics.
The guest is being repinned to the SDK above. Final disabled payloads and the
enabled candidate lock must be rebuilt after that source freeze. The current `config/supervisor-candidate.lock.mk` supersedes this older
checkpoint; it remains explicitly unaccepted until the integrated gates pass.

The SDK full fmt/test/check/docs/owners/cross/Miri/bindings matrix and actual
Kore composition (including mixed-owner rejection) passed. The host's full
workspace suite and release binaries passed with the new Git-native owner pin.
Kore's full host and final image/architecture checks are being refreshed.

## Native observations

Evidence root: `~/.local/state/vkore-supervisor-acceptance-20260922/diagnostics/`.
Every native run uses dedicated disks, firmware variables and serial logs.
The test fixture began empty and has a pre-test backup.

- `default-boot-02`: disabled production boot passed on macOS QEMU TCG with
  four CPUs and 4096 MiB. ISO SHA-256:
  `9eeac9103e7dac8f7daa65686028b4ef77bbd59f571ea3c011856756e672d168`.
  G7 retirement was not exercisable on this topology. The earlier two-CPU run
  reached the shell but failed exclusive SPMD reservation; its failure remains.
- `native-restart/gate.attempt02.log`: the first enabled image reached complete
  provisioning and shell readiness but never started the supervisor.
  Diagnostic03 identified the runner's exact `TableFull` refusal: the new
  resident set required 42 image slots, while the ABI reserved 41.
- `native-restart-reservation01`: after correcting capacity, image acquisition
  and project binding succeed. Exec-client construction repeatedly enters
  cleanup because the launcher treated a short observation error as terminal.
  The fix resumes the same affine owner and pending request without extending
  the overall startup deadline.
- `native-restart-mint-resume02`: **three idle restart cycles pass**, generations
  1 through 4, one boot, the same live QEMU PID 37975 and no parking. Initial
  authenticated readiness took 70.7 seconds including model startup; recovery
  readiness took 3.48, 3.13 and 3.15 seconds. Teardown completed successfully.
  Diagnostic ISO SHA-256:
  `23b63ecab8f18b053be088dfeb9c72921c4ac15c406f85da672d3f3feedd82c4`.
  BLAKE3: `e2b82cfb373e249e7d050476691725a8e634e2e0d1409d073e07caaad29e7769`.
  Source base: `9702a67b645d34d1ae5d2a0e07bf021117050d48` plus the run's recorded
  binary source/payload patch. Patch SHA-256:
  `f6e8ce0874aa791b27e718fe06bfc7a0a1b8b1feb59626e15c1dbf0c12a60854`.

These diagnostic images are not the final release tuple. Earlier gate-driver
failures are retained: a host stack overflow before VM launch, telemetry sent
before mandatory init configuration, and a wrong assertion that the broker
route must remain unchanged. The corrected restart gate requires a fresh
route per generation, matching HostLink's authenticated rotation contract.
A separate broker test now checks close acknowledgment and workspace retention.

## Running-session image incident

The original manual QEMU session mounted the build-output ISO directly. Builds
overwrote that inode; its original exact backup was not found. It is not valid
acceptance evidence. The incident record remains in the evidence root.

The user subsequently authorized stopping that session. Its owned UI, launcher,
VM and model processes were retired. The original workspace was not changed.
New `QemuProcess` instances own a private ISO copy until retirement; both the
regression test and the real native runs verify the separate mounted path.
This fixes subsequent launches and does not retroactively repair the old run.

## Remaining acceptance

Follow [Kore #1053](https://github.com/MicroPerceptron/kore/issues/1053) and
[the run instructions](supervisor-acceptance.md). Remaining gates include final
pinned artifact reproduction and applicable boot/boottest/desktop checks,
missing and narrowed authority, isolated/shared views, live revocation,
in-flight cancellation/termination/disconnect, native capacity reclamation,
and real UI create/read/missing-file/arithmetic, host-byte verification,
recovery, drafts and project/chat selection. Three idle restarts do not prove
those scenarios. Only one QEMU owner may run at a time.

### Pointer delivery and returning-main observations

`sequence-desktop-04` **PASSES** in 466.12 seconds on diagnostic disabled ISO
`884241d03bbf3b84781fa1f0650d7c5cb7c4036bd30b1bdf6aa214f7999a82a3`:
12 recycling cycles, editor save/reopen and five concurrent windows pass.
Driver repair Kore `d8c2c87cb` witnesses each individual HID-sized pointer
report before another move. Both new regressions fail with the old algorithm;
46 focused driver tests pass with the repair. Run 03 failed from overlapping
motion, while its Terminal still answered diagnostic commands. The earlier
shell stall does not reproduce on run 04; its hypothesized cause is not a
confirmed fix. Exact diagnostic patches, frames and logs are retained.

`sequence-runtest-observe-02` on the preserved `9ba9f188` userland ISO passes
all 20 hello rounds and the lifecycle variants, then fails at bare `status`:
exit 4 (provider unavailable), expected 0. The initial welcome-screen theory
is not established; first-command testimony and framebuffers prove successful
hello execution. Do not weaken the provider requirement or report this as a
passing returning-main gate. Run 01 never booted because its QMP path exceeded
the Unix socket length limit; run 02 uses a short path to isolated evidence.

`sequence-desktop-faults-01` exercises the new real desktop probe on the
recorded enabled `8c6f4f2e` image. Its failed result is detailed below.

`sequence-desktop-faults-01` **FAILS** (580.65 s) on enabled `8c6f4f2e`.
Twelve real desktop probes pass: Power renders/closes and fresh Terminal
commands execute around three idle restarts, idle disconnect, inference
disconnect and response-before-receipt stop. Each snapshot reports four
retained exec rows; QEMU PID 32212 remains unchanged. The final replacement
never confirms readiness. Native init repeatedly alternates Project and
ProjectCleanup (kind-2 steps 16/17), so project-view binding is the next
diagnostic target. A frontend SelectorUnknown refusal and transport
reconnection precede it. This is a failed combined recovery gate, not a
passing containment result. Frames, exact host source, logs and twelve
checkpoint records are retained; all test-owned processes have exited.


### Diagnostic full desktop/fault rerun

`sequence-desktop-faults-02` **PASSES** in 446.67 seconds (451.70 s wrapper)
on diagnostic enabled ISO SHA-256
`7fc1a6ad2aa4f617166fb3ba8133cd641274e9ccf3354878b5427f81487316ec`,
BLAKE3 `2864bc15bd69799a645200cc717907c5702e8b882b0afee5a3e00ee10fa46a20`.
Kore source base is `c57ed07820bac7ed41060140b4cd4777c684012d`, with the
saved bind-result diagnostic patch; host base is
`de9b21550d263c393711cc3cc97e4c3ea5684cbc` plus the saved test patch.
All thirteen Power/Terminal probes pass with four retained exec rows, and
supervisor generations 1–7 use QEMU PID 62000. Authenticated readiness remains
valid after each recovery probe. Project binding reports transient
`change-directory: storage failed` and then succeeds; no recovery policy was
changed. Run 01's prolonged failure is **not explained or claimed fixed**.

The focused `sequence-response-desktop-01` also passes on the same diagnostic
image (223.59 s wrapper): actual arithmetic 42, stop before receipt, three
Power/Terminal probes, generations 1–2 and QEMU PID 57744. A later storage
failure in its serial tail occurs during test teardown, when the host link
has intentionally disconnected; it is not evidence of spontaneous live failure.

Both runs preserve exact source patches, logs, hashes and JSON probes under
`~/.local/state/vkore-supervisor-acceptance-20260922/diagnostics/`.
Raw PPMs were archived and individually hash-verified before deleting duplicate
loose copies (24 and 104 frames respectively). Diagnostic guest edits are
restored before building release artifacts. Host `cargo test -p vkored --lib`
passes 91 tests with 9 native tests ignored; ignored tests require their real
native runs and are never counted as unit-test passes.


### Optional acquisition refusal — 2026-09-23

`sequence-missing-supervisor-01` passes (99.79 s wrapper). The diagnostic
changes only the optional supervisor acquisition name to a valid missing
`boot:missing-agent-supervisor`, with autostart enabled. Native init reports
kind 2/index 3 acquisition, kind 3/index 2 refusal, then kind 2/index 8 skipped
launch before founding the shell. Power renders/closes, Terminal clears and
executes time/status exec, reporting three retained rows and one compensated
founding. No supervisor-ready claim is made. Exact source base is
`c57ed07820bac7ed41060140b4cd4777c684012d`, source patch SHA-256
`c93f94138e988cef4f164196ff4436762236849c1a85dc117f970a65fc134ac6`;
ISO SHA-256 `a3763cc95dfc642277703858bcc1c86c549d87a55df69bc587cb458028b862f3`,
BLAKE3 `715167dc7fc243777b505e081bbefe9e90dbeba65df24cde11c2e08bc170dacf`.
The prebuilt run wrapper's selector 0 does not rebuild this enabled image;
its verified digest and actual acquisition/refusal testimony identify it.
Diagnostic source/payload edits were restored, with exact patches retained.
Frames were archived and byte-verified before removing duplicate loose copies.

Additional status observations isolate the local provider failure to the
scheduler/manifold section: `status sys` and `status cpu` show per-core rows
but `SCHEDULER / MANIFOLD provider unavailable` and exit 4. Memory, filesystem
and network sections render. This does not pass returning-main or explain
its separate CI first-command witness failure. The provider assertion remains
unchanged pending diagnosis.
