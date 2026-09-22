# PR #43 on PR #41: native UI validation — 2026-09-21

This report describes the earlier `74ae24c` guest. The current-main candidate
and its separate acceptance status are recorded in
[current-main validation](current-main-validation.md).

The four original prompts were exercised through the native UI in one chat.
This is **partial acceptance**, not an all-green multi-chat result: strict
readback formatting, the system folder picker, and a subsequent new-chat file
request did not pass. No guest source or ISO was changed.

## Integration

PR #43 incorporates PR #41 at `506c205ffce90fb831402683c59f980a1568eddd` with a
merge, preserving both histories. It retains the current guest pin and newer
HostLink native retirement/cancellation/authority-update protocol, adds #41's
application settings/runtime, project-chat navigation and model timing, and
keeps the native byte-level HTTP chunk decoding fix.

Closing a completed chat also exposed a native UI panic: the handler cleared
selectable-text caches while retaining transcript events. The fix synchronizes
those derived views and preserves the readable closed transcript. A regression
building the real closed-chat view reproduces the panic before the fix and
passes afterward.

Permission-update state now travels with each cached chat. An acknowledgement
received after navigation updates that chat's draft/submission, rather than the
currently selected chat. The regression test covers lost replies, retry identity,
late acknowledgement, and returning to the originating chat.

The first combined live create failed because host-only root replacement left
the current guest without usable filesystem offers. The current guest keeps
its native imported root and write grant; #41's older headless guest supported
a different attachment lifecycle. The host now reuses unchanged focus-folder
attachments, and confirms termination of the exact old VM before changing
attachments and booting its successor. A disconnected-adapter gate rejects
queued work and live transport epochs. Resetting HostLink can also restart the
model server. This is a deliberate compatibility limit: cross-workspace process
identity from the older #41 acceptance run is **not** preserved or claimed.

## Reproducibility

- Kore: `74ae24c87323dd7bc7ac0a12770c25ec8c8c4852`.
- ISO: `kore-vkore-port/target/kore-x86-release.iso`.
- SHA-256: `bfee034fa6312b3aab799e2b79031f8fe2da57d32028603541f2ee510cb4afac`.
- Qwen3.5-9B-Q4_K_M, real llama.cpp, real QEMU TCG.
- Launch: `make dev-real VKORE_REAL_STACK_LOCK=config/real-stack.lock.mk VKORE_STATE=/Users/colinhayes/.local/state/vkore-stacked-43-41`.
- A temporary `local.vkore.ui-test` app wrapper points to the exact release UI
  binary, connected to port 8798. One launcher/backend/guest/model; the launcher's
  original UI was idle and only the wrapper submitted prompts.
- Original workspace/settings preserved. Test state contains a separate fixture
  copy; `test.txt` was absent before creation.

## Four prompts in one session

Session: `ui-current-kore-integration-1790010967386`.
All prompt entry, submission, answer inspection and navigation used computer use.

| Prompt | Observed UI and independent result | Whole turn |
| --- | --- | --- |
| Create a file named test.txt containing the word hello. | UI displayed successful creation; host bytes independently equal `b'hello'`. | 154.75 s |
| Read test.txt and reply with its exact contents. | `The exact contents of test.txt are: hello`. Contents correct; strict exact-output formatting did not pass. | 57.70 s |
| Read missing-file.txt and reply with its contents. | Expanded activity says it does not exist and cannot be read. Known failed status despite a correct refusal. | 47.17 s |
| What is 17 plus 25? Answer with the number only. | Exactly `42`; broker `executables: []`, `resources: []`, `tool_calls: 0`. | 39.72 s |

QEMU PID 66807 and model server PID 66796 handled the four-prompt sequence.
The UI consumed chunked events through terminal completion; no truncated-JSON
or stuck event-stream error recurred. The original `integration-input.txt`
fixture matches the isolated copy byte-for-byte.

## Additional native checks and limits

- Draft survived Session → Agents → Filesystem → Session and Settings/Escape
  during work. Both projects retained separate unsent drafts across navigation.
- Created a virtual project through New project while the first task ran.
  Returning to A replayed the completed create answer and restored its draft.
- New chat in A cleared the old transcript and opened a new session. The next
  file read failed after 27.87 seconds with **`Agent admission failed.`**,
  after a valid broker plan selecting read + project view. This was the fifth
  prompt on that guest. The cause is not established by this test, and this is
  not classified as a model formatting failure or a passing multi-chat check.
  Retained session: `ui-current-kore-integration-1790011392930`.
- A queued arithmetic prompt in virtual project B completed with exactly `13`
  after 94.90 seconds of runtime work (plus its wait behind A). Its worker input
  had no resources or tools; captured model prompts contained no A conversation
  text. The replacement processes
  were QEMU 74446 / model server 74435. This verifies one cross-project task
  transition, not two physical-folder switching or unchanged process identity.
- After the close-chat fix, a fresh real UI conversation displayed `13` again.
  Clicking Close chat left **No chat open** in the subtitle, kept the transcript
  readable, and left the app alive. That exact interaction crashed before the fix.
  Retest session: `ui-task-18d765a8095614e8-ee2a8b6c26e4-1790011726251`.
- The folder picker entered waiting state, but no chooser was accessible on
  available computer-use surfaces. The UI eventually showed `Resource temporarily
  unavailable (os error 35)`. The owned chooser subprocess was terminated during
  recovery. The folder-picker path is **not verified**; virtual-project creation
  used its separate button.
- Native AX exposes the window frame, not Iced content. Screenshot-based
  coordinate interaction was required. Layout was checked at 1182×768 only.
- Live permission editing, document discovery, long-session stability, and Linux
  execution were not covered by this native run. Automated regression coverage
  is not a substitute for those live checks.

## Automated checks and evidence

- Release workspace before the final close-chat fix: **367 passed, 0 failed,
  3 ignored**. After that UI-only fix, the full native UI suite passed **133
  tests, 0 failed, 1 ignored**, including the new rendering regression.
- Python launcher/timing/resource tests: **11 passed**.
- Changed Rust files pass rustfmt; merge marker and whitespace checks pass.

Evidence is retained under
`~/.local/state/vkore-stacked-43-41/diagnostics/stacked-native-20260921/`:
transcripts, broker/tool events, host verification, harness/guest logs, build and
suite logs, plus process observations. The initial failing root-attachment run
is separately preserved under `diagnostics/initial-attachment-failure/`.
See [the handoff](ui-handoff.md) for future launch and storage guidance.

## Storage cleanup

The user's Claude session working-directory records and process inventory were
used to identify unused build caches. `cargo clean` removed about 2.8 GiB from
`kore-vkore-control/user/target` and 2.0 GiB from
`kore-vkore-real/user/target`. Neither had a running build/VM using it. Dirty
source worktrees, models, current guest artifacts and evidence were preserved.
Free disk was about 7.7 GiB after rebuilding, versus 733 MiB before cleanup.
