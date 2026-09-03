# vkored

`vkored` is the host endpoint and conformance backend for HostLink v1. It
listens on separate nonblocking Unix sockets for the control and bulk planes,
uses the shared `kore-hostlink` stream decoder and route admission state. Its
inference registry is explicitly bounded to 32 sequences (at most eight broker
and 24 agent sequences), with independent context, request history, pending
queue, cancellation state, output budget, token budget, and accounting for
every route/generation/role identity. The guest's current founded topology
opens one broker and two agent routes; no caller-provided ordinal is used as a
host sequence identity.

The host sends `Hello` only after both planes connect. The guest answers with
`HelloAck`; both freshness values and the frozen negotiated transcript derive
the new epoch. Route-open identities live in the bounded link-control payload,
not the all-zero link-control envelope. Rejected routed requests receive the
common typed refusal record without consuming their receive sequence.

Inference inputs use the bounded `KIR1` envelope: role-specific operation,
monotonic turn, output-token ceiling, timeout, and UTF-8 input. Stream output
uses `KIE1`: ordered nonterminal assistant parts followed by one terminal turn
record with aggregate token use. The deterministic backend retains legacy raw
input only for the existing boot witness, but always emits `KIE1`. Stale route
generations, duplicate request IDs, skipped turns, late worker events, and
post-restart replies fail closed rather than being rebound to a new sequence.

Inbound bulk data follows `BulkBegin` → ordered digest-checked `BulkChunk` →
`BulkFinish`/`BulkReady`, or `BulkAbort`/`BulkAborted`. Staging is limited to
four objects per route, twelve per link, 8 MiB per object, and 32 MiB reserved
per link; these negotiated host ceilings deliberately lower the v1 hard
maximum. Cancellation and disconnect retire staging and restore credits.

Run the deterministic endpoint:

```sh
cargo run -p vkored -- \
  --control-socket /tmp/kore-control.sock \
  --bulk-socket /tmp/kore-bulk.sock \
  --transcript /tmp/kore-hostlink.log
```

Run against a separately supervised llama.cpp server:

```sh
cargo run -p vkored -- \
  --control-socket /tmp/kore-control.sock \
  --bulk-socket /tmp/kore-bulk.sock \
  --llama-server /path/to/llama-server \
  --llama-model /path/to/model.gguf \
  --ui-listen 127.0.0.1:8787
```

`vkored` starts and negotiates the server, assigns one internal llama slot
per sequence, streams through bounded per-sequence worker channels, and erases
the slot at cancellation/retirement. A disconnect, malformed response, or
server exit fails all affected requests, poisons existing routes, increments
the backend incarnation, and restarts the child. Route IDs, task generations,
request IDs, prompts, and guest capabilities never cross the llama-server
boundary. The adapter does not download weights or execute tools. One
supervised `llama-server` process supplies broker and worker slots with
continuous batching, so the model is loaded once rather than once per role.

The optional loopback-only observer API is available at `127.0.0.1:8787` in
the example above. Run `cargo run -p vkore-ui -- --server 127.0.0.1:8787` for
the native Axiom client. The observer is owned by the daemon event loop and
cannot install authority. Its native views keep the user-visible worker
conversation separate from a collapsible, human-readable activity stream.
That stream shows validated Broker and Agent decisions such as agent creation
and tool use, but does not expose model prompts, token chunks, validation
envelopes, or raw JSON. Only validated `respond.text` is rendered as an
assistant message. Conversation and activity text is selectable and copyable,
while remaining read-only. The append-only transcript retains the structured
audit records for diagnostics.

Scheduling is deterministic round-robin across live sequences, with at most
eight dispatches per poll. Cancellation is handled before further dispatch,
and control output reserves queue capacity for cancellation, refusal, and
terminal completion. Metrics expose only counters and resource use: input and
output bytes, prompt and completion tokens, first-token/cancellation/queue/
generation ticks, pending/peak-pending counts, context occupancy, stale worker
events, restarts, and per-sequence totals. The `--transcript` file also records
the same bounded model prompt, raw completion, returned-field, validation,
validated assistant/tool, broker-proposal, and authority-observation records
that feed the UI. It never invents or exposes model reasoning that llama.cpp
did not return.

Run the portable prompt-scoped production gate with the live UI from this
worktree:

```sh
cd /private/tmp/kore-708-prompt-scoped-broker
make run-prompt-scoped-broker-gate-x86 PROMPT_UI_LISTEN=127.0.0.1:8787
```

The deterministic backend remains the acceptance oracle. To opt into the same
gate with one continuously batched llama.cpp process (one model load shared by
the broker and worker slots), provide caller-owned artifacts explicitly:

```sh
make run-prompt-scoped-broker-gate-x86 \
  PROMPT_UI_LISTEN=127.0.0.1:8787 PROMPT_USE_LLAMA=1 \
  KORE_LLAMA_SERVER=/path/to/llama-server \
  KORE_LLAMA_MODEL=/path/to/model.gguf
```

Print the exact QEMU arguments (one modern virtio-serial controller, ports 1
and 2, with legacy transport disabled and IOMMU negotiation required):

```sh
cargo run -p vkored -- \
  --control-socket /tmp/kore-control.sock \
  --bulk-socket /tmp/kore-bulk.sock \
  --print-qemu-args
```

`--max-ticks N` makes a harness run terminate predictably and flush the
optional transcript. Production supervision should request an orderly exit if
the transcript file is required; the library also exposes the bounded live
transcript and metrics directly.
