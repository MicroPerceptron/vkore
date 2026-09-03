# README media

`chat-harness.png` is generated from vkore-ui's populated 1280x800 session
fixture with the ignored `capture_and_benchmark_ui` test. It is a native
Iced + Axiom render, not a browser mock.

Regenerate it from the repository root with:

```powershell
$env:VKORE_UI_CAPTURE = Join-Path (Get-Location) "artifacts/readme/chat-harness.ppm"
$env:VKORE_UI_CAPTURE_FAST = "1"
cargo test -p vkore-ui --release tests::capture_and_benchmark_ui -- --ignored --exact
```

Convert the resulting binary PPM to PNG and remove the PPM before committing.
