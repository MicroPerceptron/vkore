# Current-main candidate acceptance — 2026-09-22

PR #43 remains stacked on PR #41 (`506c205ffce90fb831402683c59f980a1568eddd`).
Earlier results in [the stacked report](stacked-ui-validation.md) and the
historical headless section below do not certify this candidate.

## Exact provenance

- Kore main incorporated: `ba0067956d9678f22f44e84a8c2e4d02784d16b8`.
- Kore source / 19 host native owners: `68dda5a411d4d1ba0d015b5f341e2b5bfa0a5daf`.
- Generated artifact checkout: `ac9d0dc9f4aca5213ce5c0d6c494103782508f7e`.
- Libkore export: `f9ab51fa803c6fc056f88d09a0fdbde86fc59d28`, exactly 414 files
  in 31 packages from that source. The guest facade remains pinned to
  `841e46e697dfc6b40a233d8427562dda8472ea39`, with native owners patched to Kore.
- ISO: `kore-vkore-merge/target/kore-x86-release.iso`.
- Packaged ISO SHA-256: `4fb3f53c3d511c0b14fe8570bd525e3b2c5b5aff328a4d2e7dcea819e4ed0a51`.

The lock separates source identity from the generated-artifact commit.
`KORE_GIT_HEAD` and `KORE_BUILD_STAMP` use the source identity; ISO metadata uses
the artifact commit. Packaging preserved every member's bytes, and `make artifacts`
reproduced the exact ISO digest. Its SHA-verified production gate passed.

## Current checks and limitations

- Kore's complete local matrix passed: 6,359 host tests (34 ignored), payloads,
  architecture checks, separate production/test images, logged boot test,
  production provisioning and two-terminal keyboard/display gate. Production
  guests seeded 35/35 apps, 3/3 libraries and 2/2 data members.
- SDK matrix and actual-Kore local/Git composition passed: 505 host tests,
  seven tooling tests, 39 Miri tests, all target/link/binding checks and exact
  export verification. SDK validation is not guest or native UI acceptance.
- vKore's refreshed release workspace passed 368 tests (three ignored), the
  CLI/harness/UI executable builds and dev-check. Cargo metadata resolves 19
  unique Kore owners at the pinned
  HTTPS Git revision, without local substitutions or a kernel dependency.
- The full desktop guest test failed to reopen Power on recycle round eight.
  A diagnostic repeated the missing reopen on round three with zero pointer-ring
  losses, rejected reports or resets; Terminal still opened and Power remained
  cached. This remains unresolved.
- [Source hardware run 35690674760](https://github.com/MicroPerceptron/kore/actions/runs/35690674760)
  completed with 15 of 22 final scenario verdicts passing. Both boot tests passed;
  Intel production/display/window/VM status/lifecycle/Python lanes passed.
  AMD production seeded 34/35 apps (projection directory code 73); some AMD
  jobs failed during startup, others on missing injected input or editor content.
  Intel returning-main did not reach its description witness. Logs are retained;
  no missing or failed assertion is counted as a pass.
- Native vKore computer-use acceptance is pending because the Mac is locked.
  An approving review is still required before merge. No merge has been performed.

Local guests use Intel QEMU TCG with 4k48 paging. They do not certify physical
hardware, Linux, or other paging modes. The current main hardware control was
unavailable; do not label all remote failures branch regressions or CI defects.

## Four-prompt real-stack check

A fresh isolated fixture and backup are under
`~/.local/state/vkore-main-1015-nvme-optimal`; test.txt starts absent.
The refreshed headless run completed in session `cli-20280` on the exact packaged
ISO above. Create produced exactly five host bytes `hello` (independently read).
Read returned tool text `hello\n`; the assistant added introductory prose, so
strict exact-output formatting failed. Missing-file handling correctly said the
file does not exist, without issuing a read call; its harness status was failed.
Arithmetic returned exactly `42`, with zero tools, executables and resources.
Whole-turn times were 282.193s, 99.955s, 88.404s and 77.493s respectively.
The CLI exited 2 for the missing-file status and recovered to complete arithmetic
in the same guest. These are semantic results with explicit formatting/status
limits, not an all-green harness or native UI result.

Raw evidence: `~/.local/state/vkore-main-1015-nvme-optimal/diagnostics/
run.20260922T061818Z.5qqQwc/`. Independent host-byte proof and parsed plans/tool
results are retained alongside the integration logs.

UI checks also require visible replies, multi-chat, unsent drafts across panels,
Settings and project navigation, workspace isolation and cancellation recovery.
Do not report headless outcomes as native UI acceptance. Evidence remains under
`~/.local/state/kore-1015-merge/20260921/` and isolated diagnostics directories.
See [the handoff](ui-handoff.md) for launch instructions and compatibility limits.

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
