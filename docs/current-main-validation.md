# Current-main candidate acceptance — 2026-09-21

PR #43 remains stacked on PR #41 (`506c205ffce90fb831402683c59f980a1568eddd`).
This candidate updates the native owners and real guest to Kore #1015's current-main
integration. Earlier UI results in [the stacked report](stacked-ui-validation.md)
belong to a different guest and are not acceptance of this pin.

## Exact provenance

- Kore main incorporated: `ba0067956d9678f22f44e84a8c2e4d02784d16b8`.
- Kore source / host native dependency: `3ca0edd4120903414aac7d6766bb7093a2a9a9eb`.
- Kore artifact checkout: `b80e341fa936b6fd93e93293b78afb5946039fb7`.
- Libkore export: `e2d7262c2ff4e43c607dc509a016390968e8d9fd`.
  The guest facade remains pinned to byte-identical `841e46e697dfc6b40a233d8427562dda8472ea39`,
  with native owners patched to its Kore checkout.
- Native export source: `3ca0edd4120903414aac7d6766bb7093a2a9a9eb`;
  all 414 exported files match final Kore source exactly.
- ISO: `kore-vkore-merge/target/kore-x86-release.iso`.
- ISO SHA-256: `b1a33ecdf4f764ea2a03c822e4f36e514471b8bfca19fd815a5dfef4ae795573`.

The lock separates source identity from the generated-artifact commit. Both
`KORE_GIT_HEAD` and `KORE_BUILD_STAMP` use the source identity; the reproducible
packager uses the artifact commit's timestamp and identity for ISO metadata.

## Validation and remaining gates

- vKore release workspace: 368 passed, 0 failed, 3 ignored. Actual release
  harness, CLI and UI executables built separately against this source.
- Cargo metadata: 19 unique native owners, all at the pinned HTTPS Git revision;
  no kernel dependency or local path substitution.
- Kore: 6,354 host tests passed, 34 ignored; all architecture checks including
  combined boot-test/userland features passed. Fresh payload/digest checks,
  separate production and test images, logged boot-test, production gate and
  two-terminal keyboard/display gate passed.
- Production and two-terminal guests seeded 35/35 applications, 3/3 libraries,
  2/2 data entries and passed package/Lua checks. Production SPMD passed with
  the reserved-core case explicitly unavailable on this QEMU TCG topology.
- Libkore full local matrix passed, including
  505 host tests, 39 Miri tests and actual executable-link/composition checks.
  Fresh remote checks are running on the new export.
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
regressions and the full host suite pass. Earlier real-stack headless verification
confirmed host writeback at source `79b40ede`; the refreshed run is pending.
Native UI acceptance remains separate.

The fresh UI fixture is under `~/.local/state/vkore-main-1015-corrected`; a
new headless fixture is under `~/.local/state/vkore-main-1015-native-isolation`.
It contains the isolated input fixtures with `test.txt` initially absent; the
previous headless state and evidence are retained separately.
The original user workspace and earlier failure evidence are preserved.

Repeat the four handoff prompts in one UI chat, verify exactly `b'hello'` on the
host, then test a new chat, draft preservation, physical-folder switching and
cancel/recovery. Judge missing-file text separately from its known failed chip;
record exact-output formatting honestly. Do not report headless results as UI
acceptance. Retain launcher logs before stopping the UI stack.

Evidence is retained under `~/.local/state/kore-1015-merge/20260921/` and the
isolated state directories. See [the handoff](ui-handoff.md) for launch guidance.

## Previous headless acceptance (source `79b40ede`)

These results used the earlier Kore artifact `bce68f0cb` and ISO SHA-256
`ed1ec4f3ed967a170a317c58590e7b2269260db5aba436c1c91c31daf335763f`;
they do not certify the refreshed image above.

At vKore `e41b32a`, session `cli-8151` used one real QEMU guest (PID 9879)
for all four prompts, after the initial focus-folder attachment restart.

| Prompt | Observed result | Whole turn |
| --- | --- | --- |
| Create test.txt containing hello | Completed; independent host bytes exactly `b'hello'` (5 bytes). | 332.08 s |
| Read exact contents | Tool read `hello`; answer added introductory prose, so strict exact-output formatting fails. | 115.09 s |
| Read missing-file.txt | Correctly reported that it does not exist; known failed prompt status. | 112.48 s |
| 17 plus 25, number only | Exactly `42`; zero tool calls, executables and resources. | 78.61 s |

The CLI exits 2 because of the expected missing-file failure status; this is not
an all-green harness run. It recovered to complete arithmetic in the same guest.
The initial attempt failed before tools because the manually copied fixture
lacked its transaction-state directory. Normal project creation supplies it;
the fixture was corrected and the failed evidence retained. No product change
was made for that setup error.

Final raw evidence: `~/.local/state/vkore-main-1015-headless/diagnostics/
run.20260922T001645Z.QKFRKj/`. The integration evidence directory contains
`headless-host-byte-proof.json`, `final-headless-semantic-results.json`, binary
hashes, exact pins and retained failure logs. These are headless results, not
computer-use proof of native UI behavior.

## Current runtime correction

The current source settles pending providers before privileged installation and
lets native file-handle I/O proceed without awaiting unrelated HostLink attach
or delta refresh. Host-backed handles retain refresh and authority checks.
Projection diagnostics distinguish cleanup/open/directory/link/close failures
without increasing timeouts. The longer editor-resize scenario and refreshed
four-prompt real-stack run are pending. Fresh hardware CI is
[run 35678116363](https://github.com/MicroPerceptron/kore/actions/runs/35678116363).
