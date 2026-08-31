# Project and harness UI

`vkore-harnessd` serves a loopback-only, token-authenticated project wizard and
interactive agent harness. It uses the same versioned API as the `vkore chat`
CLI. Project configuration and transcripts are stored below the directory given
to `--state`.

## Start with the deterministic backend

From the vKore repository:

```powershell
cargo run -p vkored --bin vkore-harnessd -- `
  --token replace-this-local-token `
  --state .vkore-state
```

Open `http://127.0.0.1:8787/`, unlock with the token, and create a project. Pick
one narrow workspace directory. vKore derives transaction/replay state under
`.vkore-state/workspaces`; it refuses a state directory nested inside the shared
workspace. The bounded preflight scan reports symlinks without following them,
identifies unsupported entries, and warns for drive roots, home folders, and
common credential directories.

The access modes are:

- **Read only:** verified live reads with no writeback.
- **Review changes:** configuration for staged, exact-change approval.
- **Automatic writeback:** requires nonzero byte and file-count bounds. Kore's
  grant remains the enforcement boundary.

The deterministic backend is explicitly labeled as a test oracle. It drives the
authenticated broker/agent/tool protocol and is useful for UI, policy, and
capability-flow tests, but it is not a substitute for a booted Kore VM.

## Real inference

Start the harness with caller-selected artifacts:

```powershell
cargo run -p vkored --bin vkore-harnessd -- `
  --token replace-this-local-token `
  --state .vkore-state `
  --llama-server C:\path\to\llama-server.exe `
  --llama-model C:\path\to\model.gguf
```

Use repeated `--llama-arg` values for backend-specific options. For example,
`--llama-arg --device --llama-arg none` forces CPU inference when an installed
accelerator backend is unavailable. The same values can be recorded in the
wizard's advanced llama.cpp arguments field.

The UI records the Kore image, llama.cpp runtime, and model paths, and refuses
missing files during preflight. It never downloads a selected model implicitly.
Verified import, signature/hash checks, compatibility selection, activation,
and rollback are provided by `vkore-packages`; the current harness process must
still be restarted to change its active backend.

## Add-ons and approvals

The wizard displays add-on publisher, version, execution domain, tools, and
network destinations. Enabling an add-on establishes only a project ceiling.
On sandbox start, the UI submits the exact tool request through `vkore-policy`.
The approval dialog is an authenticated user action; model or transcript text
cannot create it. Candidate authority and committed grants are rendered in
separate panels. Test mode uses the deterministic resident-broker adapter. A
production Kore connection must replace it with the live HostLink authority
transaction.

Policy keys and the authenticated audit chain are stored in the application
state area, never in portable project JSON. Project JSON contains no OAuth token,
account credential, or serialized Kore capability.

## Conversation controls

Starting a project creates an isolated harness session and inference route.
Prompts stream validated public assistant text into the conversation. Tool calls,
tool results, validation, raw proposals, and capability state remain visibly
separate. **Cancel** requests generation-bound cancellation. **Clean** closes the
old session, retires its route, erases its KV slot, and starts a new generation.

Transcripts are append-only JSONL below `--state/transcripts`. Closing a session
flushes terminal and teardown events. Project retention values are portable
policy preferences; a maintenance process may enforce deletion.

## Security model

- The server refuses non-loopback binds.
- Every data API uses bearer authentication.
- Browser mutations require an exact local origin and the non-simple
  `X-vKore-Client: harness-ui-v1` header. Cross-origin requests are rejected and
  no permissive CORS response is emitted.
- The token stays in browser `sessionStorage` and is excluded from project files.
- Native file and directory dialogs originate in the trusted local daemon, not
  model output.
- Project paths are configuration. They do not enlarge a live agent's Kore
  capabilities.

## Current integration boundary

The host QEMU supervisor and workspace adapter exist in vKore, but PR #722's
guest does not expose the interactive frontend ingress needed to route these UI
prompts into a newly booted VM. The deterministic UI path is fully runnable and
the llama.cpp path runs against the deterministic guest protocol oracle. Live
UI-to-QEMU guest execution remains an explicit integration boundary rather than
a simulated success indicator.
