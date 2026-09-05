# Real chat validation — 2026-09-05

Status: work in progress, not ready to merge.

The latest run used the actual `make dev-real` harness, QEMU/Kore, and
Qwen3.5-9B-Q4_K_M via llama.cpp. Six prompts were submitted in one session
through the terminal/API because the Mac was locked. No simulated guest,
deterministic inference fallback, or replacement model answers were used.

| Prompt | Observed result |
| --- | --- |
| Remember a phrase | Correct acknowledgement; answer event 37 preceded teardown 38 |
| Read, write, then read a project file | Actual cat/write/cat in order; accurate old/new contents; answer 123 preceded teardown 124 |
| Combined phrase and file recall | File values correct, remembered phrase incorrect despite correct history being supplied; semantic failure |
| Read a missing project file | Broker refused despite cat being available; BrokerSetup failure, no agent answer, then unconfirmed-cleanup shutdown |
| Arithmetic after that failure | Correct answer; answer 224 preceded teardown 225 |
| Precise first-message recall | Correct original phrase; answer 264 preceded teardown 265 |

All 266 event sequences were unique. The batch exited 2, correctly retaining
the failed turn; this is not a six-prompt acceptance pass. Host file contents
were independently checked. The trace is retained locally as
`session-ordered-live-events.jsonl`; private runtime state and raw logs are not
included in the repository.

## Fixes exercised

- Bounded session history survives prompt-scoped VM/agent retirement without
  retaining capabilities. Closing a session clears its history.
- Structured generation uses the model's native chat template and a visible
  schema, followed by strict validation and at most one live correction.
- Duplicate broker selectors are rejected explicitly. The companion Kore
  change allows 512 total broker output tokens for a plan and bounded retry.
- VM pause/resume follows actual pending backend requests, not the number of
  diagnostic prompts: a correction cannot leave the VM permanently paused.
- The CLI does not replay earlier answers, remains open after failed/rejected
  prompts, and retains a nonzero batch result when any prompt fails.
- Worker instructions require step order and evidence-based external claims.
  They do not guarantee semantic correctness or substitute host-written plans.

## Remaining work and verification limits

1. Improve model recall reliability without hiding incorrect answers.
2. Diagnose unnecessary broker refusals. Refusal currently yields no
   agent-authored response and subsequently expects reclamation that is not
   confirmed. Fix that failure path and its user-visible outcome.
3. Perform visual native-UI acceptance after unlocking the Mac; draft
   preservation and UI regression tests are not visual acceptance.
4. Publish/review canonical artifacts and pins for the companion Kore commit.
   The tested local image was built from PR #909 commit
   `fd383fb8705e63400e7b78762185f3208ee355c2` plus tracked patch SHA-256
   `70a32975fe1dbef9614d317ef9eb7febfc1c1fad5e15c1f8aaebbf839b77f39b`.
   ISO SHA-256:
   `36f49b9f4f84ab05c22c31f3ab7b080e66682ffd842bf9200d81064a2f936e71`.
   Committing that source does not retroactively rebuild the ISO with the new
   commit's packaging metadata. Upstream artifact pins remain unchanged.

The recorded vKore workspace tests and local `make dev-check` pass. Kore's
production boot gate, kernel boot test, and three architecture checks passed.
The broad Kore host-test target is not green: it has an existing
`AgentToolDescriptor: Serialize` compilation failure in
`tools/hostlinkd/src/llama.rs`.

Current bounds: prompts are limited to 128 UTF-8 bytes, and session memory to
16 completed exchanges / 16 KiB. Memory is not restored across harness
restarts. Disabling automatic writeback blocks host writes; a complete
interactive review/apply workflow is not implemented.
