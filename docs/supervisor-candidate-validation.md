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
passes with repaired pointer delivery. A full desktop/fault diagnostic rerun
also passes functionally, with its earlier timeout retained. New capacity checks
exposed accumulating pinned filesystem views. A kernel-validated cleanup fix
passes the final full fault gate with unchanged view counts through generation 7.
The corrected kernel boot, returning-main and two-terminal gates pass. The extended desktop repeat passes; its earlier intermittent Files-activation failure remains unexplained. Hardware CI remains unresolved. Native Mac regression checks pass with the host/UI fixes recorded below. The default real-stack lock has not been promoted.

## Current coordinated candidate

The unaccepted x86 candidate now includes the projection timeout fix at Kore
`c6dd5fe714183c71475e5fa4372dfde9a7747c1d` (both source and artifact base).
SDK `688d5bfb40db47a63d5e0f45593898b42d6f9d61` and host native owner
`60e659644acf6e5e730b6b566a68765f8016563c` are unchanged. ISO SHA-256:
`fef674e7906dfb84d36d16ba8a0c3d0f62e1ac37c3605d93c15b910b6237b367`;
BLAKE3: `afb93f8a0dfe4c4035a163e4ebd59f910f60bba657caa0fe6db3f8130bfac38e`.
The generated payload diff from the clean tracked baseline has SHA-256
`aeae492476f717afa805b0d49d07da6f2808b9c8bf68af0a1ee03848508eeda2`.

The enabled build passes. `make artifacts` from the clean committed baseline
reproduces the ISO byte-for-byte, and launcher `make dev-check` passes with the
candidate lock and dedicated real-model state. The first working-tree diff also
contained older ARM diagnostic artifacts; the clean rebuild removed those
dependencies from the candidate record without changing any x86 image bytes.
Both original and clean payload records are preserved under Kore's
`target/agent-supervisor-evidence/projection-enabled-c6dd/`.

`projection-enabled-files-01` passes in 608.43 s (607.90 s native test) against
this exact image using the real model and guest. Boot reports 35/35 apps,
3/3 libraries, 2/2 data, package present and library failure 0. All four prompts
run in the same VM: create and write succeed, the independent host file is
exactly five bytes `hello`, guest read returns `hello\n`, missing-file handling
accurately reports absence, and arithmetic returns exactly `42`. The model
still wraps readback in prose; the valid missing-file refusal still receives
Failed status. Neither behavior is relabeled fixed by this patch.

This does not assert that the timeout continuation fired
during boot. Earlier recovery/authority/desktop results below belong to their
recorded images; they are not reruns on this candidate. Native Mac computer-use
checks now pass the four core prompts, Retry, layout and project draft isolation,
and initially exposed cancellation-confirmation and blocking folder-picker failures.
The fixes now pass native cancellation followed by exact `42` in the same
VM, and picker selection/cancellation with responsive HTTP during inference.
See [the original native UI report](native-ui-acceptance-20260924.md) and
[the fixes and final regression evidence](native-ui-fixes-20260924.md).
Hardware CI remains unresolved. These checks
do not rerun the full fault matrix or close the separate filesystem-inventory
UI limitation. No default-lock promotion.

## Prior coordinated candidate (before the projection fix)

Native owner `60e659644acf6e5e730b6b566a68765f8016563c` is mechanically exported by SDK
`688d5bfb40db47a63d5e0f45593898b42d6f9d61`. All required local SDK checks, actual-Kore composition and
all seven SDK GitHub checks pass. Kore consumer source is `e89901a1bf05c4f7949f085548b05c80765e1fc9`;
disabled payload commit is `a99e0c539a2578b6b973c003f318dfe3b855ec4a`.
The current packaging base is `d65ddeb357a8fbb8004bff1c1690e682f07b811f`.
Kore's full host/architecture matrix and vKore's locked workspace pass;
all three vKore release binaries build successfully.

The disabled production image passes `canary-final-default-01` in 46.25 s,
with all 35 applications seeded and shell startup verified. SHA-256:
`5d54dca9b28abc7d40ca5efb8067a36f05d3c61f9ca07a408cda1154a14d0c3d`.
The enabled image is built: SHA-256 `b8a6f1d8609d19365bc4fd00fce457f2af150eaab3c0532504cca2aebe599df1`,
BLAKE3 `d9b9178af33b6b44a96281ccc102074d72de82de716181b7d255da90c80ed24c`, generated-payload diff SHA-256
`9ba86caf28b901d79f6beac03097062f59f8b50bf7b0ea7098f426051d1d2af0`. The unaccepted candidate lock selects these identities.
The earlier `a99e0c539` packaging base passed clean reproduction and launcher
preflight. The current `d65ddeb35` clean reproduction and `make dev-check`
also pass against the dedicated real-model UI state. Both ISO filesystem
inventories and the complete generated-payload patches are byte-identical
between bases; only the deterministic ISO packaging metadata differs.
The final full fault gate passes all thirteen probes through generations 1–7
with stable FS capacity in one VM. Kernel boot, returning-main and two-terminal
isolation also pass on their distinct recorded products. The extended desktop repeat passes with the earlier intermittent failure
retained. UI and unresolved CI remain open. No default lock promotion.

The candidate packaging head `d65ddeb357a8fbb8004bff1c1690e682f07b811f` changes only
the returning-main test driver, its regression and this project's evidence ledger.
It does not alter guest payload source or SDK exports. The candidate has now been resealed for that exact commit and its lock updated.
Preflight continues to require the recorded revision, image and generated diff.

Kore CI `35859572970` now passes its host job after the cache correction.
Captured hardware failures persist, including AMD returning-main stopping
before shell readiness. Current-main CI `35807492440` also fails, but captured
Intel window-lifecycle and AMD returning-main/MMIO failures occur at different
boundaries; they do not prove the branch's provisioning failures are inherited.

## Prior telemetry source tuple

The preceding native owner was `52493011e3491f955f478f92e895731af9fa9604`,
with byte-exact SDK export `9ab4ee4b537c437cdc2a5e39ee34e8548358b157`.
Kore consumer source freeze is `c0b7536026fbee512f17d32f54bd68aa53f5704b`;
committed disabled ARM/x86 payload base is
`00cf2d40230f9da39198393899ee033a43dd4920`. Full Kore host tests and all
architecture checks pass. Required SDK checks, Miri, cross-target bindings and
actual-Kore mixed-owner/identity checks pass; all seven SDK GitHub checks pass.
The refreshed vKore workspace passes 372 tests, with 12 ignored tests requiring
separate acceptance rather than being counted as passes.

Disabled production `final-default-boot-01` passes in 42.03 seconds (4 CPUs,
4096 MiB, Intel profile under macOS TCG), ISO SHA-256
`89ef0216e4236192b775fb91de2e845967a7200470a556a52cd378b5c60fc71f`.
The initial final-image packaging attempt omitted environment variables needed
by the reproducible ISO wrapper; payload compilation/sealing had succeeded.
Corrected packaging passes; both attempts and exact hashes are retained.
The enabled image and candidate lock still require refresh and validation.

## Source refresh history

Kore repair `05fef507f` retires and reacquires the workspace view on supervisor
replacement. This application-only change does not change the exported native
SDK closure. Its diagnostic native fault gate passes; refresh the final source
and artifact lock before promotion.

Kore merged main `515ed58c4` in `9c9c08637461a9986abdf3841ee738d5ddf96a3f`.
That checkpoint selected native owner `2dc4cf1e04436b0a0aeb89ba9f60cbdcef7b01fb`;
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

The earlier guest consumer freeze `580a7c0ce446fd24eb62c2dbb52ec00a7c8c1b9c`
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

The earlier unaccepted candidate lock selected disabled artifact base
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

## Earlier source checkpoint

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
[the run instructions](supervisor-acceptance.md). The later ledger entries record
completed native authority, file, fault/recovery and capacity checks on their
exact artifacts. Remaining work is diagnosis of the unresolved native/hardware
failures, final source/SDK/artifact convergence and reproduction, applicable
native gates on that final tuple, and live macOS UI create/read/missing-file/
arithmetic, recovery, drafts and project/chat selection with independent host
readback. Headless UI tests and fixture renders are separate evidence and do
not complete native computer-use acceptance. Only one QEMU owner may run at a time.

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

Additional status observations show missing telemetry in the
scheduler/manifold and memory sections: `status sys` and `status cpu` show per-core rows
but `SCHEDULER / MANIFOLD provider unavailable` and exit 4. Memory reports both the system allocator and memory service unavailable;
filesystem and network sections render. This does not pass returning-main or explain
its separate CI first-command witness failure. The provider assertion remains
unchanged pending diagnosis.


### Native telemetry correction

Kore `52493011e3491f955f478f92e895731af9fa9604` stops applying the 5 ms
abandoned-operation maintenance slice to fresh MM Stats requests. The ordinary
reply wait retains original-request custody; recovery remains bounded. The
prior early return also prevented monitor/manifold observation. Seven focused
exec recovery tests and 28 MM client tests pass.

`sequence-provider-after-02` passes all six status forms (bare, sys, cpu, mem,
fs, net), with fresh framebuffer output and exact new exit-0 serial testimony,
in 124.42 seconds on disabled diagnostic ISO SHA-256
`fc08f9ab0a8a7e7f312161110bf2fd40a83e057e30b288a8485b4f08e4606fdf`.
Memory now reports actual allocator/supply data and all three manifold queries
reach the kernel. The original after-run returned 0 but its screen assertion
required a footer truncated by the existing process reply limit; the corrected
check accepts that explicit truncation and still requires exit 0. Diagnostic
kernel logging and payload changes were restored. This is not yet a final
artifact or original returning-main acceptance result. All failed runs remain
retained alongside the passing run, including the seven hardware scenarios
that failed CI run 35847395393.


### Final telemetry candidate exposes retained filesystem views — 2026-09-23

The source-frozen disabled image passes the production boot gate
(`final-default-boot-01`, 42.03 s, SHA-256
`89ef0216e4236192b775fb91de2e845967a7200470a556a52cd378b5c60fc71f`).
The enabled image passes launcher preflight and all thirteen Power/Terminal
probes through supervisor generations 1–7 in the same QEMU PID 92680
(`final-desktop-faults-01`, 1044.14 s wrapper, SHA-256
`98fb97f89c371fbc1c6b93f0c334e3ebe10f54b35bd5e50489847e508c3ae1d9`).
However, it **fails capacity acceptance**: pinned filesystem views increase
from 2 initially to 3, 4 and 5 after the three idle restarts and keep increasing
through the later faults. The process-level test's exit 0 proves its recorded
functional assertions only. `capacity-verdict.json` retains every provider
snapshot and the separate failed verdict. No lock promotion or merge.

Cleanup used a read of the canary's immutable local capability record as a
liveness test. Native ancestor revocation leaves that record unchanged; the
kernel validates the lineage when the capability is used. The pending fix
uses the existing same-kind derive/revoke probe pattern for send endpoints.
A non-delegable live canary remains non-delegable and returns an uncertain
probe result; only an explicit kernel Revoked result permits retirement.
No notice is sent and no binder rights are broadened. The real kernel test
proves live Denied, unchanged live canary, then Revoked after parent teardown.
Runtime and filesystem host suites pass. Native correction validation is
pending. Future recovery probes fail when ready-state filesystem session,
client-pair or view occupancy exceeds its initial value.


CI run `35855204535` at `00cf2d402` records successful kernel, userland and
build-tool test steps, followed by a GitHub runner `No space left on device`
exception while finishing the job. The exact job and annotations are retained
in `diagnostics/ci-00cf2d402/`. Host tests now retain compiler/download caches
but omit the multi-gigabyte build-output restore/save, avoiding that extra
storage peak on a hosted runner. Hardware failures are separate: six captured
lanes fail incomplete seeding, while both returning-main captures lack the
first described witness. Intel's framebuffer contains a long repeated `l`;
AMD's initial frame is the welcome screen. Returning-main now shares the
already-tested explicit HID press/release implementation; 49 focused driver
tests pass. This is not yet a native or CI confirmation of either correction.


### Kernel-validated filesystem canary — native correction confirmed

`canary-response-recovery-01` **passes** (353.67 s wrapper): a real worker
responds exactly `42`, its supervisor is stopped before receipt, and the
replacement reaches generation 2 in the same unparked QEMU PID 30392. Three
Power/Terminal probes pass. Each fresh FS snapshot has exactly 3 sessions,
0 client pairs, 1 private view and 2 pinned views, including after recovery;
the new capacity assertion passes. The preceding candidate accumulated
2 → 10 pinned views across the full fault sequence. The kernel regression
separately proves live non-delegable denial versus ancestor-revoked closure.
This establishes the targeted correction, not complete resource reclamation
or final release acceptance.

Diagnostic source base is `00cf2d40230f9da39198393899ee033a43dd4920`, saved
source patch SHA-256 `27f7214b65db183382b95fe0ea0c4036eeada98c7db4c9a2ecb39942b1e79508`.
ISO SHA-256 is `2200df55f85b8e7245c3d4ad522dba40185466d60cdf0c274ddd261f16cd5cc7`,
BLAKE3 `83b2b31c9008e128eafd494d6dd2464be87c8ad9cba347d41a49e61c1e2eb62a`.
Exact source/payload patches and all byte-verified framebuffer archives are
retained. Generated payload edits were verified against the saved patch and
restored before the next source cut. The run uses the recorded diagnostic
image and host native owner `52493011e`; a new coordinated export, final
artifacts and the full fault/ordinary-boot/UI acceptance still remain.

Host checks before the full matrix: 1,031 runtime/filesystem tests passed,
7 ignored; the new kernel regression passed; all 49 keyboard/returning-main
driver tests passed; 9 build-cache tests passed, 1 skipped. The first host
invocation accidentally selected stable Rust and failed on required nightly
features; the preserved successful rerun uses the checked-in compiler. No
result is inferred from the earlier empty test discovery.

Storage cleanup removed 924 compiled library/metadata objects (1,576,298,417
bytes) whose dependency records identified twelve obsolete native revisions
absent from the current locked graph. Current owner 52493011e, executables,
source, models and evidence were retained. The dependency records and paths
are saved in `diagnostics/obsolete-native-codegen-cleanup-20260923.json`.


vKore's updated native pin `60e659644` passes the complete locked workspace
suite (372 tests passed, 12 ignored) and release builds of `vkore-harnessd`,
`vkore`, and `vkore-ui` (25.68 s). Cargo changed only the nineteen native Git
source entries. The initial release command mistook a binary name for a
package name and stopped before compilation; the corrected explicit binary
build is the passing check. The final artifact lock remains unpromoted.


### Coordinated full fault and filesystem capacity acceptance — 2026-09-23

`canary-final-desktop-faults-01` **passes**: 944.63 s native test, 958.38 s
wrapper including compilation/setup. All thirteen Power/Terminal probes pass
through three idle restarts, idle host disconnect/reconnect, disconnection
during real inference and supervisor stop after the real response `42` but
before receipt. Every generation 1–7 uses the same unparked QEMU PID 6691.

Each snapshot reports exactly 3 FS sessions, 0 client pairs, 1 private view,
2 pinned views and 4 retained exec rows. This verifies the FS cleanup correction
against the earlier full-run baseline that accumulated 2 → 10 pinned views.
MM class allocations remain 208.0 MiB (4K) and 600.0 MiB (2M). Reported physical
usage increases 803.2 → 806.1 MiB and kernel heap 14.7 → 16.6 MiB; cumulative
child supplies increase 94 → 372. These observations are retained, not turned
into a claim that every kernel allocation or mapping has been reclaimed.

Host source is `a49eff47c1ed06834e6c2b7eebb8c95ec8fff532`, native owner
`60e659644acf6e5e730b6b566a68765f8016563c`; the tested enabled ISO is
`d3da72bf02df450ae51ee80d9a3629349e6ff8db0f9b2bc5d617529df178d3c5`.
The exact SDK/source/artifact tuple appears above. `capacity-verdict.json`
records all provider snapshots; completed frame archives are byte-verified
before loose copies are removed. No guest/model process remains after the run.

Storage maintenance removed 277 compiled library/metadata objects belonging
to ten obsolete SDK revisions absent from the current userland lock graph,
reclaiming 260,017,308 bytes. Current SDK `688d5bf`, source, executables, models
and fixtures remain. Exact dependency records and deleted-object paths are in
`diagnostics/obsolete-sdk-codegen-cleanup-20260923.json`.


### Final kernel, returning-main and terminal isolation gates

- `canary-final-boottest-01`: PASS, 14.47 s, native kernel-test image SHA-256
  `97e0a1a52cbce8ca1b51d71709f9aaf91ad7a6ef2f4288674140a70298e45316`.
  This executes the actual boottest product, separate from production founding.
  The capture helper initially selected the production filename; that metadata
  was retained and corrected before the gate ran.
- `canary-focused-returning-main-01`: PASS, native and wrapper exit 0, 216.52 s.
  Twenty hello cycles, full-width status/exit results, main return/finalizers,
  authority, nested/detached/controller workers and later status commands pass.
  Driver SHA-256 `aa7ab4c76e9a27d52d1b8f0aa7c32db57168033028839ff677f7303f5f7d2a28`.
  Hardware CI screenshots had shown the second terminal stealing focus before
  the first Return. The driver now waits for both seats and stable first-seat
  focus; all original guest assertions remain. Fifty driver tests pass, including
  delayed seat presentation and stolen-focus coverage. The first native run also
  reported PASS but its wrapper failed during capture; only this rerun records
  both exit codes successfully.
- `canary-final-two-terminal-01`: PASS, 469.94 s, driver checkout `d65ddeb35`.
  Sibling tasks retain isolated input/exec, task-local help and private roots.
  Explicit shared grants expose the same object, reject unauthorized writes,
  creation and moves, and revoke access without damaging the owner's bytes.

The last two gates use the interactive image SHA-256
`4b6e00b96157beacf23a081002a480c0315946cce7cddffcd1e1eef5b9bca871`,
built from source `e89901a1b` at artifact base `a99e0c539`. Run manifests,
commands, logs and captures are retained under the dedicated acceptance
state's `diagnostics/` directory. Hardware CI for `d65ddeb35` is still running;
its three architecture checks pass. Native host computer use remains blocked
by the locked Mac, independently of these guest desktop tests.


### Final ordinary desktop failure retained

`canary-final-desktop-01` fails on disabled production ISO
`5d54dca9b28abc7d40ca5efb8067a36f05d3c61f9ca07a408cda1154a14d0c3d`.
All twelve Power/Terminal recycling cycles pass with exactly three retained exec
rows; spawned/reclaimed counts reach 77/74. Terminal output, unsaved editor text,
post-layout keyboard input and saved-file reopening also pass. The final
concurrent-window sequence opens Aftershock and Skylilt, but Files does not
publish the required focused tile. The captured frame keeps Terminal focused,
without a Files launch result. The bounded gate fails; this is not a final
ordinary-desktop acceptance pass or proof of a specific guest/input defect.

All 99 raw frame duplicates (270,830,903 bytes) were verified against the retained
archive before removal. The run manifest, native failure, serial log and frame
archive remain. A diagnostic repeat preserves the original assertion and adds
failure-time window/provider observations; its result is still pending.

An independent code review found pre-existing running-dock activation that
selects by generic application kind, without the selected app identity. It is
tracked separately in [Kore #1058](https://github.com/MicroPerceptron/kore/issues/1058);
no causal link to this failure is claimed.


`canary-desktop-capacity-diagnostic-01` subsequently **passes** in 537.46 s
on the identical disabled production ISO, including all twelve recycle cycles,
editor persistence and all five concurrent windows. The copied driver retains
the original assertions and constrains the capacity phase to 120 s; its added
failure-time diagnostic path is not executed. This is a complete passing run,
not a causal fix for the prior intermittent Files-activation failure. Both runs
remain part of the release evidence.

Hardware CI `35867475708` passes host tests, all architecture checks, both user
application cross-builds and both vendor kernel boot tests. Intel production
and 1920x1200 display pass. AMD display fails with only 33/35 apps seeded and
`APP_PROJECTION_LINK` (74); the exact filesystem cause is still collapsed by
that verdict. The initial 4K57 interactive builder stops inside its compile
action without a published job log; the workflow's On-Demand recovery succeeds.
These results do not justify treating either failure as a passing gate.


### Resealed candidate file prompts

`canary-resealed-files-01` passes in 604.12 s (603.70 s native test), on ISO
`b8a6f1d8609d19365bc4fd00fce457f2af150eaab3c0532504cca2aebe599df1`,
source `e89901a1b`, packaging base `d65ddeb35`, SDK `688d5bfb`, native owner
`60e659644`. The new isolated fixture was empty and backed up before use.
Native create and write complete; independent host readback is exactly the
five bytes `hello` (SHA-256
`2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824`).
The native read result is `hello\n`, and arithmetic returns exactly `42`.
The absent-file response accurately says the file does not exist; the host
confirms its absence. The same VM serves all four prompts.

Formatting/status limits remain explicit: read-answer prose violates the
number/content-only presentation instruction; the accurate missing-file reply
is still marked Failed by the frontend. `observations.json` preserves exact
tool results, user-visible answers and statuses. These are functional native
checks, not computer-use acceptance. The separate collision repetition passes; live-revocation and interrupted-write
results are tracked below.

Forty-one completed older fixture disks were compressed losslessly. Each
`disk-archive.json` records the original byte hash and replacement gzip path;
the original is removed only after streamed decompression matches that hash.
Models and fixtures outside completed test evidence are untouched.


### Resealed collision and authority-update diagnosis

`canary-resealed-collision-01` passes in 339.53 s on the resealed `b8a6f1d8`
image. The real create tool reports the existing name; host bytes remain
exactly `hello`, and the follow-up answer is `42`.

`canary-resealed-authority-01` fails in 306.08 s. Its broker returns
`create_agent` with empty resources/executables for the live revoke request.
That is a syntactically valid creation proposal, but the guest's live transaction
correctly refuses it. No file revocation is confirmed, and the fixture remains
`authority fixture`. The harness cancels the test-owned worker after the terminal
update refusal. This is not the earlier bulk-upload receipt bug.

The broker system instructions explained creation without describing existing-agent
actions. The pending host correction explicitly describes `revoke_grant` and its
existing-agent/existing-grant selectors. The native test also now requests one
file-grant revocation, matching KBR1's one-action reply and its unchanged assertions
for Applied/resumed receipt, native Revoked observation, denied reads, preserved
bytes and same-VM arithmetic. It retains the read tool to witness the denial; it
does not assert that all tool grants were revoked. Twelve broker/agent host tests
pass, including exact revocation-reference and wrong-kind rejection checks.
`canary-resealed-authority-02` PASSES in 612.53 s with that saved host patch
and the same unchanged guest image. The guest publishes Applied after resume,
revokes the actual file object, and denies all five subsequent reads with an
empty resource catalog. Host bytes remain `authority fixture`; the next prompt
returns exactly `42` in the same QEMU PID 638. Initial setup switched from the
default project to the isolated fixture before the measured prompt sequence;
that setup restart is not presented as supervisor recovery. The broker initially
duplicated a tool selector; strict validation rejected it and accepted its
bounded correction. `observations.json` preserves the exact evidence. The initial host test command selected stable Rust and
failed on required nightly features; the pinned-nightly run is the passing result.

Hardware CI `35867475708` completed with seven failing hardware scenarios.
AMD VM additionally fails executable-acquisition timeouts (rungs 31328/31428)
and the required MicroPython startup witness. Intel VM passes four subscenarios
but its repeated-launch stress test never observes the final fresh prompt; the
last frame contains a truncated MicroPython banner. These join the previously
recorded AMD projection-link, window-lifecycle and early-readiness failures, and
both returning-main execution-witness failures. The initial 4k57 builder's
on-demand recovery passes. Detailed final classifications and raw vendor VM logs
are in `diagnostics/ci-d65ddeb35/`; these failures are not dismissed as CI noise.

Kore's pending projection diagnostic patch preserves the actual Link/validation
Open filesystem cause instead of collapsing it into failure 74. It changes no
timeouts or authority. The exec, client-wire and ABI host suites pass 383 tests
with one ignored, including exact outstanding-request recovery without replay.
All three architecture checks and the native x86 exec-service check also pass.
It is not part of the resealed guest image and needs its own source/export/build
validation before publication. No default lock promotion or merge.


### Resealed interrupted-write recovery confirmed

`canary-resealed-transactions-01` PASSES in 662.71 s on the same `b8a6f1d8`
candidate. At each fault, the host has committed the actual file creation and
withholds its original reply. The test first disconnects HostLink, then repeats
with supervisor termination. Generations 1–3 share QEMU PID 22865. Both original
effects remain, and the transaction journal is byte-identical across each
retirement: cleanup does not replay the operation. A fresh native create after
both faults succeeds. Independent host readback confirms all three files are
empty, including `recovery-ready.txt`. `observations.json` retains the boundaries,
source patch hash and host bytes. The run starts at host `1bf3ebf55` plus the saved
correction later committed as `e389274`; the guest/native/SDK pins are unchanged.

Kore diagnostic source `93fefba5f2ba668555a06c73616a627b88d68322` is now published
on PR #1015. Full `make test` passes: 178 Rust suites total 6,417 passed and
34 ignored, including the 1,616-pass kernel suite; build-tool checks also pass.
Architecture and native exec-service checks pass. CI `35878845333` has started
for the diagnostic, and a local disabled guest build is running. This is a
diagnostic source checkpoint, not a new coordinated SDK export or accepted
replacement for the resealed candidate. Its seven-bit failure reporting is
wire-compatible with the current client; final source/export/artifact convergence
still remains.


The corrected host's complete locked workspace test run passes 373 tests with
12 native tests ignored by that command (46 suites). Its three launcher binaries
(`vkore-harnessd`, `vkore`, `vkore-ui`) build successfully in release mode with the
locked dependency graph (36.79 s). Native gate results above come from separate
real guest/model executions. Logs are `broker-live-update-workspace.log` and
`broker-live-update-release.log` in `/tmp/vkore-supervisor-evidence/`.


### Projection-cause diagnostic native checks

Kore source/base `93fefba5f2ba668555a06c73616a627b88d68322` builds fresh
disabled ARM and x86 payloads in 843.82 s. This is a diagnostic checkpoint with
SDK `688d5bfb` and host native owner `60e659644`; it is not a new coordinated
export or an enabled candidate. Its generated payload patch SHA-256 is
`dd14c2a3e33367908a5c23ef0f76bcbea44c7605c0e47fc12042957a4fdd60d5`.
The production ISO SHA-256 is
`47de113048ed53ec5e8369e3128aaac9b925587b28982d18710873225291ce34`
(BLAKE3 `c4f58f32182438df97a89b87baead8cec6136d364316703b34c170b4b8c0c267`).
`projection-cause-default-01` PASSES the ordinary production bring-up in
50.10 s with agent supervision disabled, QEMU TCG, 4 CPUs and 4 GiB RAM.
The G7 topology-specific retirement witness remains explicitly not exercisable
on this run, as reported by the unchanged gate.

The separate kernel test image builds in 121.58 s with the exact captured
payload patch, SHA-256
`903d13197fddc80ebabff2239563e26ead67fed562f7769bb0bbb68610e9d6b4`.
`projection-cause-kernel-01` PASSES in 14.59 s. The build helper verifies and
saves the initial generated patch instead of silently accepting arbitrary
working-tree changes, and captures the actual test-specific ISO. These results
complete the local host/architecture/native service/kernel/production checks
for the reporting change. They do not explain or erase the earlier hardware
failures. CI `35878845333` has passed all architecture checks, supply-chain
checks, shared payload build and ARM/RISC-V application builds at the latest
snapshot; host and hardware work are still in progress.

The Mac remains locked at the last computer-use attempt. Final UI acceptance,
hardware failure diagnosis, final source/export convergence and default-lock
promotion are still open. Generated diagnostic payloads remain uncommitted;
the earlier enabled candidate and its source/diff/hash evidence are preserved.


### Locked-Mac validation and completed diagnostic CI — 2026-09-23

Computer use reports that automatic unlock is paused after local physical input.
This is not missing computer-use permission. Native Mac interaction remains
unavailable until a manual unlock; QEMU control, host checks and offscreen
rendering remain available.

The unchanged vKore runtime at `e389274` (documentation head `d5435f9`) passes
133 headless UI tests, with its one ignored capture/benchmark test run separately
and passing. The full reference comparison measures 60 samples with zero
differing pixels and zero maximum channel delta. Wide/narrow chat, narrow
settings and uncertain-authority-update fixtures render and were visually
inspected. These are fixture/application-state checks, not live macOS or guest
UI acceptance. The exact previously built workspace binary and its SHA-256
are recorded in `diagnostics/locked-mac-ui-renders/manifest.json`. An initial
isolated-package rebuild was stopped for disk pressure; only its recorded new
outputs and superseded UI test binaries were removed. Models, fixtures and
current binaries remain intact. Superseded diagnostic images and completed VM
disks were losslessly compressed with verified hashes and restore manifests.

Kore CI `35878845333` completed against merge `d2dce91cdb6a3cfe24bac0bef9c473ac0062f1da`:
host, architecture, supply-chain, application and image builds pass; 11 of 18
hardware test lanes pass. Seven fail: AMD window capacity/power controls, AMD
high-resolution display provisioning, both returning-main execution witnesses,
both vendor VM suites, and AMD status-devices startup. The reporting change
identifies actual projection timeouts (code 96), including incomplete app seeds.
Intel VM's saved status frame also proves a separate input focus race: partial
`vm` remains in Terminal 0 while Terminal 1 owns focus. Both returning-main
frames show focused Terminal 0, submitted `hello` and a fresh prompt without
execution testimony. Classification, raw logs and selected frames are retained
in `diagnostics/ci-93fefba5f/`; these are not blanket-classified as CI noise.

The source correction switches init's service waits to its existing observed
executor immediately after READY. Post-ready store/Lua/SPMD work previously
used the bootstrap executor, which could consume shell notices without
forwarding them. The command driver now shares the returning-main focus
barrier, witnessing all configured terminals and the selected seat before
input. Fifty-two focused driver tests, the real executor observer regression,
and init checks on x86/ARM64/RISC-V pass. No capability rights, wire layout,
observation budgets or command-output requirements were changed.

Fresh disabled production and interactive images were built from `93fefba5f`
plus the recorded source patch; they are diagnostic products, not a release
pin or coordinated SDK refresh. Production ISO SHA-256:
`fac1f3bf32055a7e2c6704cfb08413df8fa901659393fa8bd2b496530aa43c30`.
Interactive ISO SHA-256:
`77ab3868fac85e55833362eef017cb6566866197c4c45cd54e901d645da8dfb6`.
The complete tracked working-tree patch SHA-256 is
`eb84a7f086109808d0c916a648d6c94dc94e919013cea45254c790d908029b20`;
the newly added focus helper is saved separately with its own hash. The build
manifest includes all source-file hashes. The initial packaging attempt lacked
`KORE_ROOT`; the corrected helper supplies it and the expected commit. This
helper failure is not presented as a native build defect.

`post-ready-default-01` PASSES production boot in 42.32 s. The first new
returning-main run FAILS after 603.28 s at its provisioning prerequisite:
34/35 applications, no package, projection timeout 96. It never reaches the
command sequence and therefore does not validate the init correction. This
reproduces a hardware failure boundary locally. Projection retains the exact
uncertain operation for cleanup but publication fails after one observation
slice; compare its continuation with the existing bounded write-owner resumes
before changing a budget or claiming a root-cause fix.

`post-ready-focus-driver-02` PASSES in 61.35 s on the same interactive image:
35/35 seed, both seats ready, selected-seat focus, exact expected `help` rows
and a fresh prompt. The earlier focus pilot submitted `status` successfully
but used a token that scrolled offscreen; its failure and test-design limitation
remain recorded. All native tests ran serially with isolated VM disks.

`post-ready-returning-02` PASSES the full unchanged gate in 220.01 s:
20 hello rounds, full-width exit values, lifecycle/finalizer/worker variants,
utilities, status selectors and dynamically linked lspci. All 35 applications
seeded on this boot. The pass exercises report forwarding, but does not erase
the earlier incomplete-provisioning failure or establish hardware convergence.

The enabled candidate remains `b8a6f1d8` at artifact base `d65ddeb35`, SDK
`688d5bfb` and native owner `60e659644`. These newer diagnostic source changes
are not part of that tuple. Remaining work is unresolved provisioning and other
hardware failures, final source/export/artifact convergence and reproduction,
applicable native validation on that final tuple, and live macOS computer use.
No merge, issue closure, or default artifact-lock promotion.

The init/focus correction and execution ledger are committed as Kore `f3e2b45df`;
generated diagnostic payloads remain separate, captured build products.


### Projection timeout investigation — 2026-09-23

The installer incorrectly completed a seed with failure 96 after one expired
filesystem observation while the original projection request remained owned.
Init's outer retry can only resume a still-outstanding exec call, so it cannot
recover that completed negative verdict. The regression reproduces this exact
boundary on the old source. Bounded continuation of the original Open/Create/
Link now follows the existing write-owner policy: 250 ms slices, at most eight
continuations, unchanged terminal refusals and retained cleanup ownership.

Six new regressions and all 182 exec tests pass. Full host validation passes
6,423 Rust tests with 34 ignored; build-tool suites and all three architecture
checks pass. Native RISC-V exec checking and ARM/x86 payload builds pass.
Fresh disabled production, separate kernel boottest, full 20-round returning-
main/lifecycle/status and a second fresh-disk production boot pass. Every
production/interactive boot reports 35/35 apps, 3/3 libraries, 2/2 data, package
present and app failure 0. Tests run serially under QEMU TCG; G7 remains
explicitly not exercisable on this topology.

Hardware CI 35898242114 predates this projection patch and fails nine of its
18 hardware scenarios. AMD window and SMP logs report 27/35 and 33/35 app
seeds with code 96; returning-main reaches repeated launches but fails later
exit-value assertions. Neither that run nor the local repeat is relabeled green.

See [the investigation report](https://github.com/MicroPerceptron/kore/blob/c6dd5fe714183c71475e5fa4372dfde9a7747c1d/res/agent-supervisor/projection-timeout-investigation.md) for exact
hashes, source paths, red/green proof and native timings. Successful Link waits
for a durable metadata/root publication with two flushes; the captured timeout
does not identify which storage/scheduling/notification wait was late. Hardware
validation of the patch, final source/SDK/artifact convergence and native Mac UI
acceptance remain open. No merge or default-lock promotion.

Published Kore source: `c6dd5fe714183c71475e5fa4372dfde9a7747c1d`. The diagnostic images were built at
`f3e2b45df` plus the recorded patch; the earlier enabled candidate and default
locks remain unchanged. vKore runtime source remains `e389274`.
