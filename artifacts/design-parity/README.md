# Design parity evidence

Captures used to compare the harness against the MONOLITH reference.

- `design-reference.png` — the published prototype rendered headless at
  1204x777 logical, display scale 1.25, so it is directly comparable to a
  screenshot of the app on the same display.
- `app-current.png` — the release binary at the same size and scale.
- `hairline.png` — one horizontal slice through the sidebar divider in each,
  magnified. The design lands one pixel at `(42,42,48)`; the app landed two,
  at `(34,34,39)` and `(17,18,21)`, before pixel snapping was enabled.
- `nav-casing.png` — the sidebar footer, design beside app.
- `reference-harness-1280.png`, `reference-harness-agents-1280.png`, and
  `reference-harness-filesystem-1280.png` — the imported all-in-one Harness at
  the exact 1280x800 comparison viewport.
- `native-session-1280.png`, `native-agents-1280.png`, and
  `native-filesystem-1280.png` — the matching native Iced + Axiom frames after
  a real redraw event has resolved every control state. The topology views
  remain embedded Harness tabs; there is no standalone screen. The canvas
  geometry carries its own viewport clip, so paths and the dot grid cannot
  paint over the project rail or inspector. Automatic fitting may use the
  published 32% interaction floor instead of the mock's preferred 70% floor
  when needed, keeping every card wholly visible rather than cutting names or
  authority labels at a viewport boundary.
- `compare-session.png`, `compare-agents.png`, and `compare-filesystem.png` —
  reference, native, amplified difference, and 50/50 overlay in reading order.
- `final-wizard-0.png` through `final-wizard-4.png` and
  `final-approval.png` — mock-backed coverage for UI states whose complete
  backend data may not be present in a development daemon. These captures use
  Axiom's painter-ordered 2px page and 14px glass-surface backdrop filters.

All native captures use shape-derived control alignment: the shared button
labels fill the inner rectangle computed by Iced and center their paragraph
within it. A regression renders 26, 28, 30, 32, and 34px controls and checks
the recorded glyph-run center against the recorded button-quad center, so new
button sizes cannot silently return to padding-based alignment.

The release capture benchmark at 1280x800 records no shared-kernel fallbacks
and no pixel disagreement between optimized and scalar-reference output. The
empty session, agents, and filesystem full-frame medians are below 1.3ms. A
changed modal damage region is about 15.5ms; a stable retained submission is
3-5 microseconds and presents zero bytes.

The mock topology data covers information not yet carried explicitly by the
daemon event stream. Live events currently expose generations and tool-call
paths, but not stable agent identity/name/role/parent links or normalized
filesystem grant rights. Until those fields exist, the live adapter groups
each nonzero generation under the broker and labels path grants from the tool
name; the captures install the complete design fixture so every intended state
is still rendered and tested.

Regenerate the reference with:

    python3 -m http.server 8899 --directory <prototypes>
    msedge --headless=new --force-device-scale-factor=1.25 \
      --window-size=1204,777 --virtual-time-budget=4000 \
      --screenshot=design-reference.png http://127.0.0.1:8899/HarnessView.dc.html

Regenerate a native state with the ignored release capture test, for example:

    VKORE_UI_VIEW=agents \
    VKORE_UI_CAPTURE=artifacts/design-parity/native-agents.ppm \
    cargo +nightly-2026-07-13 test -p vkore-ui --release \
      tests::capture_and_benchmark_ui -- --ignored --exact --nocapture
