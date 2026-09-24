# Native Mac UI acceptance — 2026-09-24

Result: core file/chat interactions pass on the projection-fixed image. Full UI
acceptance is **not green**: active cancellation and the native folder-picker
path exposed separate failures. No merge-readiness or default-lock promotion.

## Tested stack and evidence

- Kore source/artifact base: `c6dd5fe714183c71475e5fa4372dfde9a7747c1d`.
- ISO SHA-256: `fef674e7906dfb84d36d16ba8a0c3d0f62e1ac37c3605d93c15b910b6237b367`.
- vKore head: `ff51da9bb113036283337f55f95994c07365dcd6`, with the local candidate
  lock/documentation update; runtime implementation unchanged.
- SDK: `688d5bfb40db47a63d5e0f45593898b42d6f9d61`; host native owner:
  `60e659644acf6e5e730b6b566a68765f8016563c`.
- Launch: `make dev-real`, real QEMU/Kore/llama.cpp. The empty dedicated workspace
  was backed up before testing. Input, tab changes, resize, cancellation and
  Retry were performed through native computer use and visually verified.
- Local evidence:
  `~/.local/state/vkore-supervisor-ui-20260923/diagnostics/ui-projection-20260924/`.
  `ui-run.json`, `prompt-outcomes.json`, complete copied transcripts, launcher/
  harness/serial logs, pre-retry records, picker failure and post-cleanup health
  records preserve the outcome. UI screenshots are in the computer-use task.

The macOS test bundle symlinks the release UI binary. Selecting it opened a
second frontend on the same launcher/harness; the original frontend stayed
idle. All six prompts below target session
`ui-current-kore-integration-1790275664937`. After project binding, QEMU PID
66832 remained unchanged through the four core prompts, cancellation and Retry.

## Observed checks

| Check | Result |
| --- | --- |
| Guest provisioning | 35/35 apps, 3/3 libraries, 2/2 data, package present, `app-fail: 0`. No projection timeout observed. |
| Create `test.txt` containing `hello` | Visible completion in 5m29s; independent host read is exactly five bytes `hello`. |
| Read `test.txt` | Visible completion in 2m7s; guest tool returns `hello\n`; model adds prose around `hello`. |
| Read missing file | Correct explanation after 2m58s; expanded activity shows `read` and `inspect` refusals for the nonexistent path. Known Failed status remains. |
| Arithmetic after refusal | Exactly `42`, visibly completed in 1m37s. |
| Draft and layout | Draft survives Session/Agents/Filesystem switches and task completion; 1280×832 and 1040×720 layouts remain usable. |
| Cancellation | **Fail:** active broker inference cancellation becomes Failed and readiness unavailable; details below. |
| Retry after cancellation failure | Ready restored; fresh arithmetic returns exact `42` in 1m38s, same VM, no automatic counting-prompt replay. |
| Folder picker | **Fail/incomplete:** waiting picker blocks harness responsiveness; actual native folder choice could not be completed. |
| Folder-free project creation | Pass; original and second project each restore their own draft when switching both ways. |
| New chat | Opens a distinct session with empty transcript; retains that project's draft. |

The Filesystem tab displays effective worker grants, not directory inventory:
the create worker showed a grant to `project`, without its new descendant file.
After worker retirement the grants disappeared while the host file remained.
`topology/filesystem.rs::Filesystem::from_events` reconstructs capability events
and does not enumerate directory contents. Keep this under the existing #28
visualization workstream; do not confuse it with failed file creation.

## Cancellation failure

Reproduction: submit `Count from 1 to 100, separated by commas.` and click Cancel
while the UI says the broker is selecting capabilities. The host records
`kore_cancellation_requested` at 29.998s. A guest `Prompt cancelled.` result is
observed at 46.852s, followed by `Kore cancellation was not confirmed` at 46.869s.
The UI reports Failed after 47s and exposes Retry because supervisor readiness
is unavailable. Retry restores readiness and the subsequent prompt succeeds.

`kore_runtime.rs::run_prompt_inner` sets a 10-second deadline when cancellation
is requested, and checks it before the next daemon tick/retirement processing.
The observed guest result arrives about 16.85 seconds after the request. This
locates the failure boundary; it does not explain all time spent cancelling or
prove that merely increasing the deadline is the correct fix. Preserve the
receipt/retirement evidence when investigating.

## Folder-picker failure

Reproduction: New project → Select or drop a folder. The UI remains at Waiting
for the system folder picker, with its controls disabled. The harness-owned
`osascript` child remains waiting. It is not an addressable app in this CUA
inventory, and no selectable dialog appeared in vKore or Finder's inspected
surfaces. Do not infer from this that no dialog existed anywhere on the desktop.

While the helper waited, the UI reported Assistant connection unavailable and
an HTTP probe timed out with no response after three seconds. That probe used
`/health`, which is not a registered route; it establishes stalled HTTP handling,
not a failed health response. Escape dismissed vKore's modal and preserved the
draft, but did not end the helper. Terminating only this test-created helper
restored UI readiness; the proper `/api/v1/health` then returned `ok: true`.

`interactive.rs` calls `native_picker` synchronously from `/api/v1/picker`;
the macOS helper uses blocking `Command::output()`. Picker waiting must not
block unrelated harness requests, and cancellation must own the helper's
lifetime. Native folder selection remains unverified. Creating a project
without a folder and switching drafts succeeded after recovery.

The app and VM were left available for manual inspection; only test drafts were
cleared. `test.txt` remains `hello`. No source fix was applied during these UI
checks. Forced supervisor death and the full fault/containment matrix were not
repeated here. Native accessibility exposes window chrome only, so these checks
used screenshot-based input rather than proving accessibility support.
