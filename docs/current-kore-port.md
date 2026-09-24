# Current Kore integration worktree

The in-progress port uses current DagFS for image seeding and recovery, and
current Kore contracts for the host daemon and live workspace adapter. The
guest changes are not yet a committed release candidate. Historical wire pins
and the current-main native package pins in `Cargo.toml` must not be treated as
the completed migration or used to promote an ISO.

Generate a complete local override from the selected Kore integration checkout:

```sh
python3 scripts/local-kore.py /path/to/kore > /tmp/vkore-local.toml
cargo --config /tmp/vkore-local.toml metadata --all-features --format-version 1 > /tmp/vkore-owners.json
python3 scripts/local-kore.py /path/to/kore --metadata /tmp/vkore-owners.json
cargo --config /tmp/vkore-local.toml test -p vkore-workspace --lib image::tests
cargo --config /tmp/vkore-local.toml test -p vkore-workspace --lib live::tests
cargo --config /tmp/vkore-local.toml test -p vkored --lib workspace::tests
```

The generator validates the complete direct dependency inventory. The metadata
check rejects duplicate native owners, owners from another checkout, and kernel
dependencies. The image adapter uses `kore-driver::block` and `kore-svc-dagfs`;
the retired block/filesystem packages are not compatibility owners.

These commands update the worktree's lockfile for local composition. That lock
is provisional. Before promotion, select the committed integration revision for
every Kore dependency, regenerate the distribution lock without local overrides,
and validate it with the freshly built guest artifact and recorded provenance.

libkore belongs in the guest application graph. Kore's native commands must
consume the selected external facade with its ABI, runtime, filesystem and
driver roots patched to the same Kore owners. Adding libkore to this host daemon
would not establish guest adoption. See the umbrella issue and libkore's
`docs/integration.md` for the cross-repository composition requirements.

The current frontend requires the guest's `BrokerRetired` exchange (frontend
poll phase 3). After receiving the terminal result's publication receipt, the
resident broker reaps all native worker/exec/view ownership, closes and drains
the old worker route, rotates its generation, and reports that exact intent
retired. Refusal before any native reservation reports retirement without
inventing a worker. Partial founding must complete its actual cleanup first.

The host accepts this report only from the intent's claiming broker generation,
after publication, outstanding result receipts and old worker route closure.
It acknowledges retirement, then waits for `BrokerReadyNext` (poll phase 5)
naming that same intent before the runtime completes the prompt and releases
its completion record. This message also polls for the lane's next intent.
An initial `BrokerNext` carries zero and acknowledges no previous completion.
The distinction prevents one concurrent lane's polling from completing another
lane's retirement/readiness obligation.
Telemetry, model observations and unrelated workers cannot satisfy this barrier.
Older guests without this exchange are not compatible with this integration
worktree; promote the host and guest together with the final coherent pins.

The native frontend has one independently retained lifecycle per actual held
worker face (currently two), with one shared policy owner and request allocator.
The private worker startup record is now version 6: its counted configuration
includes a dedicated 4 KiB resource-catalog lease, and one live update can carry
complete resource/tool changes and lifetime call/wall-duration updates. Versions 1–5 are refused.
This uses the existing SPSC ring
with one bounded record; it does not rewrite immutable ProcessIo arguments or
context while the child runs. The parent can publish only for an acknowledged
pause revision. MutateAuthority makes the child consume and validate that exact
generation/revision, install its tool/resource changes and runtime limits before
acknowledging AuthorityMutated. Policy commit and the separate Resume/Resumed
exchange follow that acknowledgement. The old ResumeCatalog path is refused.
The backing remains charged and retained through native cleanup. The
broker's live filesystem mutation transaction drives this publication path;
resource records alone cannot grant native authority or add executable caps.
It demultiplexes the broker face by exact pending request IDs and advances every
lane each dispatch. Each lane founds only in its assigned native slot, retains
its own control/result/retirement exchanges and drains its own authority-report
queue before requesting more work. Source inventory is preserved while another
lane has a frozen plan or native worker. The host serializes broker intent
claims through planning/founding, releasing the next claim when the prior worker
claims its intent (or the broker reports a terminal outcome). Founded workers
can continue independently while the next broker plan runs. Claim admission
also checks the exact backend-owned broker sequence: a cancelled HTTP worker
must finish cleanup before another prompt can start. Cancellation receipts and
cumulative metrics are insufficient. A lane's phase-5 readiness is recorded
immediately even while its next claim waits behind another plan.

After a backend-wide failure, the llama backend retains failed HTTP workers
until their terminal messages are consumed and their threads are joined. A
resident broker becomes eligible again after those failed owners and its old
terminal events have drained and a replacement server has been started. Its
next structured turn-one request must have a fresh request ID and successfully
erase its exact model slot before resetting the poisoned model context. This
also covers a broker that was idle during another sequence's failure. Raw
continuations and poisoned agent sequences remain refused. This recovers the
broker's model context without manufacturing fresh guest authority; native
worker cleanup still follows the per-intent receipt/retirement protocol.

Supervision also checks for server death when no inference is pending. If a
replacement fails to start, polling retries after a one-second host-clock
delay; each attempt retains the configured startup timeout. Guest ticks may
remain frozen throughout recovery. Failed attempts leave broker admission
unavailable, and idle failure does not invent terminal results for nonexistent
requests. Successful restart returns to the structured prompt reset above.

This wiring still requires a fresh
guest/model/UI concurrency witness; compile and protocol tests alone do not
prove that production acceptance.

Per-intent cancellation uses `BrokerControl` (poll phase 4), with bounded typed
Active/CancelRequested/Settled replies from the host. The resident dispatches
control and planning/result exchanges by their owned request IDs. It cancels
its exact in-flight planning request, or publishes the worker cancellation and
then follows the normal receipt/native retirement/readiness sequence. Queued
intents can be removed before any broker claim without touching guest routes.

Cancellation accepted before a terminal result reserves the Cancelled outcome;
an already accepted terminal result is preserved. The runtime resumes a paused
VM and keeps it awake for cancellation so pending model inference cannot prevent
the broker from observing its control state. Healthy cancellation preserves the
VM and unrelated routes. An unconfirmed cancellation still has a bounded failure
timeout and VM shutdown fallback; live VM isolation/recovery is not yet boot
validated. Known terminal model requests accept a late cancel without replaying
their reply or reconnecting the link; this history is bounded per generation.

Admitted model requests fail with `INFER_REPLY / TurnFailed`, carrying the
bounded typed inference reason and exact request/turn/part. A model timeout,
invalid response or backend error therefore ends that request without poisoning
HostLink. The broker/worker publishes the frontend failure and completes native
retirement. Protocol-level `REFUSED` retains its existing epoch retirement rules.
The host's VM pause decision accounts for failed/cancelled intents and pending
frontend completion ownership, so an unrelated pending model cannot keep a
worker asleep while it needs to settle. These paths have focused protocol and
native compilation evidence; real guest/model failure-recovery gates remain open.

## Original working-tree carry-over inventory

The original working tree at `4d8b5e76863fa9aa1931ea6f98a9e7173b33971d` retains
the following uncommitted timing work. It has been preserved without modification;
these edits are now adapted into this integration tree. `vkore-axiom` is the
executable Bash launcher, not a submodule.

| Source | Behavior to carry over | SHA-256 at inventory |
|---|---|---|
| `crates/vkored/src/bin/vkore.rs` | Time prompt submission and CLI event receipt | `ebdb4239d23211847ff3d314433840726fcc0b3d0f16415207557e88d8722f82` |
| `crates/vkored/src/interactive.rs` | Add route, generation and request to inference observations | `7568fa00b9f8df5d15a96ea57d52f9a9c7eef17d169471b96eb4b0a54298f310` |
| `crates/vkored/src/kore_runtime.rs` | Monotonic prompt timing, startup milestones, bounded incremental serial observations, loop and publication costs | `b0045e6a0df5a79499bc1ba894ea7375b23fcc7662a887aaf03ed639f2664704` |
| `vkore-axiom` | Generate diagnostic timing reports while retaining CLI failure status | `a3a2ca095491eb71238f5a7ef76ba94bc90dad88cb99ff572a19e5bdcafdfba4` |
| `docs/performance-timing.md` | Measurement boundaries and explicitly historical results | `c9058c3b2ac67b2c213a2df1998a736a212946ca70e9175da318711c228e09cc` |
| `scripts/report-timings.py` | Phase, model and tool timing report generation | `0a3e1c17636027e93661cf7b991016266c8f38a969425e0d7abe3f36cc6b3040` |
| `scripts/test_report_timings.py` | Cumulative model timing and failed-startup report regressions | `f0e9bc22e9e2cf58e9ad301e8eafc0d999cc53f2eb017575e6ecc1d136124c2c` |

Runtime instrumentation preserves the current per-intent publication and
retirement protocol. Historical aggregate teardown/idle counters and fatal
backend-transcript handling remain superseded. Serial timing observations are
bounded to 128 markers of at most 512 bytes; they cannot satisfy readiness or
cleanup. The reporter recognizes `kore_response_published` and
`kore_frontend_ready`, emitted after the corresponding actual runtime transitions.
It matches tool call/result observations by role, route, generation and call ID,
allowing results on a later inference request without mixing concurrent workers.
Missing or duplicate call identities remain unmeasured.

Focused validation passed: eight runtime tests, four CLI tests, six Python report
tests and launcher Bash syntax. The runtime test exercises a real missing-project
failure through the instrumented entry point; report tests include overlapping
workers, stale generations, session separation and actual Markdown/JSON output.
No live QEMU/model/UI timing measurement was performed for this port. The older
real-run measurements and test claims remain explicitly historical.

## Live update transport checkpoint (2026-09-15)

LinkCore/Daemon now expose queue_frontend_update for an exactly claimed active
intent and take_frontend_update_result for its separate outcome. The bounded
owner holds at most eight updates, one per intent, with nonwrapping host
revisions and the original broker route/generation. Delivery uses that broker's
existing control poll. Worker routes, replacement generations, stale revisions
and results for undelivered updates are refused. Output backpressure preserves
both update delivery and result receipt custody. Only undelivered work can be
cancelled locally; delivered work needs guest testimony or an explicit unknown
outcome on broker/link closure. Pending updates require guest progress.

Kore's KFU1/KFA1 records carry bounded authenticated user text and outcomes;
FrontendUpdateResult is inference request kind 5, separate from a prompt's
terminal answer. The resident frontend now consumes updates for the exact live
intent. It serializes broker planning, offers only that agent's existing grants,
waits for native pause, applies the broker proposal through the real resource
transaction, resumes, and reports Applied only after the exact resume receipt.
Uncertain native effects enter the existing failure/retirement path. Executable
grant/revoke now uses a private mutation handshake: grants transfer an
execute-only endpoint, revocations remove and retire the worker's endpoint,
and policy commits after the exact native acknowledgement. Existing tools and
consumed-use counters stay in place. Budget changes remain unimplemented and
are refused. Full live-authority acceptance is still open.

Host delivery waits for already-claimed initial planning and backend readiness;
new prompt claims wait for update result receipt. Cancelled update planning keeps
its exact request until terminal testimony. Lost shared control exchanges retain
the lane and request instead of discarding their lifecycle metadata; complete
link failure recovery remains outstanding.

## Authenticated live update API (2026-09-15)

The real KoreVmRuntime now connects the session owner to LinkCore's update API.
Submit through the existing authenticated loopback HTTP boundary:

```http
POST /api/v1/sessions/{session}/prompts/{prompt}/updates
Content-Type: application/json

{"api_version":1,"prompt_id":"<prompt>","update_id":"<unique-id>","text":"Revoke input."}
```

The prompt must currently be running, without cancellation or a published answer.
The native broker also requires a claimed active worker. The body prompt ID must
match the path. Text is limited to the current broker planning contract's 128
UTF-8 bytes, nonempty and without NUL. Oversized direct guest updates are refused
without terminating an otherwise live prompt. This is a deliberate existing
planning limit, not a claim that the larger transport bound is usable here.

There is one pending update per prompt and at most 32 retained update records per
session. The record binds update ID to the full original request: exact retries
return current state; changed text or prompt with the same ID conflicts. HTTP
acknowledgement means queued, not Applied. Runtime dispatch is one-shot. Results
are fetched only for the exact intent and host revision, after the separate guest
receipt is queued (or generation closure resolves receipt custody). A normal
prompt completion waits for its pending update to settle. VM/link shutdown is
checked once more for a known outcome before marking remaining dispatched work
Failed/unknown. Undispatched work ends Cancelled; it is never guessed Applied.

```http
GET /api/v1/sessions/{session}/prompts/{prompt}/updates/{update-id}
```

GET and idempotent POST return request, status, revision when known, and message.
States are queued, dispatched, applied, refused, failed and cancelled. The existing
session event stream records separate `authority_update` events for acceptance
and terminal outcome. These events cannot satisfy response publication or become
a second final assistant/error response.

The native UI now offers an explicit **Update permissions** button next to
Cancel while a prompt is active. Enter a permission request in the composer and
use that button; ordinary Send/Enter keeps its existing chat behavior. Updates
have their own status below the composer and entries in Working. API acceptance
is shown as queued; only a correlated terminal result with a valid revision is
shown as Applied. Cancellation, refusal and unknown failure stay distinct.

The UI keeps the exact session, prompt, update ID and submitted text after an
uncertain HTTP response. **Retry update** resends that request even if the user
has since edited the composer. A confirmed acknowledgement clears only the
unchanged submitted draft. Delayed replies from a previous submission cannot
clear a new draft or affect a replacement prompt/session; streamed terminal
outcomes cannot regress when an older queued acknowledgement arrives. Another
client's pending update disables submission until its terminal event arrives.
A previous prompt's permission status is hidden when the active prompt changes.

Four focused UI tests cover these request/result races, bounds and display
semantics; the existing chat draft/acknowledgement regression also passes.
Native-renderer fixture captures at 480 and 1100 pixels were visually inspected
for retry controls, wrapped status prose and applied Working rows. The fixture
capture mode is `VKORE_UI_PERMISSION_UPDATE=uncertain|applied` with the existing
`VKORE_UI_CAPTURE_FAST` test helper. These captures verify layout with fixture
state; real guest/model/native UI acceptance remains pending.

Focused checks pass: four existing host update/sequence tests; three API/bridge
unit tests (bounds, retry identity, cancellation versus uncertain dispatch, exact
receipt/backpressure and independent result custody); and one real loopback HTTP
harness test reaching a running runtime callback. The latter uses an explicitly
identified runtime fixture and does not claim guest capability execution. Native
init/worker compilation and guest live-control/resource tests are recorded in the
Kore ledger. Real QEMU/model/native UI acceptance, full native budget enforcement,
recovery, committed library exports/pins and canonical artifacts remain required.
No broad suite or real guest/model/UI acceptance ran for this checkpoint.

## Native tool updates and retirement recovery (2026-09-15)

The guest validates each live tool change against the paused agent's current
contracts and exact committed grant. The one-record shared catalog carries
metadata; a grant's execute-only capability travels on the retained private
configuration receiver. Duplicate mutation notices cannot consume another row
or endpoint. The parent waits for the worker's mutation acknowledgement before
committing policy, and for resume testimony before reporting Applied. A tool
proposal can now carry up to twice the selected-tool capacity (currently eight
changes): revoke every old endpoint and grant every slot anew. Revocations run
before grants even if the proposal lists them later, permitting a full namespace
replacement. Every new grant must be prepared before publication. One exact
generation/revision identifies the entire batch and one acknowledgement confirms
all its operations. Existing entries retain consumed-use counters; the aggregate
call budget is not reset. A partial native failure keeps the worker paused for
retirement and does not commit policy or report Applied. This is not native
rollback of an already performed prefix.

Filesystem changes now also form a batch: up to twice the selected-resource
capacity (currently 32 changes) can replace every native grant. Resource revokes
retain their exact native GrantId and execute before new resource grants. All
new resources are prepared together using the final selected tool set, including
tools granted or revoked in the same proposal. This permits a writable output and
its write executable to be added together; a revoked resource cannot supply
rights required to prepare a new grant. Unchanged resource grants retain their
rights. Mixed filesystem/tool updates publish one combined record and wait for
the same acknowledgement before committing policy. An absent resource component
preserves aliases; an explicitly empty catalog removes them all. The bounded
authority report queue fits every change plus a terminal marker.

Live tool-call allowance changes now use the same paused mutation and child
receipt, including budget-only updates. The allowance is a lifetime total:
lowering it below consumed usage exhausts it, and raising it grants only the
difference above usage. Replaying or restoring a limit cannot refund calls.
The native scope charges before Invoke may suspend and caps advertised tool
uses by its remaining total. Its outer loop retains the protocol history bound
instead of freezing the initial allowance. Live offers advertise the resident's
configured call and wall-duration ceilings, preserving the exact agent's other
admitted limits, so an earlier reduction can later be raised. Other live budget
fields remain refused pending their native implementation.

Wall duration is anchored to the original start time carried by startup v6;
updates cannot restart elapsed lifetimes. The parent checks the duration before
native mutation and waits for child acknowledgement under both old and new
deadlines before committing lifecycle and policy. Expired replacements and late
receipts refuse, retaining partial native effects for retirement. The child
checks expiry at control/inference, capability receipt and invocation boundaries;
its successful-result selection also checks the lifetime. Parent-run selected
resource bind/share/receipt/revoke commands receive the absolute deadline.
Cleanup retains its separate grace and requires confirmed native closure.

Sixty-four focused guest tests and native init/worker compilation pass for this
checkpoint. The actual Broker and SPSC fixtures cover missing acknowledgements,
consumed calls, birth-preserving duration changes, late receipt refusal, unchanged
cleanup grace, replay and strict wire versions. The runtime clock is sampled at
native parks: these checks are cooperative observations plus a parent watchdog,
not independently preemptive kernel quotas or proof that blocking native calls
are bounded. Founded runtime acceptance remains required; existing images
predate startup v6.

Retirement quarantine now retries existing native cleanup every 100 ms, keeping
the original deadline, owners and request correlations. Time alone never makes
capacity reusable. The teardown observation also survives a full report queue
and is retried after earlier records drain. Route close and rotation now resume
their exact outstanding service request in bounded 5 ms observation slices.
The entire operation, slot, generation and request identity must match before
the channel can consume a late acknowledgement. An occupied route exchange
blocks other broker-channel calls until it settles; an already-consumed frontend
reply is delivered to its owner first. Backpressure permits a fresh attempt only
after the service explicitly refused it. Malformed/poisoned exchanges, shared-link
failure and other halted-state recovery still need implementation.

Frontend exchanges now retain their complete encoded envelope in the lane before
the first send. Claims, controls, terminal responses, retirement reports and live
update outcomes use bounded 5 ms service-acknowledgement waits and exact-request
continuation. An expired wait cannot replay a response or satisfy another call.
Service acceptance remains distinct from the later frontend publication receipt.
Explicit backpressure preserves the envelope for a fresh attempt; uncertain
delivery preserves it for resume. Matching malformed/terminal receipt failures
retain the original claim or response lifecycle rather than erasing its metadata.
The runtime exposes a read-only borrow of its actual outstanding wire request so
an older uncertain route cannot capture recovery of a newer frontend send.
Shared broker polling also uses bounded exact-request continuation. A pending
Poll is resumed before new sends, preserving its original event and lane identity
without dequeuing a second event. Transient observation failures keep the request
available for retry; malformed or poisoned exchanges remain failed. Permanent
refusal/disconnect and poisoned-pair reset still require recovery work.

Initial broker planning and live-update planning now retain their immutable
inference bytes and send progress in the owning frontend lane. Each dispatch
observes at most one inline or bulk service verb for 5 ms. An uncertain wait
resumes only when the actual outstanding wire request matches the complete
operation; a pre-publication wait can safely start fresh when the client is still
idle. Bulk offsets advance only after exact service acceptance, and explicit
backpressure retries the same bytes. A sibling frontend send or broker Poll
cannot consume the inference acknowledgement. Offers, intent and update revision
stay in their existing owners until the request is accepted and planning replies
arrive. A service refusal before an upload opens needs no abort. After a partial
upload, the lane sends the exact BulkAbort and waits for the matching remote
BULK_ABORTED receipt before releasing that request. Service acceptance alone
cannot finish cleanup. The service waits for pending begin/chunk credits before
publishing the abort, preserving reply sequence order. Cancellation intent is
retained during this wait, and other lanes can progress while it awaits receipt.
Confirmed cleanup produces the affected prompt's failure/cancellation, or a
separate failed/cancelled live-update outcome while retaining its worker.
Malformed cleanup replies, disconnected/poisoned pairs and refused cleanup still
retain ownership and require broader link recovery. Other legacy upload callers
still require the same recovery audit.

The actual native worker now also retains inference upload state across bounded
observations, including tool-result continuation requests. Its status query,
frontend intent claim, inference send/poll and frontend result receipt all use
5 ms observation intervals. Dropped waits resume the original service operation
or mailbox dequeue; a partial refusal waits for its exact remote abort receipt.
Parent controls can be handled between inference observations. If a turn ends
with an unresolved upload or Poll, its root keeps that state and native owners
for parent deadline/retirement rather than publishing another request on the
occupied face. Frontend sends suspended before publication can now start safely
when the native pair is still idle, while an occupied pair requires exact wire
matching. Fourteen focused HostLink regressions, the executor timeout test and
native init/worker compilation pass. Real guest/model recovery is still untested.

The shared broker now observes authority report, cancellation and idle-readiness
service operations in bounded intervals. Exact pending operations take priority
over fresh sends and Poll. Report acknowledgement requires the original encoded
bytes; the report stays at its ordered queue front until accepted. Cancellation
send custody is separate from the awaited remote terminal event, so an
interrupted send is retried without losing the request ID. Cancellation observed
during an upload survives successful upload completion; an exact late abort
receipt can settle a stopped update without a redundant cancellation request.

Twenty-five focused HostLink tests, the executor timeout test, both authority
queue tests and native init/worker compilation pass at this checkpoint. Wrong
acknowledgement IDs poison the pair and cannot confirm publication. These are
protocol and compilation results. The host and guest pins/artifacts remain
provisional.

Native worker observation now runs before all shared-connection waits and the
failed-link guard. A lane retains its exact stop lifetime and retries task,
private-memory and parent exec/view cleanup even while a HostLink response is
missing. Normal completion retains the response-receipt/grace ordering; forced
failure/cancellation can stop native work while keeping the exact undelivered
response, policy and route-retirement obligations. No timeout authorizes reuse.
The current init loop still observes these at its steady dispatch cadence.

Exec-face retirement now uses bounded observations of the original held-admin
request. A late ack is resumed without republishing a recyclable face ID;
wrong-face resumes leave the original ack untouched, and terminal refusal
retains custody. Forty focused lifecycle/owner tests, three exec wire tests and
native init/worker compilation pass. Real stalled-connection guest acceptance
remains pending.

Exec now separates accepted retirement from physical completion. RetireClient
returns a nonzero service-issued token, and the parent retains it across bounded
ConfirmClientRetired observations. The service retains a tombstone until its
serving pair is detached, every held native resource closes, and the exact
confirmation is processed. Pending replies and legacy queued acknowledgements
cannot release parent custody; stale tokens cannot free a reused face. Busy
cleanup explicitly refuses without effect so a sibling can retry. Fourteen
focused owner/service/wire tests and native init/worker compilation pass for
this change. Real founded resource reclamation remains unverified.

The same audit corrected the wire decoder to accept the resident agent's explicit
empty filesystem rights when minting an exec client. The existing service path
then creates no default filesystem view or home; filesystem operations still
need the separately selected view. Unknown/oversized rights remain refused.
The codec tests cover this request, while the real capability-bearing mint needs
a founded guest (the host runtime cannot allocate native retained grant slots).

Exec service construction now holds returned backing, roots and endpoints in
its persistent client row before mapping or further IPC. Failed construction
retains uncertain cleanup in that row, including the unused channel endpoints,
and the real maintenance loop retries it. Unpublished failures release their
claim only after closure; failed attachment preserves an already-issued face id
until the client's exact retirement confirmation. Fifteen focused tests and
native init/worker compilation pass after this change; native failure injection
and founded reclamation remain required.

Exec client construction is now a caller-owned ExecClientMint, retained in each
resident worker's Creating state. It keeps the service-authored face id and
partial native grants across interrupted observations; abort consumes its original
stage, skips later construction, and uses physical retirement confirmation before
closing local owners. Terminal bootstrap callers also retain explicit construction
owners. The former best-effort cleanup calls are removed. Native worker mint
observations use the original startup deadline and complete before START.

Ordinary SvcClient calls now reserve the pair during prepublication post
preparation. Another call cannot overtake a cancelled preflight, even with identical
request bytes. Exact resume finishes and publishes the original operation once.
Seventy-two focused owner/runtime/exec/HostLink tests pass, and native init/worker
compilation is checked with the local composition. These fixtures do not prove
native retained grants were successfully minted or reclaimed.

Invoke and the other non-parking return transfers now use the existing synchronous
affine commit boundary, so transfer classification and reply correlation are
installed before the next cancellable await. Thirty-one focused runtime client
tests and the native init/worker check pass. This does not yet fix caller-owned
input retention during earlier grant-post preflight: ToolProcess and worker
founding still need their typed invocation continuations, followed by the other
native launch consumers.

Remaining work includes affine return-bearing preflight and Invoke custody,
unobserved mm supply/filesystem registration, permanent connection/poison recovery,
other bounded native operations and real guest/model cancellation acceptance.
The source graph and artifacts remain provisional.

Native CPU-budget auditing confirms that the current admission checks do not
install CPU quotas. Scheduler runtime measurement and native CpuShare authority
are not quota enforcement. Full CPU/memory/time and the other live budget dimensions remain an
implementation requirement, including aggregate accounting for the actual
process worker pool and tool descendants.

Focused SPSC tests drop and resume waits at every inline/bulk stage, exercise
backpressure at each stage, reject different payloads with the same request ID,
check digest/byte completeness, and abort a refused partial upload without
publishing BulkFinish. Additional service tests prove credit ordering and request
retirement only on remote abort receipt, followed by admission of another request.
The executor's bounded reply test and native init/worker
compilation pass. These checks do not establish real guest/model recovery.

Focused guest tests cover full resource/tool replacement, final-tool rights
preparation, revoked-rights exclusion, the maximum combined catalog in one ring
row, empty versus unchanged aliases, one receipt and duplicate notices,
incomplete preparation, stale
and duplicate grants, strict batch encoding, ring backpressure, broker commit
failure, mutation identity/replay, startup/configuration compatibility, ordered
authority reports, retained cleanup custody and recovery retry bounds.
These protocol and ownership tests do not establish real founded
guest execution; the production QEMU/model/native UI acceptance remains pending.

Before the batch changes, focused release linking passed for svc-init (static x86_64 ET_EXEC with the
service image-base/link settings) and kore-agent-runner (x86_64 ET_DYN). Both
fresh binaries pass the real Kore loader's image and mapping-shape validator.
These are provisional outputs in the isolated guest target directory, built with
local native/libkore overrides. They have not replaced committed service images,
the archive, measured-boot digests or the canonical ISO. Boot and runtime acceptance
still require a complete fresh artifact set. Those linked images predate startup
versions 3–6 and the combined update path; they do not validate these newer edits.


Guest continuation checkpoint: affine input preparation and transfer now finish
without suspension before reply correlation. Tool/worker Invoke reply observation
uses the original absolute execution deadline. Cancelled/expired founding can
recover its original late task handle into the retained process owner, through
both worker teardown and parent maintenance; unrelated shared-exec replies remain
protected. The parent reaper validates the exact generation before recovery.

69 focused runtime/exec/agent-runner tests and native init + worker compilation
passed (guest logs `/tmp/vkore-port-996-invoke-recovery-focused-final.log` and
`/tmp/vkore-port-996-invoke-recovery-native-final.log`). These are protocol and
compile checks, not native late-handle/reclamation acceptance. Guest quotas,
remaining uncertain native resource operations, poisoned-connection recovery,
canonical exports/pins/artifacts and the full real-stack acceptance remain open.
Broad tests stay deferred until implementation is complete. No new artifact or
host dependency pin was promoted by this checkpoint.


Guest MM continuation checkpoint: WorkerAllocation now retains the exact pending
supply index and uses fallible MM observation plus exact bounded continuation.
Cleanup receives the original init MM pair, recovers any late root before another
await, and retires it through the existing native owner. Deadline-bearing worker
and parent command preparation carry their original absolute deadline through MM
observation. Non-started requests cannot claim another allocation's pending work.

Final focused coverage passed: 7 allocation, 61 runtime and 11 MM tests. Native
init + agent-runner compilation passed. Evidence is in the guest progress ledger
and `/tmp/vkore-port-996-mm-recovery-*-final.log` plus the native log. These checks
do not prove real late-lease delivery/reclamation. Mapping uncertainty, other
legacy shared-MM users, poisoned-connection recovery, quotas and all final real
stack/pin/artifact acceptance remain open. No broad tests or promotion occurred.


Guest mapping checkpoint: worker backing and parent HostLink retirement mappings
now distinguish proved refusals from unknown publication. Unknown outcomes retain
an affine cleanup owner, arena name, address reservation and original backing;
only confirmed exact unshare releases that obligation. Proved refusals can retry
or clean up without permanently blocking the row. The shared whole-lease mapping
helper no longer reopens a reserved result after an unknown commit.

44 focused allocation/capability-retirement tests and final native init + worker
compilation passed (`/tmp/vkore-port-996-mapping-outcome-focused-final.log` and
`/tmp/vkore-port-996-mapping-outcome-native-final.log`). These are not native PTE
or real-stack acceptance. Carved-lease result custody, remaining recovery/budget
work, canonical library/pin/artifact promotion and real guest/model/UI verification
remain open. Broad tests were not run.


Guest carve checkpoint: native carved-lease results now distinguish proved
refusal from uncertain publication and retain uncertain arena names for cleanup.
Actual worker/tool ancestor and input preparation stops at its first failure,
retaining every completed member and the cleanup-only unresolved result. It does
not reissue a carve during cleanup or expose an uncertain result as a lease.

70 focused tests and native init + agent-runner compilation passed (guest logs
`/tmp/vkore-port-996-carve-outcome-focused.log` and
`/tmp/vkore-port-996-carve-outcome-native.log`). Real native reclamation remains
unverified. Endpoint derivation/view registration, remaining recovery and budget
work, committed library/pin/artifact promotion, full gates and real guest/model/UI
acceptance remain open. No broad tests or promotion occurred.


Guest endpoint checkpoint: send/receive derivation now has typed refusal versus
unknown-publication outcomes. Worker/tool filesystem inputs, grant-target
auxiliaries, initial allocation bindings and ViewBinding retain unresolved local
results until native cleanup closes them. Send-selector and receive-access rights
remain unchanged. The cleanup owner is shared with unmapped local lease results.

79 focused runtime/agent-runner tests and native init + worker compilation passed
(`/tmp/vkore-port-996-endpoint-outcome-focused-final.log` and
`/tmp/vkore-port-996-endpoint-outcome-native.log`). DagFS compiled in the focused
invocation but had zero selected tests. Real endpoint reclamation is unverified.
Configuration publication and two resident temporary-source derivations remain
to audit, along with the original recovery, budget, promotion and real-stack
acceptance requirements. No broad tests or artifact/pin promotion occurred.


Guest configuration checkpoint: filesystem and executable members now transfer
atomically from their retained source pivots, removing temporary derived endpoints
from the async publication/retry loop. All six affine ChanTx operations settle
through non-parking commit before another cancellation point. The persistent
publication owner still prevents replay of a partial package.

61 focused tests and native init + worker compilation passed (guest logs
`/tmp/vkore-port-996-configuration-affine-focused.log` and
`/tmp/vkore-port-996-configuration-affine-native.log`). Native delivered-rights,
partial-publication cleanup and reuse remain unverified. Two resident temporary
source paths and the original recovery/budget/promotion/real-stack requirements
remain open. No broad tests or canonical artifact/pin promotion occurred.


Guest resident-source checkpoint: discovery and selected-resource binding now
borrow the exact retained inventory source. Workspace refresh explicitly selects
the exec context and derives directly into ToolProcess's existing input custody,
including uncertain results. Selected-resource and ordinary tool calls retain
explicit held-source selection; no refused view falls back to the exec context.

43 focused agent-runner/exec tests and native init + worker compilation passed
(guest logs /tmp/vkore-port-996-resident-source-focused.log and
/tmp/vkore-port-996-resident-source-native.log). Real endpoint retirement/reuse
remains unverified. View registration/native IPC ownership, shared-MM/recovery,
aggregate/live budgets, canonical library/pin/artifact promotion, broad gates and
real guest/model/UI acceptance remain open. No broad tests or promotion occurred.


Guest channel-mint checkpoint: unknown mint outcomes now return affine cleanup
custody for all three original arena names. View bindings, worker control/config
owners, and exec filesystem/power partial peers retain that custody until native
closure; a closed root alone does not release the endpoint names.

43 focused tests passed (37 runtime/agent-runner and 6 exec cleanup/retirement),
as did final native init + worker compilation. Evidence: guest
/tmp/vkore-port-996-channel-custody-focused.log,
/tmp/vkore-port-996-channel-custody-exec-focused.log and
/tmp/vkore-port-996-channel-custody-native.log. Real channel reclamation remains
unverified. Remaining exec/runtime/init mint callers, registration/recovery/MM,
aggregate/live budgets, canonical promotion and full real-stack acceptance remain
open. No broad tests or canonical artifact/pin promotion occurred.


Guest exec-channel checkpoint: partial/uncertain private-doorbell and cleanup-
canary creation now stays in the existing spawn rollback ledger. Executable-peer
setup retains its template and failed channel in the service row, including post
failure, until ordinary maintenance closes them. Retiring rows are excluded from
catalogue selection.

13 focused exec tests and final native init + worker compilation passed (guest
/tmp/vkore-port-996-spawn-channel-focused.log and
/tmp/vkore-port-996-spawn-channel-native.log). Native failure cleanup/reuse is
unverified. Client/bootstrap minting, pre-row template cleanup, terminal Found
outcome audit and the original MM/recovery/budget/promotion/real-stack requirements
remain open. No broad tests or canonical artifact/pin promotion occurred.


Guest client/template checkpoint: exec client-face construction now retains
uncertain channel results in its retiring face row. Install and TreeAcquire
reserve rollback capacity before template minting and preserve returned partial
templates in the existing ledger. Bootstrap mint refusal was traced to terminal
startup NACK/SELF exit rather than runtime reuse.

8 focused exec tests and final native init + worker compilation passed (guest
/tmp/vkore-port-996-client-channel-focused.log and
/tmp/vkore-port-996-client-channel-native.log). Real native cleanup/reuse remains
unverified. Terminal Found outcome handling, underlying image/registration/MM
recovery, budgets, canonical promotion and full real-stack acceptance remain
open. No broad tests or artifact/pin promotion occurred.


Native transport audit correction: TrapUnavailable proves the syscall was never
issued. Channel mint now reports NotIssued and releases its reserved names for
that case; unknown returned completion words still retain UncertainChannel.
6 focused runtime tests and native init + worker compilation passed (guest
/tmp/vkore-port-996-native-issue-focused.log and
/tmp/vkore-port-996-native-issue-native.log). Found's unexpected-completion and
caller recovery handling remains open; it was not changed. All prior recovery,
budget, canonical promotion and real-stack acceptance requirements remain open.
No broad tests or artifact/pin promotion occurred.


Native preflight checkpoint: endpoint derivation, lease carving and fixed-address
mapping now release unused reservations on proved NotIssued errors; unknown
returned completions retain their existing cleanup owners. 73 focused tests and
native init + worker compilation passed (guest native-local-refusal logs).
Found recovery and the original recovery/budget/promotion/real-stack requirements
remain open. No broad tests or canonical promotion occurred.


Heap refill checkpoint: the MM-backed source retains cancelled/failed supply
identity and resumes it before another request. Exec maintenance drives recovery,
and its other MM users wait for the retained heap request to settle. Four new
cancellation/error/demand protocol fixtures are included. 31 focused tests and
native init + worker compilation passed (guest heap-refill logs). Mapping-failure
custody, other MM consumers and all original recovery/budget/promotion/real-stack
acceptance work remain open. No broad tests or canonical promotion occurred.


Heap mapping checkpoint: the guest now retains failed backing/view/uncertain-map
owners together until exact cleanup succeeds, and uses explicit view ownership
for kernel-selected heap mappings. Image and compositor keep retries scheduled
without new allocation requests; exec distinguishes local cleanup from a pending
shared-MM operation. The existing deadline host now awaits async deadline work
and retains a timer for its own peer/admission cleanup. 127 focused tests and
native init/worker/image/compositor/time compilation passed in the guest
heap-map logs. Host fixtures do not prove native mapping/timer recovery; founded
failure/reuse gates remain required. Non-heap MM coordination, Found and other
publication recovery, aggregate/live budgets, canonical export/pin/artifact
promotion and the full real-stack acceptance remain open. Broad tests are still
deferred until all implementation is complete.


Exec MM checkpoint: its five non-heap supply users and telemetry now share one
persistent recovery record beside the actual MM client. Cancellation retains
original request identity; late supplies are held for retirement; heap growth
waits for non-heap recovery and can resume its own pending request. Stats uses
strict grant-free validation and exact resume. 41 focused exec/MM tests and
native exec/init/worker compilation passed (guest mm-recovery logs). Supply
lifetime budgeting and poisoned-pair recovery remain open, as do non-heap
mapping cleanup, init MM coordination, Found/publication recovery, aggregate
budgets, canonical promotion and full real-stack/native acceptance. No broad
tests or artifact promotion occurred.


Exec mapping checkpoint: filesystem/power peer setup and the filesystem image
copy destination now retain their actual mapped or uncertain-view owner beside
the backing lease. Cleanup unmaps before backing revoke, and the shared transient
window checks these records before reuse. Image-copy custody survives filesystem
awaits in TaskManager. 61 focused runtime/exec tests and native exec/init/worker
compilation passed (guest peer-mapping logs); actual native failure/reuse evidence
is still required. Export/client/source/boot mappings, other IPC cancellation,
init MM coordination, Found recovery, budgets, canonical promotion and full
real-stack acceptance remain open. Broad tests remain deferred.


Named mapping checkpoint: exec client faces now retain explicit serving-view
owners through host detach and exact cleanup; persistent filesystem-source
setup retains failed mapping/backing custody and moves the successful pair into
its durable source owner. TaskManager has no remaining map_held_rw calls. 61
focused tests and native exec/init/worker compilation passed (guest
named-consumers logs). Export/boot mapping recovery, other IPC cancellation,
init MM coordination, Found/poisoned-pair recovery, aggregate budgets, canonical
promotion and full guest/model/UI/platform acceptance remain open. No broad
testing or artifact promotion occurred.


## Exec archive export mapping and parent custody — 2026-09-15

The exact read-only export view and its MM lease now stay together in
TaskManager through filesystem copy, overwrite and immutable readback awaits.
The typed read-only fixed mapping preserves an uncertain exact-unshare owner;
cleanup must close the view and then its parent before success or transient
window reuse. Shared-window admission also settles abandoned peer and image
copy mappings. Maintenance retries export cleanup. Missing-lease filesystem
cleanup and seeding diagnostics were preserved.

Final observed evidence: /tmp/vkore-port-996-export-mapping-focused.log records
63 passing focused tests (45 runtime, 18 exec), including retirement and archive
seeding regressions. /tmp/vkore-port-996-export-mapping-native.log records
successful native svc-exec, svc-init and agent-runner compilation under the
local libkore composition. Both logs have terminal completion; git diff --check
passed. These checks do not prove founded mapping failure/reuse behavior.

Export request cancellation before receipt remains separate: the next change
will preserve its original IPC identity and late lease custody. Boot-page
mapping, other remote cancellation/Found recovery, quotas, committed library
exports and pins, fresh artifacts and real guest/model/UI acceptance remain
open. Broad suites remain deferred until all implementation is complete.


## Exec export request recovery — 2026-09-15

ArchiveExport now owns the original ImageName before publication and the actual
export lease/view after receipt. All three archive consumers acquire through
this record. Maintenance resumes only that exact export with a 5 ms observation
slice, stores a late owner synchronously, then retires it before reuse. A new
request cannot adopt even an identical foreign operation. Cancellation before
publication clears only upon an idle observation; transport/timeout/poison
errors retain the original identity. Fresh exports preserve the existing
unbounded observation contract. Failed typed replies now report EXPORT; a
success without its mandatory lease is rejected by RingImage, not passed to
the filesystem consumer as WRITE_NO_SLOT.

RingImage's typed export methods validate nonempty bytes, exactly one retained
lease and no foreign deliveries; malformed ownership stays in pair quarantine.
Bounded recovery uses SvcClient.resume_exact_bounded on the original encoded
Export request. No new service pair or authority is manufactured.

/tmp/vkore-port-996-export-recovery-focused.log: 10 passing focused tests, seven
exec cancellation/seed tests and three actual image-client shape/preflight/
foreign-reply tests. The protocol lifetime fixtures return only grantless
refusals; they do not fabricate native capabilities or prove physical cleanup.
/tmp/vkore-port-996-export-recovery-native.log: native svc-exec, svc-init and
agent-runner compilation passed. Both handles exited zero; git diff --check
passed. Actual founded late-lease delivery/cleanup and repeated reuse remain
required. Broad tests and promotion remain deferred.

Next inspected scope is boot-page carve/mapping recovery. Other remote
cancellation, Found/poisoned-pair recovery, aggregate budgets, committed
exports/pins/artifacts and full guest/model/UI/platform acceptance remain open.


## Supply-backed boot stamp and invocation input custody — 2026-09-15

The export-recovery turn was progress. Source inspection confirmed the boot
supply path still carved into a fixed scratch slot, reduced map completion to
a boolean, and attempted parent revoke on map failure. stamp_boot_page now
uses BootStamp with the existing typed carve and LocalMapping/MappedSupply
owners. It stores the actual carved lease or uncertain carve, then the exact
successful/uncertain mapping. No raw slice is constructed before successful
geometry validation. Exact mapping cleanup precedes carve retirement, and the
original held invocation source remains protected until both finish. Fixed
SPAWN_SELF_VIEW_RESULT is no longer reused for a preliminary boot mapping;
SPAWN_BOOT_WRITE_SLOT remains only as a reserved coordinate to keep the frozen
SSA layout stable. Two dynamic names are transiently required for a boot stamp.

Failed invocation inputs now have a persistent cleanup mask over exec's actual
fixed convey band. Each native revoke is retried until closed; the mask clears
individual successful inputs without touching another live landing. The boot
source is excluded while its descendants remain. Admission selection, contended
sweeps and receive reposting skip protected/pending landings. A stamp failure
records input cleanup before awaiting remote rollback. Maintenance retries boot
cleanup, then eligible inputs and reposts. All shared transient-window consumers
also wait for outstanding boot cleanup. This does not fix every other
cancellation boundary in remote rollback or Found completion.

Evidence: /tmp/vkore-port-996-boot-custody-focused.log has 50 passing tests
(43 runtime retirement/uncertainty tests and seven exec boot/input/client
retirement tests). New tests exercise protected-parent ordering, partial
retirement retries without touching unrelated live landings, and honest host
carve refusal without fabricated native owners. Native svc-exec, svc-init and
agent-runner compilation passed in /tmp/vkore-port-996-boot-custody-native.log.
Both handles exited zero. Subsequent edits only formatted changed functions;
git diff --check passed. Actual founded carve/map/unshare failure and capacity
reuse still require guest evidence. No broad suites or artifact promotion ran.

Remaining boot variant: the older non-supply path still takes and maps a
detached LeasedToken through map_view. It needs a separate native ownership
audit: runtime/session/dispatch.rs revoke_prepare explicitly rejects a view
whose lease_state is Some from exact unshare, directing detached/transferred
grants to RETURN. A blind replacement with another exact-unshare wrapper
would retain the wrong cleanup protocol. Verify its real caller/endowment and
parent rights, then implement the appropriate mapped-view return/custody path
without broadening authority or silently dropping a supported boot mode.

Full scope remains active: other FS/image and rollback IPC cancellation, init
MM coordination, Found and poisoned-pair recovery, aggregate live budgets,
committed library exports/pins/provenance, fresh artifacts, all architecture
and serial guest gates, real model/native UI and applicable platform evidence.
Broad testing stays deferred until all implementation is complete.


## Boot protocol compatibility disposition — 2026-09-15

The preceding boot-custody turn was progress. The detached-view question was
resolved from actual producers and current native authority: every production
SpawnSpec producer in the port (init, shell, agent process and window-session
layout) sets BOOT_FROM_SUPPLY. The old #933/Linux-handoff guest also uses that
form in all production launchers. The selected libkore source exports the ABI
but contains no separate SpawnSpec producer of the detached pair.

The historical wire form supplies a writable detached view and a read-only
sublease for the child's share. Current exact unshare requires a directly
installed view with lease_state None, while detached receiver mapping uses
lease_state Some. RETURN is gated on MODIFY and immediate-parent identity;
the read-only sibling supplies neither the required writable authority nor a
proof of that relationship. Restoring the old map-then-best-effort-unshare
branch would introduce a live mapping that exec cannot reliably retire.

That obsolete form is now explicitly refused before provisioning. It still
decodes, so an old wire caller receives BadBlock rather than disappearing into
wire rejection. RingExec checks before affine input publication and returns
InvokeError::NotMoved with the original inputs. bind_invoke_spec repeats the
guard for direct protocol callers, and stamp_boot_page contains only the
supply-backed typed owner path. ABI and SpawnSpec comments describe this
compatibility boundary. Required old and current vkore launch behavior is
retained; there is no claim that external callers of the old form remain
supported. This is an explicit superseded-protocol disposition, not native
detached-view cleanup support or a new kernel mapping/return authority.

Focused evidence: boot-contract-focused.log has eight passing exec binding,
wire and input-custody tests; boot-producers-focused.log has two passing real
agent/window record-builder tests; boot-shell-focused.log has two passing
shell/boot-plan tests (all under /tmp/vkore-port-996-). Binding fixtures now use
the actual supply form and check that page offsets are not translated as
context indices. The new negative test proves historical syntax decodes but
cannot bind a live invocation. Native init, exec, agent-runner and shell
compilation passed in /tmp/vkore-port-996-boot-contract-native.log. An initial
native command used an incorrect shell package name and was corrected to
kore-shell-bin; an initial new test missed a qualified constant and was fixed
before passing. All observed handles exited. Subsequent edits only formatted
changed service functions; git diff --check passed.

No broad suites, fresh ISO, commits, PRs, issue closure or canonical promotion
occurred. Founded mapping failure/reuse and real stack acceptance remain
pending. Next live recovery work is the shared image registration/request
identity and its actual source custody, alongside the other FS/rollback
cancellation gaps, init MM coordination, Found/poisoned-pair recovery and live
budgets. Committed exports/pins/artifacts and the full guest/model/UI/platform
acceptance remain required; broad tests wait until all implementation finishes.


## Image registration recovery and provider rollback checkpoint

Verified the previously completed registration work against the current tree
and terminal logs before continuing. Exec's Registration record owns the
unmapped source synchronously before the request future exists and keeps the
original image name, source metadata and request identity across cancellation.
The concrete RingImage path uses exact retained transfer and bounded exact
resumption. A pre-transfer failure retains the source for local retirement;
a missing correlation after transfer is not treated as rollback. Successful
registration remains cleanup-owned until the caller validates and accepts it
synchronously. A late or unusable successful result is retired before another
registration or latest-generation consumer can proceed. Exec is the sole
registrar and name-retirement owner on the fixed image control pair.

The legacy registration resume methods also verify the original Register and
return route, refusing to consume unrelated outstanding replies. Registration
completion checks require a valid Registered descriptor or a grant-free
Refused response. The retained-return runtime tests and actual scoped protocol
tests in /tmp/vkore-port-996-registration-focused.log passed: 2 runtime,
12 exec and 4 image tests. The prior native check of init, exec, agent-runner
and shell finished successfully in registration-native.log with the same
/tmp/vkore-port-996- prefix; no matching cargo process remained at inspection.

The provider audit found that a partially carved failed registration could
retain its rollback record indefinitely after replying Refused. ImageEntry
now distinguishes Live, RegistrationRollback and explicit Retiring states.
A failed carve records every owned earlier segment and its source before the
validation mapping closes. The deadline-driven service retries at most one
rollback record's bounded segment set per pass, advances past refusals, and
frees the record only after its owners report closure. Validation cleanup
must settle first. Explicit name/bound retirement also waits while validation
cleanup is pending. Background work selects only RegistrationRollback:
removing an explicit Retiring record could make the client's next name-based
retry retire an older valid generation, so those records remain client-owned.

The service maintenance deadline now covers validation cleanup and failed
registrations as well as its existing MM heap recovery. It remains armed
while any background owner is pending and stops after custody settles.
Three new catalogue tests cover refusal/retry fairness, freed-slot reuse,
protection of live and explicit-retirement generations sharing the same name,
and an empty catalogue. These fixtures contain no native capability owners;
they prove catalogue selection and retry behavior, not actual kernel cleanup.
The final focused run passed 13 tests (5 exec, 8 image) in
/tmp/vkore-port-996-registration-cleanup-focused.log. Native compilation of
svc-image, svc-init, svc-exec, kore-agent-runner-bin and kore-shell-bin passed
in /tmp/vkore-port-996-registration-cleanup-native.log. Both observed sessions
exited zero. git diff --check passed.

This does not complete issue #996 or the overall port. The provider still
uses lossy optional carve/finalization and validation-map wrappers on some
unknown native completions; those must retain explicit retry owners. Source
custody on validation rejection before catalogue admission also requires
further work. Founded failure/reuse evidence remains pending, along with the
other FS/private-context/rollback cancellation paths, init MM coordination,
Found and poisoned-pair recovery, live aggregate budgets, committed
libkore exports/pins/artifacts, and the full guest/model/UI/platform acceptance.
No broad suites, fresh ISO, commits, PRs, issue closure or canonical promotion
were performed. Broad tests remain deferred until all implementation is done.


## Uncertain image creation and validation custody checkpoint

Continued from the verified provider-rollback implementation; this turn made
implementation progress. The request-side and archive registration loop now
uses full typed outcomes for both plain segment carves and executable
carve/finalization. The previous optional wrappers could discard cleanup
responsibility after an unknown native completion.

The runtime's carve_owned_exec_window_outcome reserves both scratch and
executable result names before issuing the two-invocation session. Canonical
Ok at index zero plus the executable result shape permits normal success;
ordinary rollback statuses require an index inside that actual session.
Out-of-range, unknown and inappropriate parking completions retain both
original reservations in UncertainExecCarve. The concrete TrapTransport's
pre-syscall failure remains an explicit NotIssued refusal. Cleanup attempts
both names without exposing either as an executable or writable capability.
The one-/two-invocation classifiers share the same count-aware classifier;
existing named-map and single-result checks remain covered.

ImageLease now has cleanup-only variants for uncertain plain and executable
carves. Those variants return no grant slot and are never live. A failed
registration stores the uncertain result alongside every earlier successful
segment and its actual returned source in the existing background rollback
record. Its parent cannot retire until all retained child results close.
The remaining compatibility executable-carve wrapper delegates to the typed
outcome; no production callsite in the tree uses that optional wrapper.

The validation path also now uses map_owned_lease_at_outcome. ValidationCleanup
persists either the known mapped view or the unknown map owner and any returned
source that catalogue admission did not consume. An authentication/check/
admission refusal takes the source out of ServeHost before completing
validation cleanup. A mapping failure likewise retains the source before any
ordered cleanup attempt. The exact mapping must close before source retirement;
a refused source retirement does not replay a completed mapping. A new
registration cannot replace either pending owner. The service's existing
deadline drive retries these owners without another request. Explicit image
retirement and background rollback continue to wait for validation cleanup.
An incidental drop quarantines an unclosed source instead of revoking the
ancestor under an uncertain mapping.

Focused evidence: the initial carve run passed 20 tests in
/tmp/vkore-port-996-image-carve-focused.log. The combined validation run passed
22 tests (7 runtime, 5 exec, 10 image) in
/tmp/vkore-port-996-image-validation-focused.log. New tests cover two-result
completion classification, independent closure of both original reservations,
mapping-before-source order across repeated refusals, and avoiding mapping
replay during source retry. Runtime tests exercise inert reservation state;
validation tests exercise the actual cleanup-order helper without fabricating
native authorities. They do not stand in for a founded kernel witness.

Native svc-image, svc-init, svc-exec, kore-agent-runner-bin and kore-shell-bin
compilation passed both image-carve-native.log and image-validation-native.log
under the same /tmp/vkore-port-996- prefix. All four observed process sessions
exited zero. Subsequent edits were comments and formatting of selected changed
functions; git diff --check passed. The new image validation module is
untracked and must be included when the stack is eventually assembled.

The registration carve/finalization and validation-wrapper gaps listed in the
preceding checkpoint are now implemented. Founded uncertain-completion,
mapping/refusal and repeated reuse evidence is still required. Image face-ring
and initial heap-root creation still use optional plain-carve wrappers and
need the same ownership audit in their actual surrounding lifetimes. The
remaining overall work also includes FS/private-context/rollback cancellation,
init MM coordination, Found/poisoned-pair recovery, live aggregate budgets,
canonical committed libkore exports/pins/artifacts, and full real
guest/model/UI/platform acceptance. No broad tests, ISO refresh, commits, PRs,
issue closures or canonical promotion occurred. The full goal remains active;
broad tests remain deferred until every issue's implementation is complete.


## Image face, heap seed and compositor carve custody checkpoint

The prior goal turn was progress. This turn inspected the remaining optional
image-service carves and their concrete heap consumers, then implemented their
cleanup ownership rather than relying on incidental Drop or task-lifetime
reservation loss.

Image face construction now occupies its face row before the ring carve and
stores every successful channel root/send, a receive endpoint whose immediate
retirement refused, and all three results of an uncertain channel mint.
Ring carves use the full typed outcome and retain an uncertain local result.
FaceSlot cleanup preserves refused receive custody before its parent channels,
attempts the independent uncertain channel/ring owners, and releases a row
only after every retained owner closes. Pre-publication failures are marked
for the deadline loop. A shared bounded row-selection helper retries one
eligible face and one eligible registration per pass, rotating past refusals.

A host attach failure can occur after delivery. Such a face now remains a
retiring occupied identity even if its local resources already closed; its
client's later FaceRetire carries a face id, so freeing/reusing that row early
could redirect the old retry to a new client. Explicit face retirement also
remains client-owned. Background cleanup selects only unpublished failures.
The new metadata tests cover this identity retention, refusal fairness and
protection of unrelated live clients without fabricating native capabilities.

MmSource now exposes carve_allocation_outcome. An uncertain carve from a
page-exclusive heap allocation returns its actual cleanup owner. Image face
backings keep that owner AND the allocation until native retirement confirms
closure, then return the pages to the named heap. A successful backing remains
cached across client generations. Initial image heap-root carving also uses
the typed outcome; uncertain and unconsumed known roots remain service-owned
and deadline-retried. Seed mapping failures continue to use MmSource's existing
FailedExtent source/mapping custody. ImageService has no remaining optional
plain-carve or allocation-carve production calls.

Tracing all allocation-carve callers found the compositor HeapLessor, whose
older optional result could let SurfaceOwners free pages under an uncertain
root. Both HeapLessor and PoolLessor now return NativeSurfaceRoot: a known root
can grant its subject, while an uncertain root exposes no subject and retains
native cleanup custody. The existing SurfaceOwners path already persists such
a root, refuses creation without a view grant, and retries retirement before
returning engine storage. Its existing deadline maintenance therefore owns
recovery without a new ledger. The new focused test proves that an unusable
root with refused cleanup holds its pages, an independent client uses other
pages, and only confirmed cleanup permits reuse with a fresh surface identity.
No production caller remains on MmSource's optional compatibility wrapper.

Verification: /tmp/vkore-port-996-image-face-focused.log passed 32 tests
(15 runtime, 7 image and 10 MM); /tmp/vkore-port-997-surface-carve-focused.log
passed all four surface-owner tests. Native compilation of svc-image,
svc-init, svc-exec, kore-agent-runner-bin, kore-shell-bin and svc-compositor
passed in /tmp/vkore-port-996-image-face-native.log and again after the
compositor adaptation in /tmp/vkore-port-997-surface-carve-native.log.
All four observed sessions exited zero. Subsequent changes only corrected
image face lifetime/safety comments. git diff --check passed.

The focused tests prove ownership state/selection and allocation-reuse policy;
they do not prove founded kernel failures, actual unknown completion recovery,
or real UI acceptance. The image face/backing/bootstrap and compositor carve
gaps from this checkpoint are implemented, but the overall issues remain open.
FS/private-context/rollback cancellation, init MM coordination, Found and
poisoned-pair recovery, live aggregate budgets, committed libkore
exports/pins/artifacts, and full real guest/model/UI/platform acceptance still
require work. No broad tests, rebuilt ISO, commits, PRs, issue closures or
canonical promotion occurred. Broad tests remain deferred until all issue
implementations are complete.


## Filesystem revision cancellation and bounded cleanup checkpoint

The previous goal turn was progress. This turn traced every exec caller of
version release and found affine PendingVersion/LentVersion values on local
future stacks during MM allocation, immutable reads and release. Cleanup also
took file/version phases out of TaskManager before awaiting their replies;
cancellation could discard that record despite the filesystem still owning
the corresponding handle or pinned revision.

Both register_fs_image and identify_matching_revision (used for archive/store
comparison) now store the identified version in fs_version_cleanup before any
subsequent await. Their shared read_fs_version helper synchronously records
the actual version and exact offset/length before polling ReadVersion. A
completed observation restores the Lent phase without yielding; cancellation
or a nonterminal transport result leaves Reading and its owner intact.
Release borrows that persistent owner and clears it only after Done.

RingFs gained bounded exact open, close, executable lend and version-read
observations plus borrowed retained version-release methods. The latter keep
the affine owner outside the cancellable future. Each bounded operation either
starts on an idle pair or resumes the identical encoded request, retaining the
original correlation on timeout. These methods use the Kore executor's clock.
Existing consuming release methods remain available; exec no longer uses them.

retry_fs_source_cleanup now borrows its Opening/Lending/Close and Reading
records across each five-millisecond observation. It captures late opens and
lends into normal cleanup, settles a pending read before releasing its version,
and releases the version before closing its file. No file phase is taken out
across await. The ordinary maintenance loop now drives this cleanup, and its
pending predicate includes both records, so an abandoned request no longer
requires a new registration to wake cleanup. Fresh source-open phases and
archive-comparison lends require an idle filesystem pair before recording a
new operation, preventing adoption of an identical foreign pending request.

The new protocol tests use stack-backed rings and the actual Kore executor.
One cancels a release while its transport future is pending, proves the same
version owner remains available, rejects another version without consuming
the queued reply, and completes the original release without republication.
The other checks every read coordinate and rejects release while that read
still owns the pair. Protocol identities in these tests are inert FS wire
values; they do not fabricate native capability owners.

Focused verification passed 15 tests (7 DagFS client, 8 exec) in
/tmp/vkore-port-996-fs-version-focused.log. Native svc-image, svc-init, svc-exec,
kore-agent-runner-bin, kore-shell-bin and svc-compositor compilation passed in
/tmp/vkore-port-996-fs-version-native.log, including the final idle guards.
All observed sessions exited. The first test compile exposed a Result
comparison requiring PartialEq on RingFsError; the assertion was corrected
without changing the error type. The next run correctly rejected a foreign
test waker for bounded-clock access; the tests were moved to the established
Kore-executor harness and passed. Subsequent edits only added API comments.
git diff --check passed.

This completes these version/source-cleanup implementation gaps, not issue
#996 or the overall stack. Other source-window discovery, durable write/root
handle and private-context/rollback cancellation paths still need audit.
Init MM coordination, Found/poisoned-pair recovery, live aggregate budgets,
canonical committed libkore exports/pins/artifacts and full real
guest/model/UI/platform acceptance remain required. No broad tests, fresh ISO,
commits, PRs, issue closures or canonical promotion occurred. Broad tests
remain deferred until all issue implementations are complete.


## Filesystem discovery and administration root custody checkpoint

Continued from the verified version-recovery work. This turn implemented the
remaining persistent phases around exec's source discovery and administration
root opens. Previously store_grant and open_store_directory could abandon
a pending list/open, including a late successful handle, without a cleanup
record or maintenance wake.

FsFileCleanup now includes ListingGrants and OpeningGrant with the original
grant and requested rights. store_grant records its listing before polling and
requires the source to be ready, its other cleanup records empty and the pair
idle. open_store_directory records the exact grant open before polling and
moves every returned handle into Close custody before inspecting its kind.
The app/lib/pkg root helpers share that path; they synchronously cache the
accepted directory handle and clear its Close record. A rejected directory or
an unaccepted result stays available to ordinary cleanup. No new grant or
authority mechanism was introduced.

RingFs gained bounded exact list-grants and open-grant observations and bounded
resume_attach_window. Attachment recovery never starts a new transfer, and
its geometry/return landing still belong to the request identity. Source
discovery now uses these bounded observations for pending attachment, listing
and grant opening. Fresh attachment/listing phases refuse a busy pair rather
than adopting another operation. Discovery cannot overwrite an existing
file/version cleanup record before its source is ready.

FsSourceState identifies only actual in-progress phases (Attaching, Listing,
OpeningGrant) as maintenance work. TaskManager's deadline predicate includes
them, and maintenance continues those phases after independent source cleanup
settles. Unattached, settled Attached and Ready states do not create a perpetual
discovery timer. Late source-root success is retained as Ready; a wrong-kind
handle is transferred into Close cleanup before discovery resets.

Focused tests passed 17 cases (9 DagFS client, 8 exec) in
/tmp/vkore-port-996-fs-discovery-focused.log. New stack-backed protocol tests
cancel an administration-grant open during its transport await, reject changed
grant/rights and an unrelated listing without consuming the late reply, then
resume and close the exact returned handle. They also verify list window
correlation and prove bounded attachment resume cannot publish a replacement
transfer or consume an outstanding listing. These are logical FS identities
and real client/executor code, not native capability fixtures.

Native compilation of svc-image, svc-init, svc-exec, kore-agent-runner-bin,
kore-shell-bin and svc-compositor passed in
/tmp/vkore-port-996-fs-discovery-native.log. Both observed sessions exited zero.
Subsequent edits only formatted two maintenance predicate expressions;
git diff --check passed. No broad suites or boot/model/UI gates were run.

The source-discovery and administration-root cancellation paths described here
are implemented. The overall issues remain open: durable writes, projection
directory handles and private-context/rollback cancellation still need work,
as do init MM coordination, Found/poisoned-pair recovery, live aggregate
budgets, committed libkore exports/pins/artifacts, and complete real
guest/model/UI/platform acceptance. No fresh ISO, commits, PRs, issue closures
or canonical promotion occurred. Broad tests remain deferred until all issue
implementations are complete.


## Filesystem projection cancellation checkpoint

Implemented exec's app-projection request/handle custody on the current port
worktree. The persistent TaskManager Projection owns the original OpenAt,
Create-directory, Link or Close phase and at most three connection-bound FS
handles: source plus old/new directory, or source plus final directory and
existing projection leaf. This bound follows the actual projection traversal.
The cached administration root is never entered into the transient ledger.

All production seed_app_projections, open_projection_directory and
link_projection calls now use this ledger. Every returned handle is captured
synchronously before another await, including wrong-kind and existing-edge
results. Cancellation leaves the phase and acquired handles in TaskManager;
maintenance first resumes the exact pending request, then closes the retained
handles. Failed closes retain their handle. A prepublication cancellation on
an idle pair abandons its unissued request. Fresh operations refuse an already
busy pair, including a foreign request with identical coordinates. Successful
namespace links/directories survive cleanup and are validated by the ordinary
idempotent seed retry. No native capability is fabricated from an FS handle.

RingFs now supplies bounded create_bounded (plain objects) and link_bounded
observations through the existing exact-request client correlation. These
preserve all request coordinates. Recovery observations use the same 5 ms
budget as adjacent filesystem cleanup. TaskManager maintenance_pending includes
projection state, and retry_fs_source_cleanup settles it before source reuse.

Focused verification: 4 new projection tests passed in
/tmp/vkore-port-996-projection-focused.log, using the actual Projection,
RingFs/SPSC protocol and Kore executor with a named stack-backed ring window.
They exercise cancellation of open and subsequent close, late handle custody,
all create/link coordinates, bounded handle admission, failed-close retention,
foreign identical request refusal, and unissued-operation cleanup. These are
protocol tests, not native kernel capability or on-metal acceptance evidence.
9 existing DagFS exact-attachment/discovery/version recovery tests passed in
/tmp/vkore-port-996-projection-dagfs.log. The DagFS command explicitly enabled
runtime; an initial invocation without that feature selected zero tests and
was not counted. The new fixture's initial import errors were corrected before
the successful x86_64-apple-darwin test run.

Native compilation of svc-image, svc-init, svc-exec, kore-agent-runner-bin,
kore-shell-bin and svc-compositor passed in
/tmp/vkore-port-996-projection-native.log. Observed build/test sessions are
terminal with successful final runs. git diff --check passed. No broad suite,
boot/model/UI gate, fresh ISO, commit, PR, issue closure or promotion occurred.

The full goal remains open. Next filesystem work includes durable writes and
deferred-commit progress, private-context/rollback cancellation; remaining
integration work still includes init MM coordination, Found/poisoned-pair
recovery, live aggregate budgets, committed libkore exports/pins/artifacts,
and real guest/model/UI/platform acceptance. Broad tests remain deferred until
all issue implementations are complete.


## Executable seed-write cancellation checkpoint

Implemented persistent SeedWrite custody for exec's create/overwrite member
paths. TaskManager now retains the exact DeferCommits, typed Create/OpenAt,
Resize, WriteAt, Close or DetachEdge request across every await, together with
the returned file handle, an actually created fresh edge and the potentially
open commit window. write_member and overwrite_member share seed_member_write;
write_opened_member routes all resize/chunk calls through that persistent owner.

Maintenance and source admission settle this owner before projection/version
cleanup or reuse of the FS data window. A cancelled chunk reads only the
permanently owned FS scratch window, not the archive export; recovery finishes
that exact request before closing the handle. The source export may therefore
retire independently after cancellation. A fresh incomplete member is detached
before closing deferral; an existing object's identity/links are preserved.
Only the uninterrupted copy loop certifies completion after every chunk replied.
A late abandoned final chunk does not silently certify a new seed. Failed
closes/detaches/commits retain their cleanup state for the next observation.

The dual mounted-store router can open deferral in one store and then refuse
the other. SeedWrite marks possible deferral before issuing the opening request,
so even a terminal refusal requires the closing request. A foreign busy pair
is refused before acquiring that responsibility. Idle prepublication recovery
never republishes an abandoned mutation. Closing an already absent fresh edge
accepts NotFound as proof that no such edge remains.

RingFs gained bounded exact create_typed_bounded, defer_commits_bounded,
write_at_bounded, resize_bounded and detach_edge_bounded methods. The previous
plain create_bounded delegates to the typed implementation. No wire opcode or
native capability representation changed.

Corrected the former seed-writer comment claiming deferral was an unpublished
replacement transaction. The protocol and provider evidence say it batches
persistence only: successful intermediate mutations remain visible through
other live routes, and cancellation does not roll back an existing object's
partial successor. This checkpoint fixes cancellation custody/cleanup; it does
not implement isolated atomic replacement. The existing mandatory immutable
readback still decides whether a completed copy may be stamped as seeded.
Any stronger replacement isolation requirement remains unproven here.

Focused verification passed 19 tests on x86_64-apple-darwin: 6 new SeedWrite
cases (/tmp/vkore-port-996-seed-write-focused.log), 4 projection cases after
sharing their named stack-backed RingFs fixture
(/tmp/vkore-port-996-seed-write-projection.log), and 9 DagFS exact attachment,
discovery and version cases with runtime enabled
(/tmp/vkore-port-996-seed-write-dagfs.log). New tests exercise deferral
cancellation and partial-store refusal, late typed creation, original write
and resize coordinates, incomplete-new-edge cleanup, preserved existing
identity, complete-copy commit retries, and foreign identical deferral refusal.
They use actual client/executor code and logical FS identities, not fabricated
native capability owners or on-metal evidence.

Native compilation passed for svc-image, svc-init, svc-exec,
kore-agent-runner-bin, kore-shell-bin and svc-compositor in
/tmp/vkore-port-996-seed-write-native.log. All observed handles are terminal;
final test/native sessions exited zero. Later production edits were formatting
and comments only; git diff --check passed. No broad tests, guest boot/model/UI
runs, fresh ISO, commits, PRs, issue closures or canonical promotion occurred.

The overall issues remain open. Private-context/rollback cancellation, init MM
coordination, Found/poisoned-pair recovery, actual aggregate live budgets,
committed libkore exports/pins/artifacts and complete guest/model/UI/platform
acceptance remain. Continue auditing other mutable namespace and system seed
lifecycle requests rather than treating this checkpoint as all FS work complete.
Broad tests remain deferred until all issue implementations are complete.


## Retained rollback and reconciliation owners checkpoint

Removed the take/await/reinsert pattern from exec's remote cleanup. The
retire_retained helper borrows a persistent Option owner while the cleanup
future runs and clears that slot only after the real cleanup function reports
complete. Cancellation drops the borrow, not the face or peer. Production
retire_row, retry_face_reconciliation and executable-template retirement all
use this helper. Image-face reconciliation also borrows its record in place,
so the actual canary and remote-closed phase stay in the row across awaits.
The old settle_rollback_owner helper and its misleading ownership rationale
were removed. Search confirms no entry.face/fs_peer/power_peer.take(),
reconciliation.take() or peer.template.take() remains in this module.

Retirement helpers now receive their concrete image/filesystem/power client
separately from the borrowed owner, instead of requiring all of TaskManager.
The unknown-admission uniqueness check explicitly excludes the current row;
this matters now that the owner remains in the ledger. Both FS and power
refuse mailbox adoption when another id-less pending peer exists. Existing
canary-death and remote detach-confirmation ordering remains in force; local
roots are retired only after the remote serving mappings are confirmed gone.

Added bounded exact lifecycle observations: RingImage retire_face_bounded and
retire_bound_bounded validate grant-free retirement/refusal replies;
RingFs and RingPowerAdmin take_admission_bounded, retire_peer_bounded and
confirm_retired_bounded preserve the original request identity. A different
verb or generation gets Outstanding without consuming the queued reply.
Power cleanup also rejects unexpected grants. Observations use 5 ms budgets.
RingFs checked_bounded now delegates to the common exact bounded call, keeping
its service-refusal conversion unchanged. Existing unbounded public APIs were
not removed; exec cleanup now uses the bounded ones.

Focused verification passed 25 tests on x86_64-apple-darwin: 9 selected exec
retirement tests (/tmp/vkore-port-996-retained-rollback-focused.log), 9 DagFS
client recovery tests with runtime enabled
(/tmp/vkore-port-996-retained-rollback-dagfs.log), and 7 image client
export/registration tests
(/tmp/vkore-port-996-retained-rollback-image.log). The exec tests include new
real SPSC/client/executor paths cancelling FS and power cleanup at admission,
retirement and confirmation, plus image face/bound retirement correlation.
Their partial records contain service identities without fabricated native
owners; the tests prove protocol correlation and retained-slot control flow,
not on-metal native revocation. The changed malformed-face test now cancels
retire_retained directly and verifies that its exact record remains present.
An initial wrong test filter selected zero tests and a missing test-only import
caused a compile failure; both were corrected before the reported final runs.

Native compilation of svc-image, svc-init, svc-exec, kore-agent-runner-bin,
kore-shell-bin and svc-compositor passed in
/tmp/vkore-port-996-retained-rollback-native.log. All observed build/test handles
are terminal; the final runs exited zero. git diff --check passed. No broad
suite, QEMU/model/UI gate, fresh ISO, commit, PR, issue closure or promotion ran.

The overall issues remain open. In particular, provisioning still keeps local
peer/launch construction owners across awaits before installing a rollback row;
those construction paths and private-context setup require further work.
System-seed lifecycle observations, init MM coordination, Found/poisoned-pair
recovery, actual aggregate live budgets, committed library exports/pins/artifacts
and full guest/model/UI/platform acceptance also remain. Broad tests stay
deferred until every issue implementation is complete.


## Original loader-face fork cancellation checkpoint

Implemented persistent original-template and late-face custody for the first
allocation in linked launch construction. TaskManager owns FaceFork state;
fork_linked_face routes both its initial allocation and its existing one-pass
TableFull recovery attempt through it. The template is recorded before the
client's first poll. Cancellation or an ambiguous transport return leaves that
record available to maintenance. A late Face response moves its actual ring
and receive bank, service-authored identity/generation and module descriptor
into persistent custody synchronously before any cleanup await.

Recovery resumes only that original template, retires the abandoned face by
its exact service identity, and then retires local owners before clearing the
record. A late face is never reused as the next launch's result. Prepublication
NoOutstanding clears the marker only when the pair is idle; fresh admission
cannot adopt a foreign identical request. TaskManager's maintenance deadline
includes fork state, maintenance advances it, and source/image-operation
recovery settles it before reusing the image pair. A usable uninterrupted
result still transfers synchronously to the existing launch caller; partial
results still take the existing rollback route.

RingImage now exposes resume_face_fork_bounded, which uses the client's exact
original request and cannot start a replacement fork. The typed face decoder
also rejects unexpected response variants, grants on a refusal, and delivered
or retained records hidden behind a zero grant count, preserving uncertain
returned owners in the pair's poison custody. No kernel or wire ABI changed.

Focused verification passed 12 tests on x86_64-apple-darwin: 3 FaceFork custody
cases (/tmp/vkore-port-996-face-fork-focused.log) and 9 image client cases
(/tmp/vkore-port-996-face-fork-image.log). Fork custody tests use the production
state machine with a protocol fixture containing service identities and no
native grant bank; they cover cancellation before publication, late partial
allocation followed by cancelled retirement, original-template retention and
foreign identical request refusal. The concrete RingImage/SPSC tests verify
that fork resume preserves a foreign retirement and publishes nothing when
idle, and that malformed face replies are rejected. These are protocol/custody
checks, not proof of native capability delivery or on-metal face reuse.

Native compilation passed for svc-image, svc-init, svc-exec,
kore-agent-runner-bin, kore-shell-bin and svc-compositor in
/tmp/vkore-port-996-face-fork-native.log. All observed handles are terminal and
final runs exited zero. git diff --check passed. No broad suite, boot/model/UI
run, fresh ISO, commit, PR, issue closure or canonical promotion occurred.

This closes the pending FaceFork allocation boundary, not all construction.
The successfully returned face and subsequent FS/power peer construction still
have local owners across later awaits; their admission stages and private
view application need persistent construction custody and exact recovery.
System-seed lifecycle, init MM coordination, Found/poisoned-pair recovery,
aggregate live budgets, committed library exports/pins/artifacts and full
real guest/model/UI/platform acceptance also remain. Broad tests stay deferred
until all issue implementations are complete.


## Acquired launch-resource construction rows checkpoint

Extended persistent custody past the initial FaceFork reply. serve_spawn now
puts its successfully acquired LinkedFace into the already reserved task row
before provisioning FS/power peers. Each completed peer is installed into that
same row synchronously before the next await. The later private-view call and
all delivery staging borrow these records instead of keeping them in local
Options across waits. Successful Found transfers the row's FS/power owners
into reconciliation and releases the proven-moved face names synchronously
before any telemetry await.

Added BackingKind::Constructing to distinguish the active launch from rollback.
Nested capacity sweeps during provisioning deliberately skip it. The next
top-level serve_spawn or maintain boundary converts abandoned construction rows
into ordinary Rollback without moving their owners. maintenance_pending counts
these rows. This uses the existing reserved row and named arena rather than
adding an unbounded allocation or a parallel task-lifetime authority.

queue_spawn_rollback now adds newly returned partial owners to an existing
construction row (or creates the original rollback row when none exists), then
marks it Rollback before awaiting retirement. Existing owner fields are never
replaced by empty arguments. Private internal assertions reject a duplicate
owner being assigned to an occupied field rather than silently dropping one.
The failure macro continues to move synchronous channel-construction results
into that row before its cleanup await.

Focused verification passed 14 selected tests on x86_64-apple-darwin: 10
retirement/cancellation checks in
/tmp/vkore-port-996-construction-row-focused.log and 4 construction checks in
/tmp/vkore-port-996-construction-row-cancel.log. New tests cancel after recording
a face and FS peer, verify active rows are excluded from nested rollback
selection, check top-level abandonment retains exact identities, preserve
already recorded resources when a partial power peer joins rollback, and prove
abandonment does not change reconciliation or static-boot custody. These use
partial service-identity records without fabricated native capability owners;
they are lifecycle-control tests, not native Found/revocation acceptance.

Native compilation of svc-image, svc-init, svc-exec, kore-agent-runner-bin,
kore-shell-bin and svc-compositor passed in
/tmp/vkore-port-996-construction-row-native.log. All observed test/build handles
are terminal with zero exit codes. Later production edits were selected
function formatting only; git diff --check passed. No broad tests, QEMU/model/UI
gates, fresh ISO, commits, PRs, issue closures or canonical promotion occurred.

The full objective remains open. FS/power provisioning still has local partial
owners inside its own construction/admission futures, and an in-flight private
view application needs exact recovery plus fixed invocation-input custody.
Those are not claimed fixed by retaining completed peers here. System-seed
lifecycle, init MM coordination, unknown Found/poisoned-pair recovery, live
aggregate budgets, committed library exports/pins/artifacts and full real
stack acceptance also remain. Broad tests remain deferred until every issue
implementation is complete.


## Private invocation-view recovery checkpoint

Completed and inspected the prior private-view handoff changes. FsPeer retains
its exact fixed input index before ApplyView first suspends. RingFs now resumes
only that peer, rights and announced return landing through a bounded exact
request. A matching response (including service refusal) proves the preceding
exact-held move completed; an idle NoOutstanding state leaves the input for
local cleanup. Foreign requests, poisoned pairs and uncertain transfers retain
the phase and input protection. Peer retirement cannot overtake this phase.

serve_spawn records its fixed landed-input cleanup mask before the first
await. Pending view sources join boot-stamp sources in both input cleanup and
reposting exclusions. The current callsite audit covered selection, contended
sweeps, refusal reset, maintenance, and successful Found. This does not resolve
unknown Found completion or poisoned-pair recovery, which remain separate work.

Revalidated terminal evidence: 13 selected retirement tests passed in
/tmp/vkore-port-996-private-view-focused.log; all three new private-view tests
passed in /tmp/vkore-port-996-private-view-cancel.log. The six native consumers
compiled in /tmp/vkore-port-996-private-view-native.log. No matching cargo
process remained live. Tests use actual RingFs requests/replies and executor
cancellation with logical peer identities; they do not simulate native input
capabilities or prove kernel move/reclamation acceptance.

## System package manifest ownership checkpoint

The current-state audit found a real caller mismatch introduced by the shared
seed-write refactor: write_system_manifest still opened its file directly,
then invoked write_opened_member, whose resize now requires a file owned by
SeedWrite. That path would fail NoOutstanding and also left its direct file
and directory handles outside cancellation cleanup.

The manifest writer now opens its package directory through the existing
Projection ledger and opens or creates the leaf through SeedWrite. Create's
pending operation records its exact ObjectSubtype, retaining PACKAGE_MANIFEST
across cancellation while executable seed callers continue to request
EXECUTABLE_IMAGE. A terminal NotFound alone permits creation; other open errors
propagate. The caller requires a file carrying PACKAGE_MANIFEST before writing.
Existing valid manifests keep their stable object identity. Unexpected kinds
or subtypes remain owned for cleanup and are refused.

The existing ordered cleanup now covers this caller: settle the original
write, close the file, detach an incomplete fresh leaf, close commit deferral,
then close the package directory. Cancellation preserves both ledgers. A
completed copy keeps its namespace edge. Commit deferral still batches
persistence rather than isolating or rolling back live intermediate revisions.

Focused verification: all nine service::write::tests passed on
x86_64-apple-darwin in /tmp/vkore-port-996-manifest-write-focused.log. Three new
checks exercise existing/fresh manifest files through resize/write/close/commit,
late typed Create recovery with parent retention until fresh-edge cleanup, and
an access refusal that must not create a file. These are real RingFs protocol
and retained-state tests, not durable-volume or native boot acceptance.
Native compilation of svc-image, svc-init, svc-exec, kore-agent-runner-bin,
kore-shell-bin and svc-compositor passed in
/tmp/vkore-port-996-manifest-write-native.log. Test session 88967 and native
session 43533 were observed terminal with exit code zero. Selected production
formatting and git diff --check passed.

The full objective remains open. FS/power partial provisioning/admission still
needs persistent custody and exact stage recovery. The direct SetSystemSeed
lifecycle requests still need cancellation recovery; init MM coordination,
unknown Found/poisoned-pair handling, live aggregate budgets, committed library
exports/pins/artifact provenance and complete real stack acceptance remain.
All seven tracking issues remain open. No broad tests, QEMU/model/UI gates,
fresh ISO, commits, PRs or canonical promotion were performed. Broad tests stay
deferred until every issue implementation is complete.


## System seed lifecycle and empty acknowledgements checkpoint

Added exec's persistent seed::Lifecycle owner and a bounded exact
RingFs::set_system_seed_active_bounded API. Both seed_app lifecycle publications
now go through this owner. The requested active/inactive posture is recorded
before the first await; fresh publications require an idle exclusive pair.
Recovery resumes only the original encoded SetSystemSeed request. It retains
the intended posture on transport errors or terminal refusals, so maintenance
can finish the last catalogue item's close publication without requiring that
item to be seeded again. The service's existing operation is idempotent.

TaskManager retains this owner, includes it in its maintenance deadline, and
recovers it before reusing the filesystem source pair. An unissued recorded
publication may be completed by maintenance. A foreign identical request is
not adopted by a fresh owner. A poisoned pair retains the phase and does not
publish replacements; connection reset/poison recovery is still pending and
is not claimed solved by retention.

The initial focused run caught a real decoder error: a reply carrying a
revision payload decoded as Done for SetSystemSeed because the empty-success
arm ignored ordinary payload fields. Tightened decode_rep_for's shared Done
arm to reject kind/count/handle/length/revision/cursor payload; existing checks
already reject flags, grants and reserved fields. Canonical service replies
remain unchanged. A wire test mutates each of the ten fields independently
across representative empty acknowledgement requests. The previously failing
lifecycle test now proves a malformed response poisons the pair and retains
its pending phase rather than falsely certifying completion.

Focused evidence after the fix:
- 5 exec lifecycle tests passed in
  /tmp/vkore-port-996-seed-lifecycle-focused.log (final session 81836).
- 11 DagFS wire tests passed with runtime enabled in
  /tmp/vkore-port-995-seed-ack-wire.log (session 83415).
- The existing durable service lifecycle authorization/idempotence/restoration
  test passed in /tmp/vkore-port-995-seed-lifecycle-router.log (session 47988).
- Native svc-image, svc-init, svc-exec, kore-agent-runner-bin, kore-shell-bin
  and svc-compositor compiled after the decoder fix in
  /tmp/vkore-port-996-seed-lifecycle-native-final.log (session 17174).
All these handles were observed terminal with exit code zero. The initial
exec test attempt exited 101 on the malformed-reply regression and is not
counted as passing evidence. Selected formatting and git diff --check passed.
These tests exercise real wire/client/executor state and the existing service
state transition; they do not constitute native boot/durable-volume acceptance.

The next admission audit confirmed that PeerBundleAssembler::retire_incomplete
exists and documents a fresh Ring as its abandonment boundary, but has no
production callers. Both services/dagfs admission and services/power admission
call accepts directly and reject a replacement Ring while the partial old
assembly remains. This needs coordinated repair with exec's persistent partial
peer ownership and exact stage recovery; no admission code was changed in this
checkpoint. Avoid inventing a second abort protocol without first reconciling
this existing shared mechanism and its native ownership requirements.

The objective remains incomplete. Partial FS/power provisioning/admission,
init MM coordination, unknown Found/poisoned-pair recovery, live aggregate
budgets, committed library exports/pins/artifacts and complete real-stack
acceptance remain. All tracking issues are open. Broad testing stays deferred
until every issue implementation is complete. No broad tests, QEMU/model/UI
gates, fresh ISO, commits, PRs or canonical promotion occurred.


## Retained peer provisioning and exact component admission checkpoint

Moved the remaining local FS/power construction owners into serve_spawn's
reserved Constructing row. provision_fs_peer/provision_power_peer now take the
row index and return only a status. The MM backing enters the row before any
channel-allocation await; each actual channel result is installed before the
next allocation. Admission borrows the same row in place. Failure queues that
existing row for rollback instead of taking/reinserting a partial peer. Nested
capacity sweeps still skip active Constructing rows; the top-level boundary
turns abandoned construction into rollback without dropping owners.

FsPeer and PowerPeer now retain the exact pending PeerBundlePart. Fresh staged
transfers require an idle pair and record their phase before polling. Known
NotTransferred/terminal replies clear it synchronously; AfterTransfer or
cancellation preserve it. RingFs and RingPowerAdmin have resume-only bounded
component methods matching the original part and announced return landing.
The FS retry loop no longer resumes an arbitrary request through generic
resume. Both native admission paths preserve their original source roots and
capture an accepted final component before awaiting TakeAdmission.

Rollback settles the component before retiring anything. A late accepted
ReturnRecv enters the unique admission-mailbox state, captures the exact
service-assigned generation, then performs RetirePeer and ConfirmRetired.
Known nonterminal completion/refusal or an idle unissued phase can proceed to
local lineage retirement; an incomplete bundle has no mapped serving peer.
Foreign requests, poisoned pairs and uncertain original transfers retain the
phase. New provisioning and mailbox ownership checks include pending component
records, and is_closed requires the phase to be absent.

Connected both filesystem and power service admission to the shared runtime's
existing incomplete-bundle abandonment rule. PeerBundleAssembler::prepare
requires a valid replacement Ring capability before invoking retire_incomplete.
Old returned-capability owners stay in the assembler until native retirement
succeeds; the new input remains with Serve/host cleanup if it cannot proceed.
Only the services' authorized admin paths call prepare; push still validates
synchronously. A new wire opcode or parallel abort protocol was not introduced.
The production caller audit found FS/power construction sends Ring first and
resumes ambiguous original transfers. Accel's separate admission path is not
changed by adding this opt-in preparation method.

Focused evidence on x86_64-apple-darwin:
- 13 exec retirement/construction/private-view/component checks passed in
  /tmp/vkore-port-996-peer-admission-focused.log (final session 75286).
- The filesystem exact peer-control wire check passed in
  /tmp/vkore-port-995-peer-admission-wire.log (session 86188).
- All 6 power wire checks passed in
  /tmp/vkore-port-996-power-admission-wire.log (session 72493).
The four new exec tests cover all three component stages for both protocols,
accepted and refused replies, repeated cancellation, late complete-bundle
admission through exact generation retirement, unpublished phases and foreign
request preservation. The fixture uses actual client/ring/executor state and
partial logical peer records without fabricating native capability owners.
Consequently it does not prove physical assembler cleanup/reuse on a guest.
An initial test compile found mistakenly added fields in three LinkedFace
fixtures; those edits were corrected before the passing run.

Native compilation passed for svc-dagfs, svc-power, svc-image, svc-init,
svc-exec, kore-agent-runner-bin, kore-shell-bin and svc-compositor in
/tmp/vkore-port-996-peer-admission-native-final.log (session 64351). All four
final handles were observed terminal with exit code zero. Selected formatting
and git diff --check passed; later edits changed comments only.

The objective remains open. Native capability-transfer/partial-assembler
retirement and repeated peer reuse require actual founded guest acceptance;
compilation and protocol tests do not establish those outcomes. Init shared-MM
coordination, unknown Found/poisoned-pair recovery, live aggregate budgets,
committed library exports/pins/artifacts and the full real stack acceptance
remain. All seven issues remain open. No broad tests, boot/model/UI gates,
fresh ISO, commits, PRs or canonical promotion occurred. Broad tests stay
deferred until every issue implementation is complete.


## Init workspace-command maintenance checkpoint

The current-state init MM audit corrected an earlier suspected dependency:
init has one RingMm used by serial boot probes and later resident commands /
worker allocations, but it has no MmSource or MM-backed heap-refill caller.
Do not add an exec-style heap coordinator there based on the old checklist.
RingMm rejects fresh supply requests while occupied, and SvcClient stores its
Preparing request before the first cancellable post/transport wait. Existing
WorkerAllocation retains its pending supply index and captures a completed
lease before its next await.

Found and fixed the actual missing recovery caller: workspace refresh and
resource discovery use ResidentAgents.command with command_worker=None. On a
pending MM or exec result they returned with that owner retained, but later
refresh only called synchronous try_cleanup. The other observe_cleanup caller
was gated on a worker association, which these commands never receive. Thus
the retained request had no reachable maintenance path even though its cleanup
implementation existed.

Added ResidentAgents::maintain_workspace_command. Between commands, it resumes
that unassigned allocation through the existing bounded MM/exec cleanup path,
then retires the workspace binding only after allocation cleanup succeeds.
Worker-associated commands stay with their exact worker retirement path.
ResidentFrontend::maintain_native calls this before lane maintenance, including
when every lane is idle or HostLink reporting has failed. It does not prevent
worker-local cleanup or remote control processing when workspace cleanup is
still pending. refresh_workspace also invokes it before retiring/replacing the
source inventory. No new native owner, MM client, global allocator or wire
protocol was introduced.

Focused verification: all 8 allocation::tests passed on x86_64-apple-darwin in
/tmp/vkore-port-996-workspace-command-focused-final.log (session 11231). The
existing pending-supply cancellation test now drives full observe_cleanup with
an idle real RingExec, rather than only its private MM helper, and confirms no
exec request is fabricated. Its supply client models protocol-only timeout and
late refusal, not a native lease. A new real RingMm/ring/executor test has two
allocations request identical sizes while a foreign Stats owns the pair. Both
refuse without claiming pending custody, cleanup leaves the queued snapshot
untouched, and only the original Stats resumes it. This tests shared-pair
admission and reply preservation; it does not emulate successful native grants.

Early test attempts exposed fixture errors (MmRequest lacks PartialEq, and the
wire Stats contract requires an available provider snapshot). The final fixture
uses matching and a valid snapshot; production MM validation was not weakened.
Native svc-init and kore-agent-runner-bin compilation passed in
/tmp/vkore-port-996-workspace-command-native.log (session 42845). Both final
handles were observed terminal with exit code zero. Later changes touched only
tests; git diff --check passed.

The init missing-maintenance path is implemented. Real native cancellation,
late lease/founding recovery, binding retirement and subsequent workspace reuse
still require founded guest acceptance. Unknown Found/poisoned-pair recovery,
live aggregate budgets, committed library exports/pins/artifacts and complete
real guest/model/UI/platform acceptance remain in the full objective. All seven
issues remain open. No broad tests, boot/model/UI gates, fresh ISO, commits,
PRs or canonical promotion occurred. Broad tests remain deferred until all
issue implementations are complete.


## Image face/bank partial-possession custody checkpoint

Revalidated the existing issues and current worktrees before continuing the
implementation. The prior issue-status-only turn did not advance code; this
checkpoint records a concrete ownership fix in the current guest port.

The image client previously dropped its ring lease and already-converted recv
owners when a later face grant failed typed possession. FaceBank likewise
lost earlier recv owners on a missing/wrong-kind grant. A native retirement
refusal during Drop can leave the allocator name quarantined without an owner
available for another cleanup attempt.

Moved endpoint erasure into the existing endpoint_owner macro, preserving the
ChanSendToken operation and adding the same zero-kernel-operation conversion
for ChanRecvToken. Both preserve the exact OwnedSlot. RingImage now converts
face/bank recv arrays through one bounded synchronous conversion loop. On any
missing/failed conversion it restores every earlier owner to its original
retained-array index. The face path also restores its ring lease before
poison_retained, so the generic client retains actual owners for retirement
retry. A partial face still returns its authenticated face/generation response
with no usable FaceAuthority, preserving exec FaceFork's remote-retirement
identity. This does not reset the poisoned connection or claim remote cleanup
already succeeded.

The typed face boundary now requires the full face grant count. FaceBank
requires a complete Banked reply or an empty Refused reply, rejects foreign
response shapes and hidden out-of-count delivery/retained entries, and no
longer documents dropping endpoints as adequate recovery.

Focused verification completed successfully:
- 13 image client export_tests in
  /tmp/vkore-port-image-bank-focused-final.log (session 12931).
- 6 runtime retained_ tests in /tmp/vkore-port-image-bank-runtime.log
  (session 75319), including recv erasure and the existing send/lease ownership
  and retained-return correlation cases.
- Native compilation of svc-dagfs, svc-power, svc-image, svc-init,
  kore-agent-runner-bin, svc-exec, kore-shell-bin and svc-compositor with the
  local libkore configuration in /tmp/vkore-port-image-bank-native.log
  (session 1670). Final edits after this compilation were test-only.
All three handles were observed terminal with exit code zero. The initial
12-test image run also passed (session 53740); the final run adds the face
identity/count regression. git diff --check passed.

The array tests use distinct non-Copy values with observable destruction,
exercising the production conversion loop at every failed/missing position,
short input bounds, successful conversion and preservation outside the bank.
They do not fabricate native channel authority. The runtime erasure test
checks Rust owner/reservation preservation only. Actual native possession,
retrying refused retirement, remote face reclamation and capacity reuse still
require founded guest acceptance.

Unknown Found handling remains incomplete: serve_spawn still classifies the
completion via decoded Status alone. In particular, a live cleanup canary does
not prove the root moved, and the caller receives the actual TaskToken directly
through Found's return channel. Do not infer successful founding from that
canary or treat an unknown raw result as proven rollback. Persistent exact
request/result recovery is still needed. Poisoned-pair reset, live aggregate
budgets, committed library exports/pins/artifacts, and real guest/model/UI/
platform acceptance also remain in the full objective.

All seven issues remain open. No broad tests, boot/model/UI gates, fresh ISO,
commits, PRs or canonical promotion occurred. Broad tests remain deferred until
all issue implementations are complete.


## Found reply diagnostic-ordering checkpoint

The preceding goal turn made concrete image-client custody changes and passed
its focused checks. Revalidated current exec/rt code before continuing Found
recovery. The audit confirmed two separate concerns: the raw Found completion
is still decoded with Status alone, and successful Found previously awaited
MM/monitor/filesystem/network diagnostics after the actual TaskToken had
already been delivered directly to the caller.

Changed serve_spawn to collect requested diagnostic facts after the validated
profile and FS/power construction are known, before provisioning the local
child/cleanup channels and before the terminal Found. Actual remote face and
peer owners are already stored in the persistent Constructing row at these
awaits. Profile gating is preserved, including the separate network bit.
A successful Found now releases moved reservations, installs reconciliation,
reposts input landings, increments counters and assembles Spawned without any
subsequent async provider request. The snapshot's exec field is refreshed from
actual post-Found counters; other provider fields are observations taken before
founding, not claims of an atomic cross-service sample.

Trace evidence: TaskManager's Invoke arm immediately marks direct grants and
replies when serve_spawn returns. rt stage_pair_grants skips Direct entries
and returns without a transport call for an all-direct reply. The minted pair
then pushes the reply before its optional notification await. The real exec
loop awaits serve_next to completion; no current caller-side cancellation of
that whole loop was observed. This change removes the post-handle diagnostic
suspension window; it does not establish general ServeHost cancellation safety
or fix an unknown kernel completion result.

Focused checks passed:
- 7 spawn-filtered exec tests in /tmp/vkore-port-found-reply-focused.log
  (session 97933, observed terminal exit zero).
- 4 construction-filtered exec tests in
  /tmp/vkore-port-found-reply-construction.log (command completed exit zero),
  including retained rows on cancellation and separation from reconciliation.
- Native svc-exec, svc-init and kore-agent-runner-bin compilation with the
  existing local libkore configuration in /tmp/vkore-port-found-reply-native.log
  (session 4835, observed terminal exit zero).
- git diff --check passed.
These are existing focused regression checks and native compilation, plus a
source-ordering audit; they do not simulate a successful native Found or prove
end-to-end delivery/cancellation recovery. No new test was added that merely
asserts the moved lines or fabricates a native child.

Unknown Found classification and original-request outcome recovery remain
unimplemented. A live cleanup canary alone does not prove the root moved, and
a dead canary cannot distinguish a discarded Found from an already-reaped
successful child. The actual caller-owned TaskToken is returned directly;
exec must not invent a successful outcome or claim rollback without proof.
Connection recovery, aggregate live budgets, committed native library exports,
pins/artifacts and real guest/model/UI/platform acceptance also remain.

All seven issues remain open. No broad tests, boot/model/UI gates, fresh ISO,
commits, PRs or canonical promotion occurred. Broad tests remain deferred until
all issue implementations are complete.


## Full-width init clock and wrapped kernel stamp checkpoint

The preceding turn changed diagnostic ordering before Found and passed its
focused checks. Revalidated current time producers and consumers next. Found
an actual range mismatch in init: now_ms was u32, temporary executor results
were clamped at u32::MAX, manual traps overwrote it with a wrapping low word,
and both drive paths seeded new executors from that narrow value. Resident
agent birth, update, retirement and retry deadlines all consume this anchor.
Thus native calibrated 64-bit time could be lost at each handoff after about
49.7 days, independently of whether the agent's own deadline uses u64.

Implemented one shared runtime clock::anchor_millis path. It uses calibrated
full uptime when available and extends kernel low-word samples against the
retained absolute millisecond anchor. Modular deltas under half a period are
forward observations; older/ambiguous samples leave the anchor unchanged.
A zero low word is valid at wrap. Saturating arithmetic prevents a full-width
overflow. With no calibrated clock, observations must be less than half a
period (about 24.9 days) apart; a first low-word sample alone cannot recover an
unknown boot epoch. The ABI docs now state those actual limits.

ExecCtx::anchor_ms now uses that shared extension. Added
Executor::from_standing_at_ns for handoff of an already-known absolute anchor;
the existing raw-stamp constructor remains available. Init now stores u64
milliseconds, uses the full-width constructor in drive and drive_relaying,
keeps executor results without clamping, and applies shared stamp extension to
manual post/park/pause and payload-forwarding traps. Absolute deadline types
and constants are widened through boot waits and the two accelerator probe
callers that share init's clock. Resident frontend/agent/update deadline
conversions use the full anchor with saturating ns conversion. This changes
neither the kernel status layout nor any agent wire layout.

Focused verification passed:
- 8 runtime clock-filter tests in /tmp/vkore-port-clock-range-focused.log
  (session 78463), including the new wrap/stale/half-range/overflow test.
- 2 runtime standing_handoff tests in /tmp/vkore-port-clock-range-handoff.log
  (session 66424). The new test uses real executor wakers and clock_ns across
  two executor instances after three wrap periods, then checks the actual
  sleep_for registration retains the absolute epoch and sub-ms duration.
- 1 runtime kernel_stamp_wrap test in
  /tmp/vkore-port-clock-range-deadline.log (command completed exit zero),
  covering wrap through zero, stale samples, full-width deadline evidence
  across multiple periods and preservation of finer-grained local time.
- 6 agent-runner budget tests in /tmp/vkore-port-clock-range-budget.log
  (session 7586), including original lifetime and no-revival semantics.
These are 17 passing test executions / 16 distinct tests, because the existing
standing handoff test matches both the clock and handoff filters. Tests use
explicit event-driven samples, not elapsed wall time or forged native grants.
- Native svc-init, svc-exec and kore-agent-runner-bin compilation passed with
  the local libkore configuration in
  /tmp/vkore-port-clock-range-native-verified.log (session 99584).
All listed handles were observed terminal with exit code zero. The first two
native attempts (sessions 67456 and 79057) ended with type errors identifying
remaining u32 deadline consumers; those consumers were widened, not cast back
to u32. Final git diff --check passed. No processes remain from these checks.

This fixes init/executor clock range handling, not complete live CPU/memory
budget enforcement or unknown Found-result recovery. Poisoned connection
recovery, committed library exports/pins/artifacts and real guest/model/UI/
platform acceptance also remain. All seven issues remain open. No broad tests,
boot/model/UI gates, fresh ISO, commits, PRs or canonical promotion occurred.
Broad tests remain deferred until all issue implementations are complete.


## Host disconnect preserves prompt uncertainty checkpoint

The previous turn made the full-width init clock change and passed focused
checks. Revalidated host/guest disconnect state before continuing recovery.
Found a concrete host replay hazard: LinkCore::reset_session changed every
retained frontend intent to Queued, including BrokerClaimed/AgentClaimed work.
That host queue could deliver an already-started prompt again after negotiation,
even though a transport break says nothing about previous tool effects.

Added a terminal Disconnected intent posture for claimed work. Repeated resets
preserve it; only never-delivered Queued work remains eligible for delivery.
Retained completion records now record connection_lost until the owning runtime
contains/removes the prompt. Reset no longer erases those records, marks them
retired, or makes an unconfirmed result ready. Their broker receipt route is
cleared and acknowledgements/fallback joins cannot use a replacement connection.
Already published, retired and ready completions retain their confirmed state.
The existing bounded intent/completion queues remain the sole records; no new
unbounded retry/replay table was added.

KoreTaskRuntime checks frontend_interrupted immediately after servicing
HostLink/update events. It removes that exact host intent, invokes the existing
VM stop path, emits guest_connection_lost and fails the prompt before boot
retry/prompt processing can replay it. Queued pre-claim boot retries keep their
existing behavior. Disconnect is not reported as confirmed guest retirement.
If an answer was already emitted, the connection failure uses a non-final
error event, consistent with reporting cleanup failure after publication.
This work invokes the existing process teardown method; it does not newly
prove successful QEMU termination or implement transparent guest reconnection.

Focused host checks against /tmp/vkore-current-kore.toml on
x86_64-apple-darwin passed:
- 29 frontend-filtered library tests, including two new disconnect tests and
  the updated receipt-preservation test, in
  /tmp/vkore-port-disconnect-frontend.log (session 69811).
- 8 kore_runtime tests in /tmp/vkore-port-disconnect-runtime.log
  (session 62279), covering existing publication/cleanup and runtime behavior.
- Final 3 disconnect tests in /tmp/vkore-port-disconnect-focused-final.log
  (session 67068); the last added case preserves already-confirmed readiness.
All handles were observed terminal with exit code zero. The three new tests
exercise real LinkCore encoding/dispatch/state over deterministic backend
fixtures: broker and worker claims, repeated disconnect, same route/generation
in a new epoch, a separately queued prompt, retained results, withheld receipts
and confirmed readiness. They do not fabricate actual guest task teardown or
perform filesystem effects. These are 40 passing executions / 38 distinct
tests. Later edits were formatting only; git diff --check passed.

Real QEMU/model reconnect, no replayed file effects, verified process termination
and a subsequent prompt on a fresh guest still require integration acceptance.
Guest poisoned-pair recovery, unknown Found outcome handling, live aggregate
CPU/memory budgets, committed library exports/pins/artifacts and complete guest/
model/UI/platform acceptance remain in the objective. All seven issues remain
open. No broad tests, real VM/model/UI gates, fresh ISO, commits, PRs or canonical
promotion occurred. Broad tests remain deferred until all implementation is
complete.


## Retained VM termination ownership checkpoint

The preceding turn implemented host disconnect/no-replay handling and passed
focused tests. Revalidated its actual runtime stop path. ProjectRuntime used
to take its QemuProcess out of the Option before terminate(), ignored any
termination error, and dropped the handle. Although QemuProcess::Drop retries
kill/wait, that is best effort and cannot establish successful termination to
the runtime before a replacement starts.

stop_process now sets stop_pending, disconnects HostLink and borrows the same
process through terminate()/wait. The shared retire_process helper clears the
Option only after success. Failure preserves the child and private QMP socket,
leaves stop_pending set, and returns an explicit error. Termination no longer
first resumes the VM via QMP: killing the host child does not need a running
vCPU or a responsive QMP channel.

The stop wrapper emits vm_stop_failed on error. A new prompt retries pending
termination before assigning the session or sending another intent; resume and
spawn also reject a pending stop, and spawn rejects any existing process.
Boot retries check stop_process's result before starting another guest and
retain the existing attempt counter. Explicit session close stops on failure
and withholds its teardown event/context-release claim. Configuration changes
also call stop_process before removing the old project runtime, so replacement
cannot bypass this ownership through Drop. ProjectRuntime::Drop remains a
best-effort final cleanup attempt when the runtime itself is being destroyed;
this is not presented as restart-ready proof. Cleanup-timeout text now says
shutdown was requested instead of claiming the VM already stopped.

Focused verification: all 9 kore_runtime::tests passed against the local Kore
configuration on x86_64-apple-darwin in
/tmp/vkore-port-vm-stop-focused-final.log (session 68090). The earlier 9-test
run also passed (session 92784), before the configuration-replacement guard.
Both handles were observed terminal with exit code zero. git diff --check
passed. The new test starts a real, owned OS subprocess via QemuProcess (shell
execs sleep, with no guest VM), injects two termination failures into the
production custody helper, verifies the same live PID remains owned, then
performs real kill/wait and verifies retirement is idempotent. QemuProcess's
Drop guards test failure cleanup. The test proves process custody and retry;
full runtime restart/configuration guards were compiled and callsites audited,
not exercised through a real QEMU failure or model session.

Real VM disconnect/termination/restart and subsequent prompt acceptance remain
outstanding, along with guest poisoned-pair recovery, unknown Found outcome
recovery, aggregate live budgets, committed library exports/pins/artifacts,
and complete guest/model/UI/platform acceptance. All seven issues remain open.
No broad tests, real QEMU/model/UI gates, fresh ISO, commits, PRs or canonical
promotion occurred. Broad tests remain deferred until all implementation is
complete.


## Runtime restart guard execution checkpoint

The previous turn implemented retained VM teardown and passed its ownership
failure/retry test. Revalidated ProjectRuntime startup, stop, resume and project
configuration interfaces. Added focused coverage of the production runtime
methods themselves, closing part of the prior source-audit-only gap.

The new runtime_restart_waits_for_owned_process_and_pending_stop_before_touching_artifacts
test creates a real temporary project and HostLink listener, with a deterministic
backend solely as a host fixture. Its QemuProcess wraps an owned OS subprocess
(shell execs sleep); it does not start QEMU, Kore or a model server. It verifies:
- An existing process rejects startup before serial-log truncation, preserves
  the same PID, and does not increment the boot attempt counter.
- Pending stop rejects resume before contacting QMP or emitting a resume event.
- stop_process kills and waits for the real child even with vm_suspended=true
  and no QMP server; it clears pending/suspended state only after success,
  preserves the session identity and retains the boot retry count.
- A pending-stop marker independently rejects startup even without a process,
  while preserving the prior serial evidence.
- Successful cleanup then permits a fresh launch, clears the serial log at the
  authorized startup boundary, increments the attempt count, and permits final
  explicit stop to reap the replacement and reset the retry count.

The one focused test passed on x86_64-apple-darwin using the local Kore config
in /tmp/vkore-port-runtime-restart-guard.log (session 22650, observed terminal
exit zero). It completed in 0.03s; both spawned children went through the actual
termination path, and Drop retains panic cleanup. Later changes were formatting
only. git diff --check passed. This adds test evidence rather than changing the
already implemented production behavior. Configuration-replacement failure and
real QEMU disconnect/restart remain outside this test's scope.

The full issue objective remains active: guest poisoned-pair recovery, unknown
Found outcomes, aggregate live CPU/memory budgets, committed library exports/
pins/artifacts, and complete real guest/model/UI/platform acceptance are still
outstanding. All seven issues remain open. No broad tests, real VM/model/UI
gates, fresh ISO, commits, PRs or canonical promotion occurred. Broad tests
remain deferred until all issue implementations are complete.


## Persistent standing-loader acquisition checkpoint

Exec now records its original kore-ld-stub acquisition before awaiting the
ImageService reply. The new service/stub.rs Acquisition owner retains the
request name across cancellation, resumes only that exact bounded request,
and captures the complete response and affine grant bank before returning.
RingImage exposes acquire_bounded and resume_acquire_bounded for this path.
Maintenance recovers acquisition before other image operations; new image
operations reject while acquisition or its rejected bank remains outstanding.
A cancellation before publication clears only when the pair proves idle.

Descriptor validation borrows the retained grants. A valid bank moves once
into StandingStub, keeping its segment parents for later linked founding.
Malformed/refused banks retain every actual grant whose native retirement
fails. Rejection is latched through partial cleanup: removing an extra grant
cannot make a previously rejected response eligible for acceptance on retry.
This last edge was found during review and corrected before final checks.

Verification: nine focused Exec stub tests passed initially (session 3161,
observed terminal exit zero) and after the rejection latch (session 55496,
terminal zero; /tmp/vkore-port-stub-recovery-focused-final.log). Three new
fixtures cover repeated acquisition cancellation and late refusal capture,
prepublication cancellation without adopting another request, and malformed
descriptor refusal. The real ImageService ring test
acquire_resume_cannot_consume_a_foreign_retirement_or_publish_when_idle passed
in /tmp/vkore-port-stub-recovery-client.log; its earlier tool handle was lost
across compaction, but the completed test result and absent cargo process
were verified. It proves exact request-kind correlation and no fresh publish
from resume. These tests do not fabricate native lease authority and do not
prove successful native acquisition or failed native retirement/retry.

The affected native svc-image, svc-exec, svc-init and kore-agent-runner-bin
checks passed using the local libkore composition. Final session 27986 was
observed terminal exit zero; /tmp/vkore-port-stub-recovery-native-final.log.
Initial native log also reports completed success, with no live cargo left.
Git diff --check passed. All seven existing GitHub issues were reverified open;
#993 remains the canonical inventory linking guest #994-#998 and vkore #40.

The overall implementation goal remains active. Unknown Found outcomes,
poisoned-pair recovery, aggregate live CPU/memory budgets, committed exports/
pins/artifacts, and real guest/model/UI/platform acceptance remain unfinished.
No broad tests, real QEMU/model/UI gates, ISO rebuild, commits, PRs, issue
closures or canonical promotion occurred. Broad tests remain deferred until
all implementation is complete.


## Committed-but-unfounded ownership checkpoint

The previous goal turn made implementation progress on standing-loader
acquisition and completed its focused checks. This turn traced Exec's actual
terminal Found and kernel assembly, then changed both sides of a confirmed
failure path rather than inferring ownership from a canary alone.

Exec previously sent Status::Unfounded through pre-commit rollback. Its native
contract says the session committed, the single terminal child was discarded,
and preceding transfers/revocations still stand. terminal_found_discarded now
requires the literal raw Unfounded status and the exact final invocation index
of this one-Found descriptor (clock bits do not affect the decision). That
receipt follows the existing committed-owner release path: dynamic moved slot
reservations are released without another revoke, the remote face/FS/power
owners enter canary-based reconciliation, fixed invocation inputs are cleared
and reposted, and the reply is Refused(BadHandle) without incrementing spawned.
The committed path remains synchronous after the trap. This does not classify
unknown outcome words safely; their existing rollback behavior and noncanonical
Ok handling remain outstanding, as does original-request recovery for them.

Kernel source review found that an early furnish_found failure dropped its
still-unadopted delivery sets. Capability drop does not revoke a transferred
cleanup root. A new test against actual schema/channel/task assembly reproduced
this: forcing the assembly's own zero-arena refusal returned Err(6), but the
moved cleanup root's canary stayed live. The focused test failed before the fix
in /tmp/vkore-port-unfounded-early-before.log (session 31324, terminal 101).
The earlier first compile of that test used a private module import (session
16956, terminal 101); it was corrected to the existing ipc::tests re-export.

FoundDeliveries now guards both delivery and inheritance banks before any
assembly failure. Unadopted SessionChild::Moved owners use the existing
transfer::retire custody path when the guard drops. Successful iteration drains
those owners into child custody, so guard drop does not revoke the live child.
This guard deliberately covers already-committed held moves, including Exec's
private cleanup root. It preserves existing semantics for other staged-child
variants; it does not claim general staged-resource/isolated-space leak repair.
The ordinary consumed-entry publication logic is not redesigned here.

Final focused evidence (x86_64-apple-darwin):
- Exec terminal receipt test passed: exhaustive other 16-bit status values,
  wrong invocation indices, empty session, and full clock stamp; session 45140
  terminal zero, /tmp/vkore-port-unfounded-focused.log.
- Seven existing Exec spawn tests passed; session 3858 terminal zero,
  /tmp/vkore-port-unfounded-spawn-final.log.
- Two kernel custody tests passed (three assembly scenarios): early refusal,
  later handle-publication failure, successful founding then explicit reap.
  Both delivered and inherited moved roots are real schema/channel owners;
  canary/send lineage and mm-pool balance are asserted. Normal public
  descriptors reject the fixture's invalid parameters before commit; these
  tests deliberately reach the assembly's own failure boundary. Session 5932
  terminal zero, /tmp/vkore-port-unfounded-custody-final.log. The narrower
  initial post-fix early-failure test also passed, session 94288 terminal zero.
- Existing multi-Found survivor/index and SELF-publication rollback tests both
  passed after the guard change; sessions 29559/44313 terminal zero, logs
  /tmp/vkore-port-unfounded-survivor-final.log and
  /tmp/vkore-port-unfounded-self-final.log. An earlier survivor run passed too
  (35911). The first attempted kernel command incorrectly requested --lib;
  it ran no tests and was corrected to the actual binary test target.
- Affected native svc-exec, svc-init and kore-agent-runner-bin checks passed
  with local libkore composition, session 42224 terminal zero,
  /tmp/vkore-port-unfounded-native.log. This is not a boot-feature build of the
  changed kernel assembly. That production gate remains part of final broad
  verification, as the user requested.

Twelve distinct focused tests passed for the final behavior. All launched
process handles are terminal; no wait or build remains live. git diff --check
passed. Formatting after the new custody test run was limited to its helper;
subsequent survivor/SELF tests compiled that formatting.

All seven issues remain open and the full objective remains active. Unknown
Found outcomes, poisoned native pair recovery, aggregate live CPU/memory
budgets, committed exports/pins/artifacts and real guest/model/UI/platform
acceptance still need work. No broad tests, real VM/model/UI gates, fresh ISO,
commits, PRs, issue closures or canonical promotion occurred. Broad tests stay
deferred until all issue implementation is complete.


## Native allocation extent and budget admission checkpoint

The preceding turn changed Exec/kernel committed-but-unfounded cleanup and
passed focused tests; it was progress, with all process handles terminal. This
turn re-read issue #996 and traced native budget admission, actual MM supplies,
worker startup and live update handling before changing memory admission.

Confirmed contracts and remaining question:
- Budget.cpu_share has no defined numeric unit in the broker type. Native
  CpuShare is an access grant for one CPU, not a quota or scheduler weight.
  An asynchronous user question asks whether the number means maximum
  simultaneous CPU execution slots across an agent and descendants, or
  relative scheduler weight. No reply had arrived at this checkpoint. This
  choice remains pending; no new CPU budget mechanism was invented under an
  unstated interpretation. CPU-time accounting/enforcement remains unfinished.
- The current frontend memory ceiling equals PRIVATE_BYTES, and native initial
  admission already requires at least PRIVATE_BYTES. Consequently the only
  currently admissible memory ceiling is that exact retained footprint. A live
  decrease would underfund retained backing; an increase exceeds the current
  offer ceiling. No live memory-wire extension was added or advertised as
  dynamic allocation. Full aggregate/resource-budget acceptance is still open.

Implemented:
- WorkerAllocation now verifies the kernel-reported physical geometry of each
  received supply before the next allocation or mapping. A Granted response
  alone is insufficient. The native root must report the exact requested size,
  page-aligned nonempty geometry and a non-overflowing end. This also covers
  the retained tool supplies, which are receiver-mapped and did not previously
  pass through the parent's whole-lease map-size guard.
- The returned affine root is stored before inspection. A refused, missing,
  oversized, undersized or unreadable extent stops admission while preserving
  that root for the existing cleanup path. Pending supply recovery continues
  to capture the original late grant for retirement, not reuse/admission.
- PRIVATE_BYTES and TOOL_PRIVATE_BYTES now derive directly from SIZES, the
  actual list used by allocate_supplies. Both worker and reusable tool backing
  remain priced, including the catalog ring. This is the current private grant
  footprint, not a claim to account for every kernel/shared-service resource.
- NativeTransaction::stage_budget rejects zero cpu_share or cpu_time_ns before
  founding. This is basic admission validation, not positive-budget native
  enforcement. The larger CPU budget work still needs its contract and native
  implementation.

Verification:
- All 9 allocation::tests passed, including the new extent-validation test,
  existing complete-footprint pricing, pre-native refusal, cancellation,
  original MM-request ownership and disjoint worker/command windows. Log:
  /tmp/vkore-port-budget-extents-focused.log; session 20874 observed terminal 0.
  The new test uses inert geometry values, not fabricated native capabilities;
  it covers all five request sizes, over/undersize, misalignment, overflow,
  missing reports and zero/non-page requests. Real granted-capability geometry
  acceptance still belongs to the deferred founded-process gate.
- Native svc-init and kore-agent-runner-bin checks passed through the local
  libkore composition. Initial session 95418 observed terminal 0; after the
  zero-CPU admission change the final command exited 0 synchronously. Logs:
  /tmp/vkore-port-budget-extents-native.log and
  /tmp/vkore-port-budget-extents-native-final.log.
- git diff --check passed. No process remains live from this turn.

The full seven-issue goal remains active. Native CPU quotas and aggregate/live
budget completion, unknown Found recovery, poisoned-pair recovery, committed
exports/pins/artifacts and real guest/model/UI/platform acceptance remain
unfinished. No broad tests, real VM/model/UI gates, new ISO, commits, PRs,
closures or canonical promotion occurred. Broad tests remain deferred until
all implementation is complete. The pending CPU contract question does not
block independent remaining work and is not a whole-goal blocked status.


## Original founding-result inbox custody checkpoint

Previous turn was concrete progress on exact native MM extent admission, with
all test/build handles terminal. The CPU-share semantics question remains
pending; independent founding and cancellation work continued this turn.

Confirmed and fixed a return-publication leak:
FoundReturn publishes the original, sole external child TaskHandle into the
receiver-owned transfer inbox. TransferCell::Drop and failed final adoption
previously called transfer::retire, whose deliberate TaskHandle exception is
correct for delegated authority but skipped child teardown for this original
result too. Closing an unread result inbox therefore left the dormant child
in RUN_TABLE without an external owner.

The regression uses real founded tasks, a real receiver-owned inbox and channel
post, and the production FoundReturn::publish path. Before the fix, receiver
teardown left the child alive; the test failed in
/tmp/vkore-port-founding-return-before.log (94760, terminal 101). The fixture
explicitly reclaimed the leaked child before asserting, preserving pool balance.

TransferCell now retains owns_founded_task alongside the original affine box.
Only FoundReturn's new Delivery::commit_founding_custody / reservation
commit_founding route sets it. Ordinary rendezvous custody keeps the delegation
policy. Inbox close and failed final adoption carry this origin into the
existing revocation/teardown path; original founding results reap their child,
while delegated TaskHandles still retire only their own authority. Successful
adoption moves the same boxed capability into the receiver's holdings and
removes the inbox cleanup obligation. No wire ABI or capability kind changed.
This fixes direct original Found publication, not every conceivable movement
of an already-held lifecycle token. Ordinary Exec children are dormant until
the caller adopts the handle and explicitly STARTs them. The tests do not prove
remote running-thread or guest-vCPU teardown disposition.

A second caller audit fixed a follow-on Unfounded bug from the preceding
ownership work: serve_spawn had already cleared/rearmed inputs after the
committed receipt, but serve_async still reset them again for any refusal
reply. That could touch a newly landed next invocation. The private
SpawnCompletion enum now separates pre-commit Refused from Committed(reply),
including committed Refused(BadHandle). Its outer caller resets only the former;
a successful Spawned reply still declares its direct grant(s). The committed
path remains synchronous after Found. Unknown/noncanonical status-word handling
is still not solved by this enum.

Focused results:
- Four original-result custody tests passed (8376, terminal zero), covering
  unread inbox close, final adoption refusal and exact slot scrub, successful
  adoption followed by owner teardown, and an ordinary ACCESS delegation whose
  still-live parent handle survives receiver close. Tests assert task presence,
  actual retained authority and mm-pool balance. Log:
  /tmp/vkore-port-founding-return-focused.log. The initial one-case post-fix run
  also passed (99540, terminal zero), before the four-case fixture expansion.
- Existing transfer-generation reuse and adoption-refusal tests passed (2 tests,
  20132 terminal zero, /tmp/vkore-port-founding-return-transfer.log).
- Existing dead delegated TaskHandle retirement test passed (1 test, 74716
  terminal zero, /tmp/vkore-port-founding-return-delegation.log).
- Eight Exec spawn tests passed, including the committed-refusal reset
  disposition regression (41114 terminal zero,
  /tmp/vkore-port-founding-return-exec.log).
- Native svc-exec, svc-init and kore-agent-runner-bin compile checks passed
  with the local libkore composition (76081 terminal zero,
  /tmp/vkore-port-founding-return-native.log). Changed kernel code has focused
  host execution evidence; production kernel build/gates remain deferred.
- Fifteen distinct focused tests passed. All handles are terminal; diff checks
  passed. Final changes after tests were limited to test-function formatting.

NEXT CONCRETE FOUNDING PROTOCOL EDGE (not fixed yet):
Shared-wake launches stage return_shared_wake BEFORE terminal Found. On exact
Unfounded, that wake transfer has committed even though no TaskHandle returned.
The current refusal response declares zero grants; finish_invoke_reply accepts
only grantless spawn refusals, while the live result channel ordering puts task
at index 0 and shared wake at index 1. Generic SvcReply consumes a contiguous
prefix [0..grants], so simply declaring one or two grants does not represent
this hole correctly. Need coherent server/client refusal custody and cleanup;
never clear/reuse an unclaimed posted wake target or claim successful founding.
A plausible implementation is to place optional wake at index 0 and task at
index 1 for wake-returning Invokes, add a distinct committed-discard response/
refusal, declare the one-grant prefix on Unfounded, and have the client retire
that returned wake before reporting the confirmed refusal (retaining it on
failed cleanup). Audit wire compatibility/version and every result-index caller.
This is a proposed next step, not implemented or accepted evidence. It affects
window/shared-wake callers; ordinary worker Invokes request no shared wake.

Unknown Found words, poisoned-pair recovery, native aggregate/live CPU budgets,
remaining budget accounting, exports/pins/artifacts and full real guest/model/
UI/platform acceptance remain outstanding. All seven issues and the full goal
remain open. No broad suites, real VM/model/UI gates, new ISO, commits, PRs or
canonical promotion occurred. Broad tests stay deferred until all issue
implementation is complete.


## Shared-wake committed-refusal custody checkpoint

The prior checkpoint's wake-only prefix edge is now implemented across the
native exec ABI, server, client and diagnostics. Invoke uses new opcode 25;
legacy task-first opcode 16 is a retired typed refusal on the new server.
A wake-returning Invoke stages wake grant 0 and task grant 1; an ordinary
Invoke still returns only task grant 0. Successful replies announce the full
prefix. Exact terminal Unfounded announces only the committed wake prefix
and uses new REFUSE_UNFOUNDED (21), rather than a generic pre-commit BadHandle.
The server preserves its committed input-bank handling and reconciliation row.
Unknown-opcode handling on older peers may time out, but cannot reinterpret
the changed order. Mixed old/new images are not a supported composition.

RingExec validates the refused reply's exact shape and wake kind/rights,
retains that actual ReturnedCapability in discarded_wake before attempting
retirement, and reports confirmed Unfounded only after closure. Failed cleanup
returns Timeout while retaining the same owner. resume_invoke_bounded retries
only that cleanup, without publishing or consuming another request. New
invocations return all unmoved inputs with Outstanding during cleanup; all
other direct request/resume paths, including face construction, are guarded.
is_idle remains false until the discarded owner closes. Missing/malformed
returned owners poison the pair. Init and shell diagnostics recognize the new
refusal explicitly. The affected host exec ring fixture now borrows a bounded,
aligned local window instead of allocating/leaking global heap memory.

Focused verification (all final terminal exit 0):
- 5 tests filtered by unfounded, including 3 new client tests, new wire shape
  test and the existing exact terminal receipt classifier:
  /tmp/vkore-port-unfounded-wake-focused.log (session 79179).
- 18 client/mint tests:
  /tmp/vkore-port-unfounded-wake-client.log (38517).
- 11 wire tests, including new legacy-opcode/grant-order compatibility test:
  /tmp/vkore-port-unfounded-wake-wire.log (71470). Initial run failed because
  its unknown-refusal sentinel was the newly assigned 21; changed to one past
  REFUSE_UNFOUNDED and reran the complete focused wire set.
- 8 spawn tests, including committed-refusal input reset ownership:
  /tmp/vkore-port-unfounded-wake-spawn.log (synchronous exit 0).
- 37 distinct focused tests across those overlapping filters passed.
- Native build-std check of svc-dagfs, svc-power, svc-image, svc-init,
  kore-agent-runner-bin, svc-exec, kore-shell-bin and svc-compositor against
  the same local ABI/runtime owner graph passed:
  /tmp/vkore-port-unfounded-wake-native.log (52340). Initial compile exposed
  three exhaustive init diagnostic matches; updated and rechecked.
- git diff --check passed. No live test/build sessions remain.

These tests prove reply correlation, malformed-bank refusal, inert affine
cleanup-state retention, and native compilation. They do not fabricate a
native returned wake on the host or prove production shared-wake retirement;
that requires the eventual rebuilt guest gate. All dependent native ELFs and
exports must be refreshed together before promotion.

NEXT: terminal founding still uses committed.status() == Ok as success and
pre-commit rollback for other unrecognized receipts. Noncanonical Ok+index
and unknown status words need explicit custody quarantine/original receipt
recovery; they are not rollback or success proofs. Do not paper over this
with canary-only inference (dead cannot distinguish discarded vs exited child).
Poisoned-pair recovery, native aggregate/live CPU and remaining budget
accounting, exports/pins/artifacts, and full real guest/model/UI/platform
acceptance remain outstanding. CPU-share contract question remains unanswered.
All seven issues and the full implementation goal remain open. No broad
suites, QEMU/model/UI gates, ISO, commits, PRs, issue closures or canonical
promotion occurred. Broad tests stay deferred until all issue implementation
is complete.


## Indeterminate terminal founding custody checkpoint

Previous goal turn was progress (shared-wake refused-launch custody). This
turn inspected the actual service, transport and kernel outcome paths. Native
TrapUnavailable still proves the trap was not issued; kernel commit_current
publishes Unfounded after committed child assembly fails. The defect was
exec using Committed::status() both as success and rollback proof: unknown raw
values decode to Malformed, and Ok with a nonzero index is not canonical.

Implemented a reachable founding_outcome classifier at the terminal trap:
- Literal Ok/index 0 proves founding.
- Literal non-parking refusal codes 1..=6 with an in-range index prove refusal.
- Literal Unfounded with the exact terminal Found index proves discard.
- Unknown words, park statuses, invalid index/count and noncanonical Ok are
  Indeterminate. No inference from the cleanup canary upgrades that result.

The existing construction row now has an IndeterminateFound disposition with
its original full raw receipt/clock bits, descriptor invocation count and exact
landed-input mask. It retains its loader face, FS/power owners and both local
channels synchronously before returning. It is never converted to rollback
by abandoned-construction cleanup. The shared fixed founding and input banks
remain reserved: request admission, serving, maintenance, attach and detach
are fenced while this row exists. Because no grant count can truthfully
represent an unknown task/wake transfer, SpawnCompletion::Indeterminate emits
no reply and does not reset inputs. The caller keeps its original outstanding
request/result posts and uses its bounded observation path. Normal success,
known refusal and committed discard retain their existing protocol behavior.
The generic host counts the intentionally reply-less step as wire_refused;
this containment state is not reported as successful cleanup or a refused
zero-grant founding.

This is conservative custody containment, NOT completed automatic recovery.
No exact native receipt-query/correlation mechanism currently exists for an
unrecognized synchronous trap word. The row keeps the actual evidence; it
cannot safely guess/replay or release reservations. Recovery still needs
stronger original-outcome evidence or verified whole-VM/task teardown. Service
availability and capacity recovery in this path remain part of the full goal.

Focused verification, all final terminal exit 0:
- 4 founding tests: three new classifier/custody tests plus the existing
  committed-unreferenced-slot test. Unknown/park raw words and invalid success
  indexes are swept exhaustively, with clock stamps preserved. The row test
  uses inert service metadata/empty channel-owner containers, not fabricated
  native authority. It proves persistent state disposition, not a live native
  returned task/wake recovery. Log /tmp/vkore-port-founding-outcome-focused.log,
  session 83212; initial compile 16309 exposed one stale test-helper call after
  removing the old discard-only helper, corrected before this passing run.
- 8 spawn tests: /tmp/vkore-port-founding-outcome-spawn.log, session 39722.
- 1 exact Unfounded receipt test:
  /tmp/vkore-port-founding-outcome-unfounded.log, synchronous exit 0.
- 13 distinct focused tests passed across those filters.
- Native build-std check of svc-exec, svc-init and kore-agent-runner-bin passed
  against the same local native owner graph:
  /tmp/vkore-port-founding-outcome-native.log, session 74199.
- git diff --check passed; selective function formatting only. No live
  test/build sessions remain.

Still outstanding: original receipt/pair recovery with actual owners,
native aggregate CPU/time and remaining budget enforcement (CPU-share contract
question still unanswered), exports/pins/artifacts, and full real guest/model/
UI/platform acceptance. All seven issues and the full goal remain open. No
broad suites, QEMU/model/UI gates, ISO rebuild, commits, PRs, issue closures or
canonical promotion occurred. Broad tests remain deferred until all issue
implementation is complete.


## Concurrent private memory reservation checkpoint

Previous goal turn was progress (indeterminate founding custody). This turn
traced native worker allocation and the real paused-authority-update callers.
Confirmed an accounting gap: the resident passed the entire admitted memory
allowance to a temporary share/unshare/binding/receipt command while the live
worker still retained its full private worker+tool allocation. Admission's
old PRIVATE_BYTES ceiling reserved only that worker footprint.

Implemented:
- PEAK_PRIVATE_BYTES derives from PRIVATE_BYTES + TOOL_PRIVATE_BYTES. Native
  frontend admission and broker ceiling now reserve this peak before founding,
  allowing the worker's full backing and one serialized parent command to
  coexist within that same allowance.
- WorkerAllocation preparation/retirement state retains its actual allocation
  family (Worker or Tool). The fixed native MM request count is now derived
  from that family, rather than a separately supplied count. Reservation size
  follows the same state/family; it remains fully charged during partial MM
  allocation, cancelled waits, dormant ownership and failed retirement, and
  clears only when cleanup reaches Idle. No independent quota counter can
  drift from the allocation's disposition.
- command_memory_budget subtracts the allocation's retained reservation with
  checked arithmetic and refuses unless the remaining allowance covers the
  exact temporary tool pair. Native paused unshare, share/binding, and grant
  receipt capture call it before publishing/allocating each command. Nested
  receipt capture receives the original total and prices its own non-overlapping
  command; it does not subtract twice. Existing command serialization and
  cleanup ownership remain the exclusion mechanism.
- An old worker-only allowance now refuses a concurrent binding command
  before MM work instead of silently exceeding that allowance. New admission
  reserves the needed peak. Initial binding without a worker has no retained
  worker charge; temporary command closure still precedes worker founding.

Focused verification:
- 10 allocation tests passed (session 86654, terminal 0), including the new
  concurrent allowance/underflow test and strengthened real cancellation and
  original-MM-resume fixtures. Checks cover exact supply-family pricing,
  reservation retention before a grant arrives and through two cleanup slices,
  zero reservation after confirmed completion, budget refusal before MM
  publication, and disjoint command/worker map windows.
  /tmp/vkore-port-memory-peak-focused.log
- Native build-std check of svc-init and kore-agent-runner-bin passed (20777,
  terminal 0), using /tmp/vkore-libkore-local.toml and the same unique local
  native owner graph. /tmp/vkore-port-memory-peak-native.log
- git diff --check passed. Following tests, changes were formatting only.
  No live test/build sessions remain.

This establishes the bound for the explicit fixed private MM supply families
and overlapping parent commands. It is not a claim that kernel/service-owned
allocations, dynamic aggregate CPU/time, storage/scratch or all live budgets
are fully accounted. CPU-share contract question remains unanswered. Automatic
original-receipt/pair recovery, remaining budget enforcement, exports/pins/
artifacts and real guest/model/UI/platform acceptance remain open.

Additional concrete carry-over gap found while reading issue #996: the issue
references res/AGENT_WORKFLOW.md, but that canonical workflow contract does
not exist in the current port worktree. Its source exists in
/Users/colinhayes/microperceptron/kore-vkore-linux-handoff/res/AGENT_WORKFLOW.md.
The #993 inventory already assigns that contract to #994; adapt it to current
owners and clearly distinguish required behavior from unfinished native
implementation, rather than leaving the canonical reference missing.

All seven issues and the full implementation goal remain open. No broad
suites, real VM/model/UI gates, ISO rebuild, commits, PRs, issue closures or
canonical promotion occurred. Broad tests remain deferred until all issue
implementation is complete.


## Integration completion pass (2026-09-15)

The user's instruction is now to finish the runnable integration without
expanding perfection work. Remaining aggregate budgets and uncommon recovery
hardening are follow-ups; normal execution and shutdown are the acceptance bar.
Broad validation is underway. The host workspace passed 341 tests (three
explicitly ignored), and all 353 tests for the new broker, runner, HostLink,
shared-memory, workspace-sync and VirtIO packages passed. The host daemon, CLI
and native UI release binaries build. Host metadata confirms one native Kore
owner graph. The local libkore snapshot matches the current facade source.

Fixed the missing HostLink module in boot/limine.conf, made the real-only host
refusal test distinguish public errors from private timing events, included
all new packages in make test, and limited test-only Mem convenience constructors
to test builds. check-x86 and check-riscv pass. Fresh x86 service ELFs, archive,
digests and founder manifest have been rebuilt. ARM artifacts and boot kernels
are building; no successful live guest/model acceptance is claimed yet.

Logs: /tmp/vkore-integration-*.log. Test project/state:
/Users/colinhayes/.local/state/vkore-current-kore-integration (isolated workspace,
existing Qwen3.5 9B Q4_K_M model, fresh guest ISO selected when available).


### Broad verification and first live boot findings

Host workspace: 341 passed, 3 intentionally ignored; host release daemon/CLI/UI
built. Guest make test progressed through the kernel (1542 passed, 1 ignored)
and all earlier crate suites. Updated the object-record size witness to include
pending storage custody (56 bytes), and the authenticated image archive census
to include the native agent runner (44 members, 176 immutable carves). Resumed
all remaining make test components: test-svc, test-dagfs, test-acpi-vendor,
test-vkore-integration, test-aarch64-diagnostics all passed. check-x86,
check-aarch64 and check-riscv passed. x86, ARM and RISC-V user binaries build.
QEMU log-test-x86 (Intel/TCG, 4k48) printed boot_test: PASS and powered off.

The fresh production gate exposed two concrete integration defects:
1. The driver's fixed claimant bell band still admitted only five drivers.
   HostLink is driver ordinal 5. Extended the band to slot 111 and updated
   PINS_COMMON so lifetime-banked slots cannot collide with the sixth bell.
2. HostLink refused a standalone machine with no virtio-console (NoMatch,
   rung 18). Its entry now attaches the local pairs, announces disconnected
   readiness, and returns HostDisconnected on bounded client requests. It
   parks on ordinary executor wakes and accepts quiescence. Other hardware
   claim/bind failures remain explicit refusals.

The first fix booted through founding HostLink and exposed the second. The
second is currently rebuilding under /tmp/vkore-integration-offline-build.log.
Production boot and real model/workspace acceptance are not yet established.
No commits, PRs, canonical release promotion or issue closures were performed.

## Native launch integration checkpoint (2026-09-15)

The user requested completion of the runnable integration, with optional hardening deferred. HostLink now passes standalone startup, including an explicit disconnected mode when no host device exists. Exec's six resident provider clients were incorrectly left in direct-polling mode after the serving path moved to the executor. Bounded image acquisition therefore failed before publication; those clients now use executor progress. Standing-stub acquisition resumes its original observation slices and preserves the actual refusal class. Native boot confirms the stub acquisition succeeds.

The remaining native failure is at loader-face acquisition. Diagnostic classification exposed contention on the shared image pair, and the Invoke reply classifier also omitted the Contended refusal that exec can emit. The current build resumes the existing image lifecycle owners before a fresh fork and admits that refusal on the wire. Production acceptance and real Qwen chat/file acceptance are still pending; this checkpoint does not claim completion. Temporary diagnostic refusal substitutions have been removed.

The next boot progressed through extraction to filesystem-reindex validation, where the filesystem provider had an outstanding cleanup request. Foreground invocation and private-client mint now settle the existing provider lifecycle owners before issuing new requests, including the second boundary after ledger-capacity reclamation. This does not retire current invocation landings. Exec's 190 tests pass before the final placement adjustment. Init setup errors now carry `100000 + source line` instead of the ambiguous face-setup rung 1. A fresh production build/boot remains pending.
