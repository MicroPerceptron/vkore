# Native Axiom UI upgrade evidence

These captures use the same populated vKore conversation fixture at 1280×800
on an Apple M1. The release-mode probe performs 10 warm-up frames followed by
60 measured frames. It lives in `crates/vkore-ui/src/main.rs` as the ignored
`tests::capture_and_benchmark_ui` test.

| Metric (median; p95) | Scalar reference before review | Shared Axiom kernels | Change |
| --- | ---: | ---: | ---: |
| Full 1280×800 raster | 33.929 ms; 34.083 ms | 19.711 ms; 20.134 ms | 41.9% lower median |
| 448×128 damaged-region raster | 7.853 ms; 7.904 ms | 5.165 ms; 5.207 ms | 34.2% lower median |
| Exact stable-frame submit | — | 0.0032 ms; 0.0033 ms | No raster or present |
| Executed items | 866 scalar | 836 kernel; 30 fallback | 96.5% kernel-dispatched |
| Pixel equivalence | Reference | 0 differing pixels; max channel delta 0 | Exact |

For continuity with the original UI upgrade measurement, the legacy direct
quality renderer took 32.508 ms median (32.745 ms p95) for a full frame. The
new shared-kernel path is 39.4% faster than that legacy renderer as well as
41.9% faster than the scalar retained implementation it replaces. Display-list
recording is 0.125 ms median (0.183 ms p95). Approximate stable record plus
admission is therefore 0.128 ms, about 255× faster than the legacy full repaint.

The raster measurement excludes the platform present call. Presentation byte
counts are explicit backend counters. A changed frame still renders directly
into the mapped XRGB surface. An exact stable `Frame<System>` bypasses target
acquisition, rasterization, and presentation entirely.

The after path deliberately measures comparable visual quality: bundled DM
Sans OpenType shaping and grayscale masks, analytic signed-distance geometry
edges, Gaussian retained shadows, and a retained linear gradient are all
enabled. Solid rounded quads, borders, shadows, and glyph masks dispatch
through the shared `axnn::RasterBackend`/`axnn-cpu` operations. The scalar path
is retained only as the deterministic conformance/fallback implementation.
The probe renders both paths and requires exact output equality.

- [Before](before.png): legacy `axrd::axui_direct::render_xrgb` at vKore commit
  `a781d5d6ffbd2c36c4c2462b12a37abe092acf84`.
- [After](after.png): shared-kernel `axui-raster` structure path with retained
  frame admission and bounded resources, pinned to Axiom
  `fa30004c1275865a657894bc6c91fa664e515ecc`.

Reproduce the after measurement from the repository root:

```sh
VKORE_UI_CAPTURE=/tmp/vkore-after.ppm \
  cargo +nightly-2026-07-13 test -p vkore-ui --release \
  tests::capture_and_benchmark_ui -- --ignored --exact --nocapture
```

The repository uses its pinned nightly toolchain. On a host where another
`cargo` shadows rustup, invoke the pinned toolchain binary explicitly.
