# Interactive harness runtime API

The harness API is a loopback-only, versioned client boundary for vKore. It is
independent of the HTML interface, so terminal clients, tests, and third-party
local applications can drive the same lifecycle.

## Start the reference runtime

```powershell
cargo run -p vkored --bin vkore-harnessd -- `
  --listen 127.0.0.1:8787 `
  --token YOUR_RANDOM_LOCAL_TOKEN `
  --state C:\path\to\.vkore
```

The default backend is the deterministic broker/worker and guest-protocol test
oracle. To use one caller-supplied llama.cpp server executable and GGUF model
through the same API:

```powershell
cargo run -p vkored --bin vkore-harnessd -- `
  --listen 127.0.0.1:8787 `
  --token YOUR_RANDOM_LOCAL_TOKEN `
  --state C:\path\to\.vkore `
  --llama-server C:\path\to\llama-server.exe `
  --llama-model C:\path\to\model.gguf
```

The harness supervises llama.cpp, assigns separate broker and worker slots,
uses continuous batching, and erases each slot when its route closes.

## Authentication and provenance

All endpoints except health require `Authorization: Bearer TOKEN`. The daemon
stores only the token hash. An accepted prompt is bound to API version, project,
session, generation, prompt ID, idempotency key, and exact text before it is
encoded as a bounded `TaskIntentRequest`. Event projections contain a
non-secret intent ID, never the authentication value.

Model output, tool results, transcript data, workspace bytes, event payloads,
and copied intent-looking strings cannot call the authenticated submission
method. Clients send no guest route, slot, capability, endpoint, mapping, or
host descriptor.

## Endpoints

All JSON records use `api_version: 1`.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/v1/health` | Unauthenticated loopback liveness |
| `POST` | `/api/v1/sessions` | Create or idempotently reopen a project session |
| `GET` | `/api/v1/sessions/{session}` | Retrieve bounded reconnect state |
| `POST` | `/api/v1/sessions/{session}/prompts` | Submit authenticated user intent |
| `GET` | `/api/v1/sessions/{session}/events?after=N` | SSE replay after a sequence cursor |
| `POST` | `/api/v1/sessions/{session}/prompts/{prompt}/cancel` | Cancel queued or running work |
| `DELETE` | `/api/v1/sessions/{session}` | Cancel, retire agent/routes, and erase KV state |

Prompt IDs and idempotency keys are client-generated stable ASCII identifiers.
Retrying an exact request returns the original prompt ID and cannot repeat work.
Reusing a key with changed text or identity is refused. A stale event cursor is
refused after the bounded replay window rather than silently returning an
incomplete history.

## Events

SSE event records bind project, session, optional prompt, current generation,
and a monotonic sequence. Kinds cover user input, broker proposals, committed
capability projections, inference progress, validation, guest tool calls and
results, validated assistant output, errors, terminal status, and conversation
closure. Raw model completion bytes and hidden reasoning are not public output.

Each event has a `public` field. Public assistant and tool presentation remains
separate from internal broker/capability diagnostics. The transcript is an
append-only JSON-lines projection under the caller-selected state directory and
is flushed as events are committed.

## Runtime and guest boundary

`BackendHarness` uses the exact Kore `KBP1`, `KBR1`, `KAR1`, and `KAE1` bounded
records for broker and worker turns. A worker sequence is opened only after a
guest bridge accepts the broker proposal for the current generation. Tool calls
are decoded into owned inert arguments and resolved by that bridge; the model
cannot select an ambient executable or filesystem path.

`DeterministicGuest` is the portable test oracle. It models a resident-broker
commit and an exact executable/filesystem scope with three inert objects, and is
used to make security tests reproducible without a model. The production guest
bridge uses the same `GuestBridge` contract to deliver the intent and tool loop
over HostLink to the Kore VM. The Kore PR #722 QEMU gate separately exercises
the founded agent-runner, guest `cat`, result turn, teardown, stale-generation
rejection, and route/KV lifecycle on this stack.

## Headless client

```powershell
cargo run -p vkored --bin vkore -- chat `
  --server 127.0.0.1:8787 `
  --token YOUR_RANDOM_LOCAL_TOKEN `
  --project my-project
```

The client creates one session, submits sequential prompts, reconnects to the
SSE cursor until each terminal event, prints validated assistant text, and
closes the conversation on EOF.
