# Native UI cancellation and folder-picker fixes — 2026-09-24

This follows the failures recorded in [the initial UI acceptance run](native-ui-acceptance-20260924.md).
Changes are local to vKore; the Kore ISO and libkore pins are unchanged.

## Changes

- Desktop folder selection is owned by the UI. On macOS it uses an asynchronous
  `NSOpenPanel` sheet attached to the app window; closing the owner cancels it.
  The harness rejects the former picker endpoint immediately. It cannot launch
  an unattended desktop helper or block health, event or cancellation requests.
  Windows/Linux helpers also live in the UI, off the request thread, and are
  cancelled when their owner is dropped. Those platforms were not exercised here.
- A guest terminal response begins a separate five-second retirement phase,
  rather than inheriting an already expired cancellation/request deadline.
  One synchronous model-restart tick can be excluded from the guest wait budget;
  subsequent restart ticks receive no additional credit. Success still requires
  response publication, receipt acknowledgement and `finish_frontend_intent`.
- Model management HTTP responses carry an absolute deadline through headers
  and body. Previously socket timeouts were retried indefinitely by a reader
  shared with inference streaming. Inference streaming retains cancellation
  polling; management requests now expire even if a server stalls mid-response.

- A shared model restart no longer invalidates preopened agents that have never
  accepted work. Used or already invalidated sequences still fail closed. The
  old behavior let cancellation finish but refused the next newly claimed agent
  with `BackendUnavailable`, disconnecting HostLink.

## Automated checks

`cargo check -p vkore-ui -p vkored`, release builds and `make dev-check` passed.
The selected suites passed 252 tests: 96 host unit, 15 interactive harness,
8 project/API and 133 UI tests. Nine host native-gate tests and one UI test were
ignored by these commands; this is not a claim that those gates were rerun.

The regressions cover a late cancellation response after a slow tick, bounded
restart compensation, HTTP expiry before headers and during a partial body,
immediate rejection of a headless picker request followed by a healthy API,
and model-restart handling that preserves unused agents while invalidating
used sequences and delivering each pending failure once.

## Native checks and evidence

Evidence root:
`~/.local/state/vkore-supervisor-ui-20260923/diagnostics/ui-fixes-20260924/`.
The original failures and the intermediate failed cancellation attempts are
retained separately. Source patches, picker source, test logs and transcripts
identify each tested build; screenshots remain in the computer-use task.

The native folder sheet opened, Cancel returned to the project selector, and
selecting the dedicated `workspace/ui-picker-check` folder opened its project.
While the sheet was open, health returned `ok: true` in 19.6 ms and runtime
responded in 0.4 ms; no `osascript` process was running. A second check held
the sheet open for more than 35 seconds during real inference: all eight health
and eight runtime probes succeeded (maximum 57.17 ms and 6.16 ms respectively).

The final run (`ui-current-kore-integration-1790284654423`) passes both checks.
A counting request was cancelled during active inference. Kore published its
cancel response, the host performed one 30.993-second model restart, and guest
retirement completed about 31.1 seconds after the cancellation request. The UI
showed **Task cancelled** and **Assistant ready**, with terminal status
`cancelled`. Without Retry, app restart or guest reboot, the next prompt
`What is 17 plus 25? Answer with the number only.` returned exactly `42` and
completed in 102.19 seconds. QEMU PID remained **78662** and supervisor generation
remained **1**; the model incarnation advanced to **2** as expected.

The final build also passed a 20-second picker-open probe during that follow-up:
all five health and five runtime requests succeeded, with maxima 22.34 ms and
5.19 ms. Cancelling the sheet and project selector returned to the running chat.
The fixture `test.txt` remains exactly `hello`.

Final source, binary hashes, logs and transcript are under `attempt-5/`, with
`fix-run.json` indexing the results. These host/UI fixes are submitted for
review with this report; the default lock has not been promoted. The full fault/containment
matrix and Windows/Linux native UI were not rerun. The app is left running for
manual inspection.
