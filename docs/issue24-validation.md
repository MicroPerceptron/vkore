# Shared application assistant validation

Issue #24 is stacked on vKore #39 and Kore #933. Neither parent PR is merged by
this change. The architecture and explicit bounds are recorded in
[ADR 24](adr/0024-shared-assistant.md); use [Linux setup](linux-setup.md) for the
matching repositories, image and model.

## Revisions and environment

Guest source: `5173ebce12f6b6631792e8967142b87fcf4c6789`. Shared-core host and
architecture gates ran at `51b66453f6b644e39fca26b7418c0b3a45ec830f`; the
subsequent change adds exact-request cancellation handling in the worker only,
with rebuilt artifacts, production boot and live cancellation revalidation. The artifact commit,
release and exact ISO digest are recorded in `config/real-stack.lock.mk`.
`boot-vkore-headless` has its own kernel cache and ISO filename; the production
standalone and kernel-test images are built and booted separately.

Validation used an Apple M1 with 16 GiB RAM, QEMU 11.1.0 TCG, an x86-64 Kore
guest with 2 CPUs/2048 MiB, Rust nightly-2026-07-13, and Qwen3.5-9B-Q4_K_M on
llama.cpp `0b953005451976d78c8227b16ad3d42a07c2053b` with Metal, 32768 context,
three slots and six host threads. Linux end-to-end execution is not verified.
The model and ISO are release/download artifacts, not checked into vKore.

## Automated checks

- vKore release workspace: 309 passed, 3 intentionally ignored; run with
  `cargo test --release --workspace -- --test-threads=1`.
- Python launcher/timing/resource-selection checks: 7 passed.
- After the final broker planning instruction change: 10 llama backend tests
  passed, and release harness binaries rebuilt successfully.
- Kore host suites: 2878 passed, 13 intentionally ignored.
- Kore x86, AArch64 and RISC-V checks: passed.
- Kernel production and kernel-test builds: passed. Actual kernel-test boot
  passed with four CPUs/4 GiB, matching the parent PR’s recorded gate profile.
  Two exploratory two-CPU runs failed CPU-dispatch spread (one also failed
  wake placement); these runs are retained as failures, not counted as green.
- Production userland gate with 2 CPUs/2 GiB: passed on the final cancellation
  image, with 17/17 services and 27/27 application-store entries.
- Latest native UI suite: 121 passed, 1 intentionally ignored, after the final
  status changes. `make dev-check` verified the headless feature, source pin
  and exact ISO digest. The setup helper passed for virtual and focus-folder
  projects and refused to overwrite existing settings.

Contract tests cover startup queuing, cross-project fairness, generation
invalidation, shutdown with a full work queue, close/cancellation, legacy
settings migration, file identity checks, task grant projection, and native
chunked event decoding. The broker bulk test exercises sixteen requests on the
same route, beyond the twelve retained diagnostic copies. Diagnostic retention
cannot block typed backend processing.

## Live guest and model checks

The retained acceptance scripts record source commits, binary Git diffs and
ISO hashes alongside serial/HostLink/inference logs and durable transcripts.
Pre-final investigations are kept separately from the final acceptance run.

The ten-worker investigation demonstrated actual file reads across two
projects containing different `acceptance.txt` files (`maple-73` versus
`cedar-91`), return to the first project, a virtual-project slideshow lookup,
project-chat close, and six additional chats. Every successful turn kept the
same QEMU and AI-server PIDs. This found and fixed two long-lived-process bugs:
MM supplies needed a reusable cleared pool with fresh revocable worker
subleases, and the host diagnostic upload queue needed bounded eviction after
backend submission. Neither fix increases an authority or admission limit. A later active-worker
cancellation test exposed an ignored HostLink cancellation control record; the
worker now recognizes only the exact active request, returns its affine
transport pair, and lets the resident broker revoke its scope. The failed
pre-fix cancellation is retained, not counted as a passed turn.

The final image completed the two-project file reads, return to A, the virtual
slideshow lookup, close of B, and three additional new chats with QEMU PID
67859 and model-server PID 67813 unchanged. An actually founded worker then
cancelled in **0.111 s**, keeping those processes alive. Chat A remembered
`orchid-52`; chat B's exact model input/transcript contained none of that
codeword, and returning to A recalled it correctly. B's unknown-codeword
question was refused by the broker, so the original test's expectation of a
completed answer failed; this is not reported as a successful answer.

A read-only directory grant exposed the actual `acceptance.txt` descendant in
its effective capability event. The first listing failed because the model
supplied two path arguments to `ls`, which accepts at most one. An explicit
one-argument follow-up produced malformed model output twice, causing a visible
runtime failure. Native Retry prepared a new process pair. A create-file task
then selected a read-only directory; Kore refused the effect and no file was
created. The broker instructions now explicitly require a write-directory
offer for creation/mutation, while preserving read-only selection for reads.
These failed attempts remain in the acceptance logs.

Failure injection then stopped the owned QEMU during an active prompt, with a
second project queued. Both attached generations advanced, the queued prompt
became cancelled, and the interrupted prompt failed. The native failure banner
and Retry button were verified. Clicking Retry created QEMU 2792/model server
2718, and the next file read returned the actual `maple-73`. Stopping that
model server invalidated the runtime again; explicit retry prepared QEMU
4194/model server 4183. No stale queued prompt ran in either replacement.

The final `make dev-debug PROMPT="Create proof.txt in the project root directory."`
run used the published image and corrected broker instructions. The broker
selected the writable directory, the guest `touch` returned `fs: created
proof.txt`, and the final answer confirmed creation. The actual file existed
only under project A, not B. The command exited 0 and its harness, QEMU and
model server all exited. This exercised the normal launcher's build, artifact
preflight, startup queue, guest authority, host writeback, timing report and
process-group cleanup.

The final run's prompt waited 15.569 s for cold preparation, then took 46.047 s
through runtime completion. Broker decision validation took 26.082 s, authority
installation 0.199 s, and worker model/tool/answer processing 19.445 s. The real
`touch` call took 0.198 s; public response to CLI delivery took 0.038 s. These
submeasurements overlap the sequential phases and must not be added twice.
The model calls dominate this request; removing guest startup does not remove
model prefill/generation costs.

## Native verification

Project switching preserved the chat ID and unsent draft when returning to
project A. Creating a new virtual project, New chat, Close chat and archive
kept the shared process IDs. Project catalog reordering preserves selection
by ID, and scoped close does not revoke another project’s approvals.

Native UI operation was verified with Computer Use after macOS permissions
were enabled. A virtual personal-assistant project displayed “No focus folder.”
Its initial task filesystem was empty. During an actual slideshow lookup it
showed one worker and the granted text projection, with no global broker node
or unrelated searchable file. Selecting the file showed the original PPTX
backing path, file type and read-only rights. Retirement removed the file;
the validated answer reported the actual slide's **17.25%** growth.

Large chunked HTTP event bodies previously caused a truncated JSON error in
the UI. The client now decodes the transfer before parsing events; its test
splits a large Unicode event across chunk boundaries. Native live event replay
and the answer were checked after that fix. Completion now also updates the
status instead of retaining “Agent working.”

The fixed 2220 ms native brand animation requests frames independently of
network events. Ten release captures cover 0, 950, 1500, 2100 and 2300 ms at
1x and 2x; mark, wordmark alignment, flicker/fade and the final application view
were visually checked ([captures](../artifacts/issue24/splash/README.md)). Animation completion does not imply broker readiness.

## Background chat navigation follow-up

Active work no longer blocks switching projects or creating another project.
The UI caches the active prompt ID with its chat/draft/events, then resumes
that chat's replay when selected again. Backend execution continues without
navigation sending close or cancel. Late poll events from a different chat
cannot finish the selected chat's prompt or change its cancellation target.
A short outstanding UI mutation still waits for its acknowledgement.

The native test submitted hello in A, switched to B while A was working, and
submitted hello in B. Returning to A showed its completed answer while B
continued. Returning to B then replayed its own completed answer. Both used the already running QEMU 17901 and model server 17851.
The UI suite passed 122 tests (1 ignored), including active switching, late
terminal events, replay deduplication, separate cancellation targets, and new
project creation during a request.

## Timings and interpretation

`GET /api/v1/runtime` includes a bounded startup history. Kernel-entry and
service markers use the host observation clock and 10 ms serial polling;
buffering can affect their exact arrival times. Per-prompt transcripts retain
broker preparation/inference, effective grants, worker/tool calls, final
response, teardown and host-loop timing. Cold model load depends strongly on
filesystem cache and available memory; warm prompts must be compared by process
identity, not inferred from elapsed time alone.

The final cold preparation took **16.210 s**, measured once on this Mac:

| Checkpoint | From preparation start | Interval |
| --- | ---: | ---: |
| Model server ready | 8.998 s | 8.998 s |
| VM boot begins | 9.000 s | 0.002 s |
| Observed kernel entry | 12.998 s | 3.998 s |
| HostLink connected | 13.628 s | 0.630 s |
| Display service founded | 13.676 s | 0.048 s |
| Resident broker initialized | 15.367 s | 1.691 s |
| All 17 services ready | 15.636 s | 0.269 s |
| Bootstrap ready | 16.208 s | 0.572 s |
| Broker ready for intents | 16.210 s | 0.002 s |

The HostLink-to-broker interval was **2.582 s**; the display service was founded
48 ms after HostLink connected, without the old cosmetic playback gate. This
run also initializes the executable store and reusable worker memory pools.
It is not a controlled claim that total startup improves by exactly four
seconds: host cache, memory pressure and preparation work differ. Subsequent
normal chats retained the same process IDs and paid no VM/model cold start.

The original
recorded baseline was 5.195 s from QEMU spawn to HostLink and 5.571 s from
HostLink to broker claim, including about four seconds waiting for Kore's
cosmetic splash. Headless boot removes that playback gate while preserving
framebuffer handoff and founded service initialization.

## Current limits

The task-intent wire limit remains 128 UTF-8 bytes. Personal discovery is bounded
lexical search over explicitly authorized folders, not a persistent semantic
index; it extracts supported text, PPTX slide text and DOCX document text.
“Last week” boosts recently modified files rather than applying an exact
calendar filter. PDF, OCR and embedded pictures are unsupported. Search results
remain inert until Kore validates and grants current file identities.

The real model can refuse a task or produce a rejected candidate. For example,
a story request in the pre-final investigation was refused by the broker;
no canned answer was substituted. Passing transport, isolation and lifecycle
checks does not prove arbitrary task semantics. The live checks inspect actual
files, tool observations and process identities as well as final answers.

## Model-speed display follow-up

The native Working disclosure now shows separate input/output throughput,
tokens, elapsed model time and cached input for each completed broker/worker
call. Rates use final server measurements, with token-count/duration fallback
when the server omits a rate. Partial, missing and zero-duration records do not
invent throughput. Compact final measurements are independent of verbose trace
retention; legacy raw records still render and are deduplicated per call.

UI and vkored suites: 247 passed, 3 ignored. The final UI cache/capture change
was rechecked with 124 UI tests passing, 1 ignored. The real `make dev-debug
PROMPT=hello` run completed, emitted exactly two compact timing observations
(Broker and Agent), and cleaned up its processes. The broker measured 96.9
input tok/s and 9.4 output tok/s; the worker measured 94.2 and 9.5 respectively.
Actual trace replay captures at 1182 and 700 pixels wide were visually checked
([captures](../artifacts/issue24/model-speed/README.md)). The 58-second overall
elapsed label in this cold run includes startup; it is not a model-only timer.
