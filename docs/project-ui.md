# Project and harness UI

`vkore-harnessd` serves the loopback-only API used by the native Axiom project
and agent UI. It uses the same versioned API as the `vkore chat` CLI. Project
configuration and transcripts are stored below the directory given to
`--state`.

## Guaranteed real-stack developer launch

The normal developer entry point is `make dev-real` from WSL or `.\dev.cmd`
from PowerShell. It builds release vKore binaries and fails closed unless the
application uses llama.cpp with existing runtime/model files and the exact
reviewed Kore image. `make dev-check` performs the same preflight without
starting anything; `make dev-fast` reuses already-built release vKore binaries;
and `make dev-debug PROMPT="hello"` runs a terminal prompt with retained logs.

The reviewed Kore commit and ISO digest live in
`config/real-stack.lock.mk`. The launcher verifies both, but deliberately never
runs `git checkout`: developers should create or select a dedicated Kore
worktree at the pinned revision. `make artifacts` builds the ISO in that
worktree and verifies its digest. Large ISO, llama.cpp, and GGUF artifacts stay
outside Git; `Makefile.local` records only machine-local paths and identifiers.

On macOS, install QEMU (including its EDK2 firmware), GNU Make (`gmake`),
Python 3, and the pinned Rust toolchain. The launcher uses Python's POSIX
session API to own and clean up the real harness, QEMU, and llama.cpp process
group; a separate `setsid` utility is not required. `VKORE_MAKE` selects GNU
Make for the Kore recipes (`gmake` is preferred when installed).

Application settings must name an executable llama-server, a GGUF model, and
the pinned headless Kore ISO. A selected project is optional for native launch. vKore starts llama-server
itself. `--require-real` is also passed to the daemon, which refuses simulated
guests and deterministic project inference. On Apple Silicon, the x86 Kore
guest executes through QEMU TCG; model inference can use llama.cpp's Metal
backend. TCG executes the actual guest kernel and services.

The upstream ISO digest identifies one exact build, not every build from the
same source commit. Cross-host compiler metadata and ISO timestamps can change
the bytes. For an independently built and validated image, `Makefile.local`
can select a separate `VKORE_REAL_STACK_LOCK` containing its source commit,
ISO SHA-256, and relative image path. Keep the upstream lock unchanged and
record the build and production-boot evidence beside the local lock. A local
lock still requires an exact source revision and image digest at every launch;
it does not disable verification or enable a fallback runtime.
If the native build regenerates tracked guest binaries and boot measurements,
also pin `VKORE_EXPECTED_KORE_DIFF_SHA256` to the SHA-256 of
`git diff --binary HEAD --` in that Kore worktree. This binds the recorded
local build to its regenerated artifacts and rejects later tracked changes.

`scripts/reproducible-iso.py` can be selected through `VKORE_XORRISO` for local
builds. It uses the pinned source commit's timestamp and identity for ISO
metadata. It does not alter kernel/service payloads, establish their trust, or
update any digest automatically. Build and validate the production guest
before recording a new local artifact pin.

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

## Start the live app

From the vKore repository:

```sh
make dev-check
make dev-real
```

The launcher starts the native UI together with the real harness.
The application runtime starts preparing before any project is selected.
A named persisted project opens directly without the project modal. If the daemon has exactly one active project, `--project` may be
omitted and that project opens automatically. There is no login, token, or
browser-session step. Choose **New project**, then **Create without a folder**
for a virtual task project, or click **Select or drop a folder here** to open the operating system's folder picker. A folder can also
be dropped directly on that target when the windowing system provides a
directory path. There is no in-app filesystem explorer or configuration wizard:
vKore derives the display name
and stable identity, inherits the application model default, opens the project
after automatic preflight, and stores transaction/replay state under
`.vkore-state/workspaces`; it refuses a state directory nested inside the shared
workspace. The bounded preflight scan reports symlinks without following them,
identifies unsupported entries, and warns for drive roots, home folders, and
common credential directories.

The access modes are:

- **Read only:** verified live reads with no writeback.
- **Review changes:** configuration for staged, exact-change approval.
- **Automatic writeback:** applies approved changes inside the scoped
  workspace. Kore's grant remains the enforcement boundary.

Workspace byte-count and file-count quotas are not configurable or enforced.
Legacy `max_write_bytes` and `max_write_files` keys are removed when an older
project is loaded; a legacy zero never disables writes.

The production harness requires QEMU and live llama.cpp inference. It has no
test-model choice, simulated-guest mode, or automatic fallback. Older test-model
defaults migrate to an unconfigured live model; users must configure real
artifacts before starting. Existing live model settings are preserved.
Deterministic protocol fixtures remain available to automated tests only.

Backend-specific llama.cpp arguments belong in the application's active inference
configuration (`extra_args`), not in a separate simulated-guest launcher mode.

The UI records the Kore image, llama.cpp runtime, and model paths, and refuses
missing files during preflight. It never downloads a selected model implicitly.
Verified import, signature/hash checks, compatibility selection, activation,
and rollback are provided by `vkore-packages`. Selecting a different active
model explicitly restarts the shared runtime with visible preparation status.

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

One application-owned VM, AI server and coordinating broker serve every project
and chat. Preparation starts when the harness opens, using application settings
and an empty read-only bootstrap root. Required headless guest readiness is a
parked broker call after service and executable-store initialization. Old
single-prompt guest images are incompatible and fail visibly; prompt submission
never silently boots a replacement VM.

The sidebar switches between idle project chats while preserving drafts and
chat identity. **New chat** starts a fresh conversation for the selected
project; **Close chat** retires it without stopping the assistant. Active work
must finish or be cancelled before changing projects.

Projects are optional organizational contexts. A focus folder offers task
resources; Settings → Document access authorizes independent read-only host
discovery. Selecting a tool no longer implicitly grants the project root. The
broker chooses explicit file/directory offers, and Kore validates identities
and rights before deriving revocable grants. The filesystem tab shows only
effective task grants, with the broker omitted. Office documents use a read-only
text projection linked to their original backing file.

Focus projects default to automatic writeback. Settings → Workspace → Apply
file changes automatically changes this per project after reopening. Turning it
off blocks host writes; an interactive review/apply workflow is not implemented.
Virtual projects and personal discovery files are read-only. Existing settings
migrate explicitly; opening a focus folder does not authorize global search.

Every prompt gets a fresh worker, route generation, private filesystem view and
capability transaction. At most 32 isolated chat contexts retain 16 exchanges
and 16 KiB each, with a 30-minute idle expiry. The application schedules at most
32 pending prompts fairly across projects. Closing, cleaning, cancelling or
archiving scoped work leaves the shared processes alive. Shared failure retires
all affected generations and requires explicit Retry. See
[ADR 24](adr/0024-shared-assistant.md) and [validation](issue24-validation.md).

Structured generation uses llama.cpp's `/apply-template` endpoint with the
configured model's template, then `/completion` with both a visible JSON schema
and constrained decoding. All results still pass the strict wire decoder.
After tools, the model must author a response rather than emit a bare completion.
One bounded live correction may follow a rejected candidate; there is no canned
answer or deterministic inference fallback.
The worker is instructed to follow requested step order and ground external
claims in observed tool results; validation does not prove semantic task
completion. A live test must inspect the actual calls and effects as well as
the final answer. VM pause/resume follows pending backend requests, not the
number of diagnostic prompt events (a correction emits another prompt for the
same request).

The current Kore task-intent wire format limits each prompt to 128 UTF-8 bytes.
Oversized prompts are rejected explicitly, never shortened silently. The
terminal client keeps the session open after a rejection or a failed turn so
the user can continue; batch runs still exit nonzero if any turn failed.

A submitted prompt is not reported as successful until the resident broker
claims that exact intent, founds a scoped agent, the agent claims
the same intent on its generation-bound route, and its terminal result returns
to the harness. The harness persists and exposes exactly one final response
before acknowledging that result to the guest. Receiving the result alone is
not permission to tear down. After this acknowledgement, the resident broker closes the route, stops
the agent, and revokes its grants while retaining only the project-level
revocation pivot needed to create the next isolated agent. vKore continues
driving the VM and does not emit terminal completion until Kore's agent-teardown
observation arrives and the resident broker parks its next-intent poll. These
two barriers let vKore freeze QEMU between prompts before the guest-side poll
deadline advances. The next intent is queued before vKore resumes QEMU, so an
idle UI cannot strand cleanup or leave a stale cancellation between chats.
For single-prompt guests such as PR909, if the worker is confirmed reclaimed
but no next-intent poll arrives within two seconds, vKore retires that VM and
boots a fresh real guest for the next prompt; inference remains resident.
This fallback never invents an idle or reclamation observation.
If a worker exits before answering, only its claiming broker may report its
failure through the result channel. That error is published before cleanup
and appears in the Working disclosure. Cancellation, transport failure, and
timeout report a host error before forced shutdown; a disconnected client or
failed transcript write cannot guarantee delivery.
After a failed response, missing guest reclamation confirmation is reported
as a diagnostic and the real VM is forcibly shut down after five seconds.
It is not reported as a successful guest teardown.
Capability, tool-call, and tool-result observations stream into the existing UI
while the prompt runs. Authority updates append to one block per prompt;
later cleanup diagnostics do not generate a second public final response.

Structured broker and agent replies are validated before they reach Kore. An
invalid candidate is recorded in the inference audit and gets at most one
correction attempt from the configured model, sharing the original token and
output-byte budget. Only an accepted candidate is forwarded; a second rejection
fails the turn. This does not rewrite arguments or expand capability grants.

## Settings, defaults, and activity

**Settings** is available from the persistent left navigation and is also
reachable from the compact header on narrow windows. It contains separate
General/inference, Appearance, Interface, Activity, and current-project
sections.

The application model is a default for newly created projects. Changing it
does not silently replace an existing project override. In Current project,
**Use application default model** copies the current default into that project;
save the project to persist the change. Missing local model files are shown as
unavailable and cannot be selected as a default.

The font preference is applied after a successful atomic settings save and is
restored before project content is rendered on the next launch. Structured
activity, paths, and logs retain a readable monospace face. **Show Working
panel** controls only whether the per-turn Working UI is rendered. Hiding it
does not delete transcript or audit events. When Working is shown, its default
is collapsed unless **Expand Working by default** is enabled; each turn remains
individually reversible.

Activity reads at most a bounded set of recent local transcript files and
shows summary metadata only: project, session, model, timestamps, terminal
state, and aggregate prompt/result counts. Prompts, raw model output,
credentials, capabilities, and validation envelopes are never included.
Application preferences live in a versioned `settings.json` in the app state
area, outside portable project JSON. Writes use a temporary file and atomic
replacement; malformed settings are preserved for diagnosis and replaced by
safe defaults.

## Add-ons and approvals

The post-creation plugin and policy surfaces display add-on publisher, version,
execution domain, tools, and network destinations. Enabling an add-on
establishes only a project ceiling.
On sandbox start, the UI submits the exact tool request through `vkore-policy`.
The approval dialog is an authenticated user action; model or transcript text
cannot create it. Candidate authority and committed grants are rendered in
separate panels. Production uses the live HostLink authority transaction;
deterministic adapters are restricted to automated tests.

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
