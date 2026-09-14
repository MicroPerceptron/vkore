# Real chat validation — 2026-09-14

PR #39 cleanup is ready for review. Issue #24 owns the subsequent application-wide runtime and task filesystem changes.

## Live acceptance

The native host harness used real QEMU/Kore and Qwen3.5-9B-Q4_K_M through
llama.cpp. Six prompts ran sequentially in one conversation. All six completed
with model-authored answers; no host-authored substitute answer was used.

| Prompt | Observed result | Answer time |
| --- | --- | --- |
| Remember `silver otter` | Correct acknowledgement | 63.034 s |
| Read/write/read `acceptance.txt` | Actual cat/write/cat; `cedar-42` → `maple-73` | 126.816 s |
| Combined phrase and file recall | Correct phrase and both values | 50.261 s |
| Read a missing file | Real cat attempt; honest filesystem-authority refusal | 52.620 s |
| Arithmetic after the refused read | 17 × 23 = 391 | 44.333 s |
| Recall the first phrase | Exact `silver otter` | 45.319 s |

All 669 event sequences were unique. Every assistant answer preceded the
guest's confirmed worker teardown and the prompt terminal event. The batch
exited zero. Host file contents were independently checked. The multi-step
request used one bounded broker correction and one bounded worker correction;
invalid candidates were recorded and rejected before execution.

These timings were collected during local build/validation activity and are
not a controlled performance benchmark. The initial model load took 17.735 s.
This PR still retires single-prompt guests after completion; VM/server reuse,
prewarming, and global broker ownership are issue #24. See
[performance timing](performance-timing.md) for the measurement boundaries.

## Validation

- Full vKore workspace suite: 292 tests passed, 3 ignored. Subsequent native UI
  suite: 114 passed, 1 ignored, including the new metadata/worker regression.
- Python timing/process tests: 5 passed. Real-only launcher tests: 2 passed.
- Native release build and `make dev-check` passed. The Linux setup helper
  creates the workspace state directory and refuses to overwrite existing setup.
- Native Mac UI: settings layout inspected, rejected over-limit draft remained
  editable, and the agent view stayed empty before a real worker existed.
  Rendering tests cover narrow/wide layouts; an offscreen capture also passed.
- Kore host suites: 2,875 passed, 13 ignored. The previously documented hostlinkd
  tool-descriptor serialization compilation error is fixed.
- x86, AArch64, and RISC-V checks passed. Production and kernel-test builds
  passed; the kernel boot test passed.
- The production gate passed with the actual vKore TCG configuration (2 CPUs,
  2 GiB RAM), including all 17 services, preemption, 27/27 stored applications,
  and tree read-back. The 4-CPU/4-GiB run timed out before store seeding; that
  configuration is not reported as passing.

## Guest provenance

[Kore PR #933](https://github.com/MicroPerceptron/kore/pull/933), stacked on
Kore PR #909, contains the matching guest changes. The packaged commit is
`2b1307786a0a354864a14b98578f73e413cae208`; its tracked userland artifacts were
rebuilt from source `2db155cfb9ac6621bfdd723966157c22022fae4d` and carry that
shell build stamp. Packaging derives ISO metadata from the packaged commit.
The tracked Kore tree is clean.

Download the exact image from
[vkore-pr39-20260914](https://github.com/MicroPerceptron/kore/releases/tag/vkore-pr39-20260914).
ISO SHA-256:

```text
7dec906af7a1ddffb6a5fdd3e3b4dbde37ac9951bd3d0ebf8286c4b50526fdfd
```

The launcher defaults, `config/real-stack.lock.mk`, and
[Linux setup guide](linux-setup.md) agree on this artifact. GitHub's asset digest
matches the locally verified image. Linux end-to-end execution remains unverified.

## Explicit bounds

Prompts remain limited to 128 UTF-8 bytes. Session memory retains at most
16 completed exchanges / 16 KiB and is not restored after a harness restart.
A small local model can still make semantic mistakes; this passing sequence
does not guarantee arbitrary-task correctness. Invalid structured actions
receive at most one bounded model correction and are never executed unchecked.
Disabling automatic writeback blocks host writes; an interactive review/apply
workflow is outside this PR. The settings UI states this limitation.

The September 5 failures and original September 14 timing results remain in
local diagnostic evidence; this report describes the new acceptance run, not
a reinterpretation of those earlier failures. Raw runtime logs and private
workspace contents are not committed.
