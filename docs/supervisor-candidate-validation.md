# Optional supervisor acceptance — 2026-09-23

**Not merge-ready.** Native fresh-worker arithmetic and response retirement now
pass. Recovery passes three idle restarts, idle disconnect and disconnect
during real inference in the same VM. Termination after a published response and before receipt also passes.
Final artifacts, isolation/tool and UI acceptance remain pending. The default
real-stack lock has not been promoted.

## Current source refresh

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
Desktop checks remain pending for this freeze.

`sequence-files-01` failed before agent admission: the hand-prepared acceptance
project lacked its separate transaction-state directory. Normal `ProjectStore`
creation supplies it. The fixture was corrected, the gate now checks this
precondition before VM startup, and `sequence-files-02` is running. No file
operation pass is inferred from the failed setup.

`sequence-files-02` creates the file through a native tool and verifies exact
host bytes `hello`, then fails the read prompt. Personal discovery replaced the
focus folder with hashed read-only aliases, removing `/project/test.txt` from
the worker's view. The host now retains the selected focus root whenever a
search result belongs to it. External-only and virtual-project discovery stay
read-only. The focused discovery regression passes; native retest is pending.

The unaccepted candidate lock now selects disabled artifact base
`de6206941aad8e06616cd2b5290ebfb093118c7a`, source freeze `580a7c0ce`, and the
new enabled image SHA-256
`6a98af4aa9eff4005c08af874995145b33328b7ff9e04a3d107be9169f4608d3`.
Its exact generated payload diff SHA-256 is
`ed75af931c78f9408b44630e84cd09e71d2dde0f339dd087b6a0b73e8383cb3b`.
Reproduction from the restored committed payload baseline PASSES with the
exact ISO and generated diff. The release host/UI build and `make dev-check`
also pass using the dedicated UI state. `sequence-files-03` tests this image with the focus fix; the full vKore
workspace suite passes after that fix. Native uncertain-transaction fault
gates are implemented and compile, but have not yet run.

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
