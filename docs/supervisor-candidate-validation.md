# Optional supervisor candidate — 2026-09-22

Status: source implementation, host checks, artifact reproduction and preflight
passed. **Native and computer-use acceptance have not run. Do not merge or
promote the default lock on this evidence alone.**

| Identity | Revision or digest |
| --- | --- |
| Kore source | `ac349b75dc23f7d20f35c78b923c14a691133cb4` |
| Committed disabled artifact base | `c2b45da4c4cd8ad98245e1207d069fc5d73d2628` |
| SDK | `1f7e248b070ef3a04bd1226c1cef8a46b46e1f2c` |
| SDK and host native owner source | `d0905ff6aa86487b7f6ca051277a693558c98175` |
| Enabled ISO SHA-256 | `df4c0e29b94e7629722ec42ecf7e34989eba94f0fcadeb16ab71208aa25386cc` |
| Enabled ISO BLAKE3 | `3b4148268290ef22d809fb3ea891c371be708f8de6c4b61dcd3510235df42c27` |
| Generated Kore diff SHA-256 | `71f3f93b587fbe81470c84ba4c2b50d7a3405334c6064dde31ab6d880626d273` |

The final vKore implementation includes this document's commit and its
ancestors. `config/supervisor-candidate.lock.mk` selects the tuple above.
`config/real-stack.lock.mk` is still the previous baseline. Both SDK
lockfiles and the vKore dependency lock remain unchanged by packaging.

## Observed checks

- Kore full host suite passed after the final policy/private-stop edits.
  Enabled-selector focused tests passed; x86, ARM and RISC-V checks passed.
  Disabled production and separate release boot-test images built; neither
  has been booted for this overhaul.
- SDK `make fmt test check docs owners cross miri bindings-check`, exact export,
  actual-Kore composition, renamed consumer and mixed-owner negative controls
  passed against the published native revision.
- vKore's Git-pinned full workspace suite passed, then passed again after the
  private ISO snapshot fix. Focused VM/daemon regressions and release builds
  passed. The real native restart test is compiled but remains unexecuted.
- Starting from clean tracked Kore source at the disabled artifact base,
  `make artifacts VKORE_REAL_STACK_LOCK=config/supervisor-candidate.lock.mk`
  reproduced both the ISO and all five generated files byte-for-byte. Compiler
  caches were reused; this was not a cold toolchain installation.
- `make dev-check` passed against a dedicated state and fixture. It verified
  the source, generated diff, ISO, real model/server and project settings.
  It did not start a harness or VM.

Local evidence is retained under Kore's `target/agent-supervisor-evidence/`
and `~/.local/state/vkore-supervisor-acceptance-20260922/diagnostics/`.
The latter contains candidate and host-gate manifests, preserved logs and an
isolated native restart configuration. The fixture began empty and was backed
up before testing. Failed earlier build attempts remain recorded separately.
Current remote CI must be assessed on its exact head; old passes do not apply.

## Running-session image incident

The earlier manual QEMU process mounted the shared build-output ISO directly.
During this implementation, image builds overwrote that inode even though the
process and workspace were left running. Its original exact ISO backup was
not found. This session is not valid acceptance evidence and must be restarted
before further testing; lack of visible failure does not prove it was unaffected.
The open inode was separated from subsequent build output. No manual process
was stopped or fixture changed to conceal this incident.

New `QemuProcess` instances now own a private ISO copy until retirement.
A regression test overwrites and removes the source image while the process
is alive and verifies that its mounted copy remains unchanged and is cleaned
up after process retirement. This fixes subsequent launches; it does not
retroactively repair the old session or certify native boot behavior.

## Remaining acceptance

Follow [the coordinated plan](https://github.com/MicroPerceptron/kore/issues/1053)
and [run instructions](supervisor-acceptance.md). Required gates still include
ordinary disabled boot, explicit enabled startup, missing authority, isolation,
live authority changes, faults at each lifecycle boundary, repeated retirement
and capacity reuse, two-terminal/extended desktop behavior, and real UI
create/read/missing-file/arithmetic, host-byte verification, recovery, drafts
and project/chat selection. Only one QEMU owner may run at a time. Approval to
replace the user's existing manual session is still pending.
