# Current-Kore native UI handoff

PR #43 incorporates PR #41's application runtime and chat UI. See
[stacked computer-use validation](stacked-ui-validation.md) for the September 21
results and compatibility limits. The earlier HTTP failure and fix remain in
[the September 20 report](ui-computer-use-test.md).

## Supervisor overhaul candidate (in progress)

The implementation now expects the separately founded supervisor contract v1.
The revision/hash table below still describes the earlier baseline;
**new candidate pins and native/UI acceptance have not been promoted yet**.
Do not treat those baseline hashes as validation of the overhaul.

The existing `config/supervisor-candidate.lock.mk` is a superseded checkpoint
and intentionally fails current-worktree preflight. New source and image pins
must be sealed after the remaining native acceptance gates pass. Do not use
that old lock to launch this source. See [current validation](supervisor-candidate-validation.md)
for tested revisions and the unresolved gates. The default lock is not promoted.

Ordinary Kore builds default to `KORE_AGENT_SUPERVISOR=0`. A vKore candidate
must rebuild its kernel and user payloads with `KORE_AGENT_SUPERVISOR=1` and
`KORE_AGENT_SUPERVISOR_PROJECT=1`. The launcher verifies the selected image hash;
a connected transport or an older broker poll is not supervisor readiness.

Supervisor startup, death, timeout and unconfirmed cleanup retain the VM and
request containment of the exact supervisor generation. Retry observes that
cleanup and its replacement; it does not replay the failed prompt. A write
whose response was lost remains an unknown outcome. Workspace/model replacement
still uses its separate VM replacement path described below; changing only a
host adapter would leave stale native grants and is not a safe shortcut.

The default runtime keeps the interactive desktop scheduled between prompts
and during inference. `vkore-harnessd --headless-park` explicitly enables the
headless parking optimization. Timing measurements from the old automatically
parked runtime do not predict this mode's performance.

## Previous baseline (historical)

The following table and earlier timing observations describe the pre-overhaul
baseline. `config/real-stack.lock.mk` still selects that baseline and must not
be used as acceptance evidence for this branch. Candidate execution and the
opt-in native restart gate are documented in
[supervisor acceptance](supervisor-acceptance.md).

Set `KORE_ROOT=/Users/colinhayes/microperceptron/kore-vkore-merge` in
`Makefile.local` or on the command line. Use an isolated backed-up workspace for validation.
Use only one launcher, harness, guest and model stack at a time. The launcher
builds release binaries; `make dev-fast` skips rebuilding. Application settings
own the installed model/ISO profile. Legacy per-project model settings do not
override it after migration.

| Component | Pinned value |
| --- | --- |
| vKore branch | `codex/vkore-40-current-kore`, stacked on `codex/issue24-shared-assistant` |
| Parent PR #41 revision | `506c205ffce90fb831402683c59f980a1568eddd` |
| Kore revision | `6d01d91e74b70c19cde1bbfe36d203064827b6a0` |
| ISO | `kore-vkore-merge/target/kore-x86-release.iso` |
| ISO SHA-256 | `843bcf183326cb0b22feea80ba6d090b23ab781a13b70890dd34f464d3e3517f` |
| Model | `llama.cpp/gguf/Qwen3.5-9B-Q4_K_M.gguf` |
| Model server | `llama.cpp/build/bin/llama-server` |

Paths above are relative to `/Users/colinhayes/microperceptron` except the vKore
branch. The guest source identity is `c1a0fd568bcac9b94bd9e53759bc632c4085d195`; the
artifact checkout records its rebuilt payloads separately. `make artifacts`
reproduces the image using both pinned identities. See
[current-main validation](current-main-validation.md) for acceptance status;
fresh native UI checks are pending.

## Timing and attachment compatibility

Allow five minutes per prompt before declaring it stuck. The current guest source
`c1a0fd568` completed a fresh headless run in 282 seconds for cold create,
100 seconds for readback, 89 seconds for missing-file handling and 77 seconds
for arithmetic on this Mac. Host bytes were independently verified as `hello`;
readback added prose and missing-file handling retained its known failed status.
These are one-run whole-turn observations, not token-generation benchmarks.
Inspect progress rather than treating five minutes as a hard failure;
validation uses a 900-second prompt budget.

Preparation boots an empty application root. The current guest retains native
workspace imports and standing write grants, so it cannot safely switch roots
by changing only the host adapter. The first focus-folder task and subsequent
attachment changes stop and reap the previous guest before installing the new
root and starting its successor. HostLink reset may restart the model server
as well. Repeated prompts and new chats in the unchanged focus folder reuse
the attachment and processes. The earlier PR #41 headless image's persistent
cross-workspace process behavior is not claimed for this image.

## Four prompts in one UI chat

1. `Create a file named test.txt containing the word hello.`
2. `Read test.txt and reply with its exact contents.`
3. `Read missing-file.txt and reply with its contents.`
4. `What is 17 plus 25? Answer with the number only.`

Back up the chosen workspace before testing. Move an existing `test.txt` into
the backup so creation starts with it absent. Verify the resulting host file
independently: it must contain exactly five bytes, `hello`. The original
integration workspace is `~/.local/state/vkore-current-kore-integration/workspace`.
The stacked test used a separate copy under `~/.local/state/vkore-stacked-43-41`.

Check every answer visibly, not just request submission or a status chip.
Arithmetic must receive `executables: []`, `resources: []`, and `tool_calls: 0`.
Test unsent drafts across panels, Settings, project navigation and completion.

Known model limits: readback may add prose, recall can name the wrong earlier
prompt, and the broker can choose replacement write where append was intended.
A correct missing-file refusal may still have failed harness status. These are
not new UI transport regressions.

## Evidence and storage

Native transcripts are under `<state>/transcripts/*.jsonl`. Launcher runtime
logs are under `<state>/runtime/launcher.*/`; retain copies before shutdown or
workspace replacement overwrites guest logs. Headless debug runs retain their
own `<state>/diagnostics/run.*/` directories. Broker proposals and tool results
are recorded in the JSONL events.

Check `df -h /` before builds. Prefer cleaning unused worktrees' regenerable
build caches after checking their process usage. Clearing an active build cache
only delays its disk use until the next build. Preserve source edits, models,
fixtures and acceptance evidence. Never delete GGUF files to make build room.

Linux setup and issue24 validation documents retain historical older-guest
instructions. Their release image is not interchangeable with this pin; Linux
end-to-end validation of this stacked revision has not been performed.
