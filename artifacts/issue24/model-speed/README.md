# Model speed rendering

These native renderer captures replay the actual completed hello trace from
`run.20260914T230641Z.1fOLay` at 1182×900 and 700×900. Both the compact final
measurements and legacy detailed records were present; each model call is shown
once. The surrounding shell is a labelled trace-preview fixture, not a live
readiness claim. The full 58-second turn includes cold preparation.

- [Desktop](model-speed-1182.png)
- [Narrow](model-speed-700.png)

Reproduce through the existing ignored `tests::capture_and_benchmark_ui` test,
setting `VKORE_UI_TRACE` to a JSONL transcript, `VKORE_UI_CAPTURE_FAST=1`,
`VKORE_UI_WIDTH`, `VKORE_UI_HEIGHT`, and `VKORE_UI_CAPTURE` to a PPM output path.
