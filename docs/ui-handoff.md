# vKore UI handoff — current-Kore integration

For a computer-use agent driving the **native UI** (`make dev-real`). Everything
below was measured on 2026-09-19/20, not inferred.

---

## 1. Launch

```
cd /Users/colinhayes/microperceptron/vkore-current-kore
make dev-real
```

That builds release vKore, boots the pinned Kore ISO under QEMU, starts
llama.cpp, and opens the native `vkore-ui` window. Use `make dev-fast` to skip
the rebuild if nothing changed.

`make dev-check` verifies the pins and exits without launching. Run it first if
anything looks wrong — it prints the resolved model and ISO.

### DO NOT run two of these at once
One QEMU + one llama.cpp at a time. A second launch will fight for the disk and
the port, and the failure looks like an unrelated guest hang.

---

## 2. Exact stack under test

| | |
|---|---|
| Kore commit | `74ae24c87323dd7bc7ac0a12770c25ec8c8c4852` (branch `codex/vkore-994-hostlink`) |
| Kore ISO SHA-256 | `bfee034fa6312b3aab799e2b79031f8fe2da57d32028603541f2ee510cb4afac` |
| vKore commit | `b2e7bbc` (branch `codex/vkore-40-current-kore`) |
| model | `/Users/colinhayes/microperceptron/llama.cpp/gguf/Qwen3.5-9B-Q4_K_M.gguf` |
| llama-server | `/Users/colinhayes/microperceptron/llama.cpp/build/bin/llama-server` |
| state dir | `~/.local/state/vkore-current-kore-integration` |
| workspace (host side) | `~/.local/state/vkore-current-kore-integration/workspace` |

The pins are in `config/real-stack.lock.mk`. The launcher refuses to run if the
Kore checkout or ISO does not match — it will **not** rebuild or guess.

---

## 3. Timing — be patient, these are normal

Measured on this Mac, TCG (no hardware accel), cold:

| phase | time |
|---|---|
| llama.cpp ready | ~11 s |
| guest boot to 20/20 services | ~39 s |
| first tool-using prompt | ~130 s |
| later tool-using prompt | ~50 s |
| chat-only prompt (no tools) | ~34 s |

**A prompt taking two minutes is not hung.** Give a tool-using prompt at least
5 minutes before treating it as stuck. The UI shows working state if
`show_working` is on (it is, in `settings.json`).

---

## 4. The workspace

Host directory, visible to the guest as the `project` view:

```
~/.local/state/vkore-current-kore-integration/workspace/
  integration-input.txt   42 bytes — "Kore integration witness: blue heron 962."
  test.txt                 5 bytes — "hello"   (created by the agent, 2026-09-19)
```

`integration-input.txt` is **the fixture**. A destructive prompt can overwrite
it. Back it up before anything risky:

```
cp -a ~/.local/state/vkore-current-kore-integration/workspace /tmp/ws-backup
```

Host writes land through automatic writeback — a file the agent creates really
appears here. That is the check that matters; do not trust the agent's prose.

---

## 5. Sequences that are known to work

All four verified end to end on this exact tuple, in one session:

1. `Create a file named test.txt containing the word hello.`
   -> broker grants `create` + `write` over the **project view**, budget 3
   -> host file appears with 5 bytes
2. `Read test.txt and reply with its exact contents.` -> `hello`
3. `Read missing-file.txt and reply with its contents.` -> correctly reports absent
4. `What is 17 plus 25? Answer with the number only.` -> `42`, granted **zero** capabilities

Number 4 is the interesting one to re-check: a correct run grants
`executables: []`, `resources: []`, `tool_calls: 0`. If a self-contained
question is granted a file, that is a regression.

---

## 6. Known-bad, do not report as new

- **Recall is unreliable.** Ask "what did I first ask you?" and the model may
  name a later prompt. Documented in `docs/real-chat-validation.md`; model
  quality, not the capability path. Tracked by #26.
- **Destructive-verb selection.** The broker can still pick `write` (replaces
  the whole file) where `append` was meant. Prompt guidance mitigates it; it is
  not eliminated. #26 / #42.
- A prompt whose agent answers `refused` is counted a **failed** prompt by the
  harness even when refusing is the right answer (e.g. a genuinely missing
  file). Judge by the text, not the status chip.

---

## 7. Hard bounds — a plan that exceeds these is refused, correctly

| bound | value |
|---|---|
| executables per worker | 4 |
| resources per worker | 16 |
| agent text chunk | ~500 bytes |
| inline payload | 4096 bytes |

Also structural, and not negotiable by prompting:

- A **view** resource is a directory and carries authority over the names in
  it. An **object** resource is one existing file, contents only.
- `create`, `remove`, `move`, `link`, `make-directory` need a **view**. They are
  impossible on an object — the capability model bars namespace rights there.
- `write` and `append` do **not** create a file. Creating with content is
  `create` then `write`, in that order.

If a sequence fails with `no such name` on a write, that is the system working.

---

## 8. Where the evidence is

Each run writes a retained directory:

```
~/.local/state/vkore-current-kore-integration/diagnostics/run.<timestamp>.<id>/
  events.jsonl        every event, including each broker plan and tool result
  harness.log         host side, including "authority report accepted"
  kore-serial.log     guest console
  timings.md          per-phase timing
```

To see what the broker actually decided:

```
grep -o '"kind":"BrokerProposal".\{0,220\}' <run>/events.jsonl
```

To see what a tool actually returned:

```
grep -o 'result.\{0,120\}' <run>/events.jsonl | grep tool_result
```

**These directories are large (~30 MB each, plus a qcow2).** Delete old ones.

---

## 9. Disk — this will bite

The host disk runs near full. At the time of writing: **3.8 GiB free of 228 GiB
capacity**. A vKore run needs room for a qcow2 and the diagnostics; a Kore
rebuild needs several GiB more.

If anything fails with a confusing I/O or build error, check disk **first**:

```
df -h /
```

Safe to delete (all regenerable):

```
rm -rf ~/.local/state/vkore-current-kore-integration/diagnostics
rm -rf /Users/colinhayes/microperceptron/kore-vkore-port/target/debug
```

Do **not** delete the GGUF models.

---

## 10. What is not covered

- Nothing here is merged. All three PRs are open and unreviewed
  (kore#1015, vkore#43, libkore#22).
- Visual/native-UI acceptance has never been done — every result above came
  from the headless path. **That is exactly the gap this handoff exists to
  close.** Treat UI-specific behaviour (layout, focus, draft preservation,
  multi-chat, the workspace picker) as untested.
