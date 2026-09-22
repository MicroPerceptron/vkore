# Current-main candidate acceptance — 2026-09-21

PR #43 remains stacked on PR #41 (`506c205ffce90fb831402683c59f980a1568eddd`).
This candidate updates the native owners and real guest to Kore #1015's current-main
integration. Earlier UI results in [the stacked report](stacked-ui-validation.md)
belong to a different guest and are not acceptance of this pin.

## Exact provenance

- Kore main incorporated: `ba0067956d9678f22f44e84a8c2e4d02784d16b8`.
  Its final update changes CI only; validated product bytes are unchanged.
  CI tooling ran 348 tests successfully (6 skipped).
- Kore source / host native dependency: `79b40ede71a37984d46f0e16fb9208be1c21b116`.
- Kore artifact checkout: `bce68f0cb221b9cc278fe10a345645d4f96145df`.
- Libkore: `841e46e697dfc6b40a233d8427562dda8472ea39`.
- Native export source: `cce0f31670773eab781bf542b783c37f67a6921f`;
  all 414 exported files match final Kore source exactly.
- ISO: `kore-vkore-merge/target/kore-x86-release.iso`.
- ISO SHA-256: `ed1ec4f3ed967a170a317c58590e7b2269260db5aba436c1c91c31daf335763f`.

The lock separates source identity from the generated-artifact commit. Both
`KORE_GIT_HEAD` and `KORE_BUILD_STAMP` use the source identity; the reproducible
packager uses the artifact commit's timestamp and identity for ISO metadata.

## Validation and remaining gates

- vKore release workspace: 368 passed, 0 failed, 3 ignored. Actual release
  harness, CLI and UI executables built separately against this source.
- Cargo metadata: 19 unique native owners, all at the pinned HTTPS Git revision;
  no kernel dependency or local path substitution.
- Kore: 6,353 host tests passed, 34 ignored; all architecture checks including
  combined boot-test/userland features passed. Fresh payload/digest checks,
  separate production and test images, logged boot-test, production gate and
  two-terminal keyboard/display gate passed.
- Production and two-terminal guests seeded 35/35 applications, 3/3 libraries,
  2/2 data entries and passed package/Lua checks. Production SPMD passed with
  the reserved-core case explicitly unavailable on this QEMU TCG topology.
- Libkore full local matrix and all seven remote checks passed, including
  505 host tests, 39 Miri tests and actual executable-link/composition checks.
- Canonical ISO: `make artifacts` reproduced the exact digest with unchanged
  payload bytes; the runtime-only production gate verified that SHA and passed.
- Fresh native UI acceptance: pending because computer use reports a locked Mac.
- Remote Kore CI and approving review remain required before merge.

These local guest runs use Intel profile, QEMU TCG and explicit 4k48 paging.
They do not certify hardware virtualization, physical hardware, 4k57 or Linux.
Earlier two-terminal runs had an initialization timeout and a 33/35 application
resize failure. Later diagnostic and clean-source runs passed; the earlier
failures remain retained without claiming a separate root-cause fix.

## Host-writeback regression

Testing the earlier current-main source `7c9f734` found a real defect: UI create
reported success and guest readback returned `hello`, while the host file was
empty. DagFS atomic replacement bypassed HostLink's older mutation interception.
Kore now commits the existing staged replacement through the host transaction
and requires acknowledgment before guest publication. Refusal retains staging
for abort/close; uncertain receipts retain reconciliation custody. Seven new
regressions and the full host suite pass, but they do not replace fresh end-to-end
host-byte verification or native UI acceptance.

The fresh UI fixture is under `~/.local/state/vkore-main-1015-corrected`; a
separate headless fixture is under `~/.local/state/vkore-main-1015-headless`.
Both were copied and backed up before testing, with `test.txt` initially absent.
The original user workspace and earlier failure evidence are preserved.

Repeat the four handoff prompts in one UI chat, verify exactly `b'hello'` on the
host, then test a new chat, draft preservation, physical-folder switching and
cancel/recovery. Judge missing-file text separately from its known failed chip;
record exact-output formatting honestly. Do not report headless results as UI
acceptance. Retain launcher logs before stopping the UI stack.

Evidence is retained under `~/.local/state/kore-1015-merge/20260921/` and the
isolated state directories. See [the handoff](ui-handoff.md) for launch guidance.
