# Native UI computer-use test — 2026-09-20

## Fix and live retest

**The HTTP decoding bug is fixed.** All four prompts completed and displayed their answers in one native UI session after rebuilding. The read-back answer had an extra explanatory prefix, so strict exact-output formatting was not a pass; this did not affect UI transport or the file contents.

`crates/vkore-ui/src/main.rs` now decodes HTTP chunk framing at the byte level before JSON/SSE parsing. This preserves events and UTF-8 characters split across transport chunks. Chunk extensions and trailers are consumed, malformed/truncated framing is rejected, unsupported transfer encodings produce an explicit error, and chunked HTTP error bodies are decoded too. No dependency or pin changes were needed.

Validation:

- A new regression test failed against the old reader, then passed with the fix.
- Full native UI suite: **120 passed, 0 failed, 1 existing ignored test** using the repository's nightly toolchain (`PATH="$HOME/.cargo/bin:$PATH" cargo --config target/current-kore.toml test --release -p vkore-ui`).
- `rustfmt --check` and `git diff --check` passed.
- Real stack rebuilt with `make dev-real VKORE_REAL_STACK_LOCK=config/real-stack.lock.mk`; same verified Kore ISO and model as the baseline.
- Each prompt was typed and submitted through computer use, and every answer plus completion was inspected in the native UI. No prompts were submitted through the API or CLI.

| Prompt | Live UI outcome | Elapsed |
| --- | --- | --- |
| Create `test.txt` containing `hello` | Answer displayed; host file verified as exactly `b'hello'` after starting with the file absent. | 144.38 s |
| Read `test.txt` | Displayed `The exact contents of test.txt are: hello`; correct contents, with extra prose. | 55.60 s |
| Read `missing-file.txt` | Displayed that the file does not exist in `/project`. | 58.36 s |
| 17 plus 25, number only | Displayed exactly `42`; broker budget `tool_calls: 0`, `executables: []`, `resources: []`. | 37.95 s |

All four backend terminal statuses were `completed`. The composer returned to Send after each request; drafts entered during earlier requests survived completion. No `Event stream interrupted` error recurred. The fixture is byte-for-byte unchanged. The UI's top status line can retain its previous agent-working text after completion even though the transcript shows Worked and Send is available; that separate presentation issue is outside this transport fix.

Retest evidence: `~/.local/state/vkore-current-kore-integration/diagnostics/ui-computer-use-fixed-20260920/`, containing `events.jsonl`, harness/guest logs, failing regression log, passing suite log, and release launch log.

Retest session: `ui-current-kore-integration-1789957043233`. The original `test.txt` and fixture backups remain under `/tmp/vkore-ui-test.t1fzDe/`; an additional pre-retest backup is under `retest/`. Only the regenerated `test.txt` remains as a workspace effect.

## Original reproduction

Original baseline result: **blocked by native UI HTTP decoding bug on prompt 1**. The backend successfully created the requested file, but the UI never displayed its answer or completion. Prompts 2–4 were not submitted; this is not a four-prompt pass.

## Stack and launch

- vKore: `ee60bba9de5604b046bffc3a3fe2f7416ca6e561` (handoff documentation commit over `b2e7bbc`).
- Kore: `74ae24c87323dd7bc7ac0a12770c25ec8c8c4852`.
- ISO SHA-256: `bfee034fa6312b3aab799e2b79031f8fe2da57d32028603541f2ee510cb4afac`.
- Model: Qwen3.5-9B-Q4_K_M.gguf, live llama.cpp and QEMU; no simulated backend.
- About 4 GiB free at start. No pre-existing live harness, UI, QEMU, or llama-server.
- `make dev-check` failed because ignored `Makefile.local` selected stale `target/current-kore.lock.mk` (expected `bc3a2ff…`). The committed lock matches the handoff and actual ISO.
- Verified and launched with `make dev-real VKORE_REAL_STACK_LOCK=config/real-stack.lock.mk`; no pin files changed.
- Computer-use app discovery could not select the bare executable. A temporary `.app` wrapper symlinked the exact release binary and connected to the existing harness at `127.0.0.1:8798`. The original UI was idle; only the wrapper UI submitted a prompt. There was only one backend/guest/model stack.
- Workspace and settings backup: `/tmp/vkore-ui-test.t1fzDe/`. The prior `test.txt` was moved out before guest startup so creation could be verified independently.

## Test results

| Test | Result |
| --- | --- |
| Create a file named test.txt containing the word hello. | Backend passed; UI failed to display completion. Host `test.txt` is exactly five bytes, `hello`. |
| Read test.txt and reply with its exact contents. | Blocked by first prompt's stuck UI. |
| Read missing-file.txt and reply with its contents. | Blocked; known refusal-status behavior not evaluated. |
| What is 17 plus 25? Answer with the number only. | Blocked; zero-capability grant not evaluated. |
| Draft preservation | Passed across Session → Agents → Session → Filesystem → Session and Settings open/Escape while work ran. Draft remained unsent. |
| Basic layout and navigation | Session, Agents, Filesystem, and Settings rendered and accepted screenshot-based interaction at the observed window size. No exhaustive responsive-layout check. |
| Accessibility | Only native window frame/menu elements exposed; application content and composer absent from accessibility tree. Coordinate interaction required. |
| Multi-chat / workspace picker | Not exercised. New project was disabled during the open session. |

The fixture `integration-input.txt` is byte-for-byte unchanged from backup.

## Blocking failure

The UI showed:

```
Event stream interrupted: EOF while parsing a string at line 1 column 6563
```

It retained `Working…` and a Cancel button after the backend completed. The user prompt was submitted at 21:07:45 CDT and backend terminal completion arrived about 137 seconds later. This is independent of the handoff's known model recall, append/write selection, and refused-status issues.

Root cause: `crates/vkore-ui/src/main.rs` function `http` reads raw response bytes and passes the body directly to `parse_sse`. It does not decode `Transfer-Encoding: chunked`. `parse_sse` then treats HTTP chunk boundaries as event lines. One split JSON event fails the whole batch; polling retries without advancing the cursor, so the completed prompt remains stuck.

The captured response has `Transfer-Encoding: chunked` and 33 HTTP chunks, up to 8192 bytes each. Removing HTTP framing yields 174 valid SSE events, including the assistant answer and `prompt_terminal` with `status: completed`. The original UI error was observed directly in computer-use screenshots before and after backend completion.

## Evidence

Retained at `~/.local/state/vkore-current-kore-integration/diagnostics/ui-computer-use-20260920/`:

- `events-http-response.bin`: raw response reproducing chunk framing.
- `events-decoded.jsonl`: independently decoded valid SSE events.
- `events.jsonl`: retained backend transcript.
- `harness.log`, `kore-serial.log`, and `kore-debug.log`.

Session: `ui-current-kore-integration-1789956444292`; prompt: `p-1789956465725`.

Broker proposal: create-agent budget 3, executables `[11,9]`, resources `[0]`. Tool results include file creation and `fs: wrote 5 bytes`. The assistant reported successful creation; completion is sequence 175.

No application source changes were made during this test. The next validation step is to fix HTTP transfer decoding, add coverage for an event split across chunks, and rerun all four prompts through the native UI.

The temporary test UI and original launcher were shut down after evidence capture. The created `test.txt` remains in the workspace; the original copy remains in the backup.
