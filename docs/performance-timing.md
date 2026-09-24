# Request timing diagnostics

`make dev-debug PROMPT="hello"` now writes `timings.md` and `timings.json`
beside the diagnostic `events.jsonl`. Close an existing launcher on the same
port first. The normal native runtime also records the monotonic timing fields
and startup/service milestones in its event transcript.

Regenerate a report with:

```sh
python3 scripts/report-timings.py /absolute/path/to/events.jsonl \
  --markdown /absolute/path/to/timings.md \
  --json /absolute/path/to/timings.json
```

Keep the accompanying `harness.log` beside the events: the reporter extracts
model-server loading and restart durations from its existing startup telemetry.

In the current-Kore integration, response publication and native retirement plus
broker readiness are separate observed milestones. The reporter uses the current
runtime transitions and does not infer readiness from old aggregate capability
telemetry. Tool durations match role, route, generation and call ID; missing or
ambiguous identities are left unmeasured. Startup and terminal errors retain their
original status and visibility when timing fields are attached.

## Historical September 14 measurement

These measurements came from the previous guest stack before the current-Kore
port. They are retained as historical evidence, not measurements or acceptance
of the new integration. The port now observes per-intent response publication
and native retirement/broker readiness through their actual runtime transitions;
the older teardown counters and guest-idle fallback below are superseded.

Host: Apple M1, 16 GiB RAM. Qwen3.5-9B-Q4_K_M, llama.cpp Metal enabled,
99 requested GPU layers, 32K context, three slots, six host threads. Kore is
x86-64 under QEMU TCG, two guest CPUs and 2 GiB RAM. A separate read-only
workspace contained one small file with the value `cedar-42`. The previous
native launcher was closed for the run and reopened afterward.

Three consecutive prompts used the same logical session:

| Prompt | Outcome | Response available | Runtime finished |
|---|---|---:|---:|
| Say hello in one short sentence. | BrokerSetup failure; no assistant answer | — | 42.04 s |
| Read timing-sample.txt and tell me the exact value in it. | Correct actual file read | 51.91 s | 55.91 s |
| Say hello again in one short sentence. | BrokerSetup failure; no assistant answer | — | 40.01 s |

The file-read request had this sequential breakdown:

| Stage | Duration |
|---|---:|
| Project lookup and QEMU process spawn | 0.019 s |
| VM boot and guest broker ready to claim input | 10.483 s |
| Broker request preparation | 0.080 s |
| Broker inference and validation | 22.574 s |
| Authority installation and worker claim | 0.221 s |
| Worker request preparation | 0.018 s |
| Worker reasoning, actual tool execution, and final reasoning | 18.502 s |
| Validated final answer through guest to public response event | 0.014 s |
| Response event to CLI receipt (additional) | 0.037 s |
| Post-response cleanup (overlaps client receipt) | 4.005 s |

Inside the worker interval, model calls took 10.335 s and 7.959 s; the actual
cat call to its observed result took 0.150 s. Remaining time is request/validation
and guest/host transitions. Authority timing includes observing installed agent
and budget grants and the worker claiming the intent; it is not a pure CPU
benchmark of capability validation.

llama.cpp's own timers separate input evaluation (prefill) and output generation:

| Model request | Prefill | Generation | Input tokens | Output tokens |
|---|---:|---:|---:|---:|
| Broker decision | 19.110 s | 3.410 s | 1779 | 30 |
| Worker tool selection | 7.429 s | 2.849 s | 697 | 26 |
| Worker final response | 5.531 s | 2.410 s | 497 | 22 |

The model calls account for roughly 79% of response latency. Prefill alone
accounts for about 32.07 s (62% of the 51.91 s). A short user prompt still
expands to substantial broker/tool/schema and session context.

Within VM startup, the host observed kernel entry about 4.04 s after QEMU
spawn. From there to init founding was about 4.86 s, including a roughly
4.02 s interval between HostLink service readiness and display service founding.
Init founding to the resident broker initialization marker took 1.53 s.
The detailed report records all 17 founded/ready service markers. These markers
can be buffered, so the gap is a lead for further profiling, not proof that the
display service itself consumed those four seconds.

The worker teardown was observed 46 ms after its answer. The runtime then waited
about 2 s for the guest to return to idle, retired it when that did not happen,
and restarted llama.cpp (1.867 s loading/readiness). Both failed greetings waited
about 5 s for unconfirmed cleanup, followed by VM shutdown/model restart.
Consequently this batch did not achieve a warm retained VM between prompts.
The first model-server start in this batch took 2.028 s; a preceding fresh-state
attempt measured 16.358 s for startup, then failed on a missing workspace-state
directory. That setup defect was corrected before the three-prompt batch. The
startup samples have different filesystem-cache conditions and are not an A/B
performance comparison.

## Measurement boundaries

- `runtime_elapsed_ms` uses one host monotonic clock per prompt, starting before
  project/model initialization. It also appears on failed/cancelled paths.
- Boot markers are host observations of serial lines, polled every 10 ms. They
  are not guest CPU counters or authority/readiness signals.
- The report's phase table is sequential. Boot detail, per-model, tool, VM pause,
  event-publication, and host-loop duty numbers overlap it and must not be summed
  a second time. Polling sleep runs concurrently with QEMU/model work.
- Native pixel presentation and isolated TCP/virtio wire round-trip latency are
  not measured by the terminal client. CLI trace includes local receipt times.
- First model chunk observation can be buffered until completion and must not
  be treated as true model time to first token. llama.cpp's final cumulative
  timings are used for prefill and generation; SSE cumulative samples are never
  added together. Missing/truncated observations stay unavailable.
- Synchronous event publication consumed 0.534 s in the file-read run and is
  included in its phase durations. Diagnostic logging and serial observation
  add some overhead. These are individual measured runs, not percentile claims.
- Builds and launcher preflight precede prompt admission and are excluded from
  these response-latency numbers; the launcher retains their timestamped logs.

Historical verification: Rust runtime/CLI/harness tests, incremental serial-marker regression,
and Python report/session tests passed. The real batch correctly exited nonzero
because two of its three prompts failed; no successful answer was synthesized.
