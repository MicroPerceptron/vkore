# Interactive harness runtime API

The harness API is a loopback-only, versioned client boundary for vKore. The
native Axiom UI, terminal clients, tests, and third-party local applications can
drive the same lifecycle.

## Start the real runtime

Use `make dev-real` after following [Linux setup](linux-setup.md), or supply
`vkore-harnessd --qemu PATH --state STATE` with Kore's complete QEMU topology.
The native UI connects with `vkore-ui --server 127.0.0.1:8787`; a project is
optional. `GET /` returns native-client discovery JSON, not an HTML document.

The application starts one Kore VM, one llama.cpp server and one resident
broker immediately using `STATE/settings.json`. Projects and chats attach to
that runtime. Deterministic backends are contract-test oracles and cannot be
selected by the production harness. See [ADR 24](adr/0024-shared-assistant.md)
for ownership, migration, bounds, recovery and task filesystem semantics.

## Local boundary and provenance

The server refuses non-loopback binds. Browser mutations also require an exact
local Origin and `X-vKore-Client: harness-ui-v1`; cross-origin requests fail
without permissive CORS headers. An accepted prompt is bound to API version, project,
session, generation, prompt ID, idempotency key, and exact text before it is
encoded as a bounded `TaskIntentRequest`.

Model output, tool results, transcript data, workspace bytes, event payloads,
and copied intent-looking strings cannot invoke the host submission path
method. Clients send no guest route, slot, capability, endpoint, mapping, or
host descriptor.

## Endpoints

All JSON records use `api_version: 1`.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/v1/health` | Loopback liveness |
| `GET` | `/api/v1/runtime` | Application readiness, process IDs, incarnation and bounded startup history |
| `POST` | `/api/v1/runtime/retry` | Explicit shared-runtime preparation/recovery |
| `POST` | `/api/v1/projects/virtual` | Create a project with `{"name":"..."}` and no focus folder |
| `GET` | `/api/v1/settings` | Read versioned application preferences and available models/fonts |
| `POST` | `/api/v1/settings` | Atomically update validated application preferences |
| `GET` | `/api/v1/activity` | Read bounded, privacy-safe local session summaries |
| `GET` | `/api/v1/projects` | List persisted projects |
| `POST` | `/api/v1/projects/from-workspace` | Validate one folder, derive defaults, and create or reopen its project |
| `POST` | `/api/v1/sessions` | Create or idempotently reopen a project session |
| `GET` | `/api/v1/sessions/{session}` | Retrieve bounded reconnect state |
| `POST` | `/api/v1/sessions/{session}/prompts` | Submit user intent |
| `GET` | `/api/v1/sessions/{session}/events?after=N` | SSE replay after a sequence cursor |
| `POST` | `/api/v1/sessions/{session}/prompts/{prompt}/cancel` | Cancel queued or running work |
| `DELETE` | `/api/v1/sessions/{session}` | Cancel, retire agent/routes, and erase KV state |

Prompt IDs and idempotency keys are client-generated stable ASCII identifiers.
Retrying an exact request returns the original prompt ID and cannot repeat work.
Reusing a key with changed text or identity is refused. A stale event cursor is
refused after the bounded replay window rather than silently returning an
incomplete history.

Ordinary project creation sends only `{"root":"..."}` to
`/api/v1/projects/from-workspace`. The server derives the project name, stable
ID, app-owned state path, review-first workspace posture, and current
application model default. Selecting an already registered workspace returns
that existing project with `created: false`; it never creates a duplicate.
Unsafe roots, missing folders, unsupported entries, and unavailable default
models return actionable errors. Portable project JSON has no configurable
workspace byte or file-count quota; legacy quota keys are removed on load.

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
used to make security tests reproducible without a model. In real-VM mode,
`KoreVmRuntime` queues the intent directly on the HostLink host
endpoint. Kore's resident broker and founded agent claim it in order, and only
that generation-bound agent route can publish the terminal result.
`scripts/kore-interactive-gate.sh` exercises this path from HTTP prompt
submission through guest `cat` execution and back to the assistant event.

## Headless client

```powershell
cargo run -p vkored --bin vkore -- chat `
  --server 127.0.0.1:8787 `
  --project my-project
```

The client creates one session, submits sequential prompts, reconnects to the
SSE cursor until each terminal event, prints validated assistant text, and
closes the conversation on EOF.
