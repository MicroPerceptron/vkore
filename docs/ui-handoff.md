# Current-Kore native UI handoff

PR #43 incorporates PR #41's application runtime and chat UI. See
[stacked computer-use validation](stacked-ui-validation.md) for the September 21
results and compatibility limits. The earlier HTTP failure and fix remain in
[the September 20 report](ui-computer-use-test.md).

## Launch the verified local stack

```sh
cd /Users/colinhayes/microperceptron/vkore-current-kore
make dev-check VKORE_REAL_STACK_LOCK=config/real-stack.lock.mk
make dev-real VKORE_REAL_STACK_LOCK=config/real-stack.lock.mk
```

The explicit lock overrides an ignored local Makefile selecting an older lock.
Use only one launcher, harness, guest and model stack at a time. The launcher
builds release binaries; `make dev-fast` skips rebuilding. Application settings
own the installed model/ISO profile. Legacy per-project model settings do not
override it after migration.

| Component | Verified value |
| --- | --- |
| vKore branch | `codex/vkore-40-current-kore`, stacked on `codex/issue24-shared-assistant` |
| Parent PR #41 revision | `506c205ffce90fb831402683c59f980a1568eddd` |
| Kore revision | `74ae24c87323dd7bc7ac0a12770c25ec8c8c4852` |
| ISO | `kore-vkore-port/target/kore-x86-release.iso` |
| ISO SHA-256 | `bfee034fa6312b3aab799e2b79031f8fe2da57d32028603541f2ee510cb4afac` |
| Model | `llama.cpp/gguf/Qwen3.5-9B-Q4_K_M.gguf` |
| Model server | `llama.cpp/build/bin/llama-server` |

Paths above are relative to `/Users/colinhayes/microperceptron` except the vKore
branch. No guest source or ISO rebuild is part of this integration.

## Timing and attachment compatibility

Allow five minutes per prompt before declaring it stuck. Cold tool prompts
have taken roughly 130–155 seconds, warm file prompts about 50–60 seconds, and
chat-only prompts about 35–40 seconds on this Mac. These are whole-turn times,
not model token-generation timing.

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
