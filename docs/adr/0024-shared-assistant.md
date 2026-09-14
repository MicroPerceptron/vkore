# ADR 24: One application assistant with scoped task filesystems

Status: implemented on the issue #24 branches, stacked on vKore #39 and Kore #933.

## Ownership and startup

The application owns one QEMU process, one llama.cpp server, and one Kore
resident coordinating broker. The application retains four bounded memory supplies for workers. Each prompt
derives new revocable subleases; teardown revokes those subtrees before the
next prompt maps and clears every byte through the existing transient window.
Pool supplies are allocated once during preparation. This avoids MM’s
cumulative per-client supply quota without relaxing it or retaining a prior
worker’s authority or data.

Opening the harness starts preparation on its
worker thread immediately, before any project selection or submitted prompt.
An empty, read-only application root permits the trusted guest bootstrap to
register executables and park at `BrokerNext` without project authority.
Readiness requires that parked broker call, not just a listening TCP socket.
The native 2220 ms brand animation runs independently; its completion does not
mean the assistant is ready. The status bar continues to report preparation.

The headless guest feature `boot-vkore-headless` skips Kore's cosmetic splash,
while retaining the framebuffer handoff and service founding checks. Its ISO
and kernel build cache have distinct names from graphical and test images.
Standalone graphical Kore keeps its splash and interactive founding path.

`GET /api/v1/runtime` reports model startup, VM boot, HostLink connection,
bootstrap and broker readiness, process identities, runtime incarnation, and a
bounded history of 64 phase/service transitions with elapsed and wall-clock times.
Serial kernel-entry and service markers use the host observation clock (10 ms
polling); firmware/bootloader time ends at observed kernel entry. Buffering
means these are observational timings, not cycle-level guest CPU measurements.
The broker-ready checkpoint follows executable-store initialization. The VM
parks between prompts and while waiting for host inference, then resumes for
guest work. No prompt path can boot another VM.

## Configuration migration

Application settings schema 2 owns the active inference profile and VM memory
and CPU limits. Migration runs once: adopt unanimous active legacy project
model/VM settings; conflicting model profiles require an explicit installed
model selection in Settings, while conflicting VM limits use 2 CPUs/2048 MiB.
Existing explicit application model selection wins. Legacy project profiles
remain as model catalog inputs; switching projects never switches the model.
Changing the active application model is an explicit shared-runtime restart.
Project prompt deadlines remain scoped to that project's tasks.

Project schema 2 records whether its backing folder is a user-selected focus
folder or an internal empty root for a virtual project. Schema 1 roots migrate
explicitly to focus folders (`virtual_only: false`), retaining their writeback
policy. Creating a virtual project needs a name and configured application
model, not a host folder. A focus folder offers resources for the task; merely
opening it does not grant workers its contents or authorize global discovery.

## Broker, scheduling and context

One guest broker survives completion and refusal. Each prompt receives unique
request IDs, a new worker task and inference route generation, a private view,
and an explicit grant transaction. The worker returns the exact affine
HostLink ring and kick owners before the broker tears it down and accepts the
next intent. Revoked endpoint liveness is checked through a kernel session;
cached capability metadata is not proof that a view remains live. Prior
imported workspace grant lineages are revoked before replacement publication.

The host serializes guest authority transactions through one worker. At most
32 prompts are pending; available projects alternate turns, preserving FIFO
within a project. Session metadata is bounded at 64 sessions, with closed
sessions eligible for eviction; prompt/idempotency history is bounded at 4096
prompts per session and event replay at 512 events per session. Durable local
transcripts remain separate from those in-memory bounds.

At most 32 conversation contexts are retained, each bounded to 16 exchanges and
16 KiB. Contexts expire after 30 minutes idle and least-recently-used contexts
are evicted at capacity. Context is keyed by project and chat; only that chat's
validated user/assistant exchanges are supplied to its worker. Broker KV resets
at each authenticated intent; worker slots are erased at retirement. Shared
model weights do not share conversation history or filesystem authority. No
cross-chat personal-memory store is introduced by this change. The native UI
retains each opened project’s current chat, unsent draft and up to 512 replay
events and active prompt identity when switching between projects. It does not close that chat or
recreate its backend. New chat and Close chat explicitly retire only the
selected scope. Navigation leaves active work running in the backend; returning
to a chat resumes event replay and restores its answer and cancellation target.
Only a short outstanding UI mutation blocks navigation until its acknowledgement.

## Discovery and task filesystems

Settings contains an explicit list of authorized read-only discovery folders.
This permission list is not a browsable assistant library. The internal host
search scans names and supported content, ranks candidates deterministically,
and offers at most eight selected objects to Kore. It is not a second AI
service. Plain UTF-8 text, Markdown, CSV, JSON, Rust, Python, HTML, PPTX slide
text and DOCX document text are supported. PDF, OCR, embedded images, macros,
and arbitrary Office attachments are not interpreted. “Last week” currently
boosts recently modified documents; it is a ranking hint, not an exact calendar
filter. The worker must ground the answer in document contents.

Search has explicit bounds: 16 authorized roots, 10,000 entries, depth 12,
256 pending directories, a 3-second scan deadline checked between entries,
32 MiB source files, 128 KiB extracted text, and bounded ZIP members/expansion.
A single filesystem read can exceed the between-entry time budget. Hidden
entries and symbolic links are skipped. Oversized/unsupported files are not
silently exposed as complete content. Discovery results carry stable backing
identities and retained directory capabilities. A selected file's raw digest
and regular-file identity are rechecked when attached and on each range read;
changed or replaced source data fails closed instead of serving stale text.
Office text is exposed through a read-only virtual `.txt` alias. Same-named
files from different locations receive different identity-derived aliases.

The deterministic authority layer validates catalog selectors and source
identity again before deriving grants. Exact file grants are preferred for
known files; directory grants permit traversal of that selected subtree. A
focus root is a possible explicit directory offer, not an automatic grant
implied by choosing a tool. The worker's private home is read-only; writable
operations require the explicit imported grant and existing host writeback
policy. Personal discovery objects remain read-only.

The filesystem UI replays effective grants, narrowing, revocation, teardown
and terminal events. It shows the active task's union of granted resources,
with per-agent inspection, file/directory type, rights and backing location.
Directory contents derive from the attached manifest only after the directory
grant commits. Proposals, search hits and attempted paths do not create nodes.
The coordinating broker is omitted from this view but remains in agent and
activity views. Completed/retired grants remain audit events, not live access.

## Close, cancellation and failure

Chat close, Clean, project archive and prompt cancellation retire scoped work
and conversation state; they do not own the VM or model server. Cancellation
asks the active inference route to stop and lets the guest return its transport
owners and revoke grants before another task runs. Erasing a llama slot has a
bounded control timeout independent of the short inference polling timeout.
Failure to reclaim within the deadline is a visible runtime failure.

Unexpected VM/server failure retires the shared runtime, clears affected
conversation contexts, and advances every attached session generation. Queued
work carrying an old generation terminates as cancelled, never running in the
replacement runtime. `POST /api/v1/runtime/retry` deliberately prepares a new
runtime. Retry is idempotent while the existing compatible runtime is healthy.
Completed bulk transfers are submitted directly to their typed backend; the
separate twelve-entry diagnostic history evicts old observations instead of
blocking the next prompt. App shutdown cancels startup/active work and drops the shared process owners;
the launcher also owns its child process group for abnormal termination.

## Validation

See the issue #24 validation report for exact source/artifact revisions,
commands, native captures, startup measurements and live multi-project results.
Contract tests cover queued scope, fairness, closed work and shared generation
invalidation. They supplement, rather than replace, real Kore/Qwen acceptance.
