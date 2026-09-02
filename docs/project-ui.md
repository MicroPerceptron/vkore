# Project and harness UI

`vkore-harnessd` serves the loopback-only API used by the native Axiom project
and agent UI. It uses the same versioned API as the `vkore chat` CLI. Project
configuration and transcripts are stored below the directory given to
`--state`.

## One-command launch

On Windows with WSL, run `.\vkore-axiom.cmd` from the repository root. From
WSL, run `./vkore-axiom`. The launcher uses the native Windows Axiom client for
responsiveness, while the harness, QEMU, Kore, and llama.cpp stay in WSL. It
opens `qwen9b-live-kore` by default; pass `--project PROJECT_ID` to select a
different persisted project.

Before opening the app, the launcher checks the pinned Kore source commit and
ISO SHA-256. It fails immediately for a missing, stale, or unreviewed image and
never rebuilds Kore or the ISO. Use `./vkore-axiom --check` for a read-only
preflight. A healthy harness already listening on the requested loopback port is
reused; otherwise the launcher owns and cleans up the harness process group.

## End-to-end diagnostics

Use the same launcher without the UI to reproduce one prompt and retain every
layer's timing evidence:

```powershell
.\vkore-axiom.cmd --debug --prompt "hello" --timeout-seconds 300
```

The command refuses to attach to an already-running harness so the captured
binary and process tree are unambiguous. Every terminal line is UTC-stamped.
The printed diagnostics directory contains `launcher.log`, `harness.log`, raw
`events.jsonl`, one-second `processes.log`, QEMU `kore-serial.log` and
`kore-debug.log` plus timestamped timeline copies, `project.json`, and
`manifest.txt`. All artifacts are retained after the launcher stops its
harness, llama.cpp, QEMU, and helper processes.

## Start with the deterministic backend

From the vKore repository:

```powershell
cargo run -p vkored --bin vkore-harnessd -- `
  --state .vkore-state
```

Then launch
`cargo run --release -p vkore-ui -- --server 127.0.0.1:8787 --project PROJECT_ID`.
The named persisted project opens directly and its sandbox starts without the
project modal. If the daemon has exactly one active project, `--project` may be
omitted and that project opens automatically. There is no login, token, or
browser-session step. Create a project only when you need a new configuration,
and pick one narrow workspace directory. vKore derives
transaction/replay state under
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
broker/agent/tool protocol and is useful for UI, policy, and
capability-flow tests, but it is not a substitute for a booted Kore VM.

## Real inference without a VM

Start the harness with caller-selected artifacts:

```powershell
cargo run -p vkored --bin vkore-harnessd -- `
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

## Native rendering lifecycle

The desktop client keeps Iced above Axiom's durable `Frame<System>` boundary.
Ordinary UI structure is executed by `axui-raster` through Axiom's shared
`axnn::RasterBackend` CPU kernels directly into the platform's mapped XRGB
surface. Its scalar implementation is retained only as the deterministic
conformance/fallback path. Cosmic Text shapes the
bundled DM Sans OpenType face above `Frame`; exact glyph ink bounds and phases
cross the durable boundary, then the backend produces grayscale coverage into
a bounded glyph atlas. Analytic signed-distance coverage softens geometry
edges; retained linear gradients and Gaussian shadows stay in the structure
lane. The backend also owns
an isolated-layer scratch arena, but no viewport-sized mirror of the
presentation surface.

Only a frame that differs from the last successfully presented frame may
acquire and mutate a platform target. An exact stable frame bypasses structure
rasterization and presentation. Failed rendering or presentation never replaces
the retained visible-frame identity. vKore currently emits no per-pixel fields;
a field command is refused before any target pixel changes until a live field
provider is connected.

Set `VKORE_UI_PROFILE=1` to print per-frame display-list, glyph-atlas, structure,
scratch, and presentation-byte counters. The reproducible before/after capture
and release-mode timing method is recorded in
`artifacts/ui-upgrade/README.md`.

## Real Kore VM

Start `vkore-harnessd` with `--qemu PATH` and the repeated base QEMU arguments
needed by the selected Kore image. In this mode the project supplies the Kore
image, workspace, backend, llama.cpp runtime, model, and resource limits. vKore
adds loopback HostLink devices and starts one supervised VM for the open
project. The trusted broker, its revocation face, HostLink epoch, and inference
backend remain resident between prompts. Every prompt gets a fresh agent task,
route generation, model context, budget, mappings, and capability grants. Kore
revokes and reclaims that prompt authority before admitting the next intent, so
conversation continuity does not silently preserve an earlier prompt's file or
tool access.

A submitted prompt is not reported as successful until the resident broker
claims that exact intent, founds a scoped agent, the agent claims
the same intent on its generation-bound route, and its terminal result returns
to the harness. After the result, the resident broker closes the route, stops
the agent, and revokes its grants while retaining only the project-level
revocation pivot needed to create the next isolated agent. vKore continues
driving the VM and does not emit terminal completion until Kore's agent-teardown
observation arrives and the resident broker parks its next-intent poll. These
two barriers let vKore freeze QEMU between prompts before the guest-side poll
deadline advances. The next intent is queued before vKore resumes QEMU, so an
idle UI cannot strand cleanup or leave a stale cancellation between chats.
Capability, tool-call, and tool-result observations stream into the existing UI
while the prompt runs.

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

Opening a project creates its isolated harness session and inference route
automatically, so the chat composer is ready without a separate start step.
Prompts stream validated public assistant text into the conversation. The
reversible **Working** panel shows readable Broker and Agent decisions such as
agent creation and tool use. Model prompts, token chunks, tool-result envelopes,
validation records, and raw JSON remain out of the chat UI; they stay available
in the append-only audit transcript. Conversation and activity text is
selectable and copyable but read-only. **Cancel** requests generation-bound
cancellation. **Clean** closes the old session, retires its route, erases its KV
slot, and starts a new generation.

The native client polls at 50 ms while a prompt is active for responsive
streaming, then backs off to 750 ms while idle to avoid constant loopback and
render churn.

Transcripts are append-only JSONL below `--state/transcripts`. Closing a session
flushes terminal and teardown events. Project retention values are portable
policy preferences; a maintenance process may enforce deletion.

## Security model

- The server refuses non-loopback binds.
- Native requests carry no browser origin. If a browser attempts to use the
  API, mutations still require an exact local origin and the non-simple
  `X-vKore-Client: harness-ui-v1` header; cross-origin requests are rejected.
- Native file and directory dialogs originate in the trusted local daemon, not
  model output.
- Project paths are configuration. They do not enlarge a live agent's Kore
  capabilities.

## Integration gate

Run `scripts/kore-interactive-gate.sh` from Linux or WSL with `KORE_ROOT` set to
the matching Kore checkout. It creates a project and prompt through the same API
used by the UI, then requires a real capability-scoped guest tool result to
return before it passes.
