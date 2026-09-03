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

Regenerate the reference with:

    python3 -m http.server 8899 --directory <prototypes>
    msedge --headless=new --force-device-scale-factor=1.25 \
      --window-size=1204,777 --virtual-time-budget=4000 \
      --screenshot=design-reference.png http://127.0.0.1:8899/HarnessView.dc.html
