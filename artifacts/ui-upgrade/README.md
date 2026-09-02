# Native Axiom UI upgrade evidence

These captures use the same populated vKore conversation fixture at 1280×800
on an Apple M1. The release-mode probe performs 10 warm-up frames followed by
60 measured frames. It lives in `crates/vkore-ui/src/main.rs` as the ignored
`tests::capture_and_benchmark_ui` test.

| Metric (median; p95) | Before: direct quality path | After: retained quality v2 | Change |
| --- | ---: | ---: | ---: |
| Display-list record | 0.244 ms; 0.310 ms | 0.133 ms; 0.193 ms | 45.7% lower median |
| Changed-frame CPU raster | 32.508 ms; 32.745 ms | 34.051 ms; 34.376 ms | 4.7% higher median |
| Exact stable-frame submit | 32.508 ms full repaint | 0.003 ms retained comparison | 10,134× faster |
| Approximate stable record + submit | 32.753 ms | 0.136 ms | 241× faster |
| Presentation bytes | 4,096,000 | 4,096,000 changed; 0 stable | Stable copy eliminated |

The raster measurement excludes the platform present call. Presentation byte
counts are explicit backend counters. A changed frame still renders directly
into the mapped XRGB surface. An exact stable `Frame<System>` bypasses target
acquisition, rasterization, and presentation entirely.

The after path deliberately measures comparable visual quality: bundled DM
Sans OpenType shaping and grayscale masks, Coverage8x8 geometry edges, soft
retained shadows, and a retained linear gradient are all enabled. Opaque
interior spans bypass supersampling, keeping changed-frame raster time near the
legacy quality renderer while preserving the stable-frame win. This capture
supersedes the earlier pixel-center/bitmap-font prototype.

- [Before](before.png): legacy `axrd::axui_direct::render_xrgb` at vKore commit
  `a781d5d6ffbd2c36c4c2462b12a37abe092acf84`.
- [After](after.png): quality-equivalent `axui-raster` structure path with
  retained frame admission and bounded resources, pinned to Axiom
  `57e20727f9886d95b8f65121b46de79b02c56484`.

Reproduce the after measurement from the repository root:

```sh
VKORE_UI_CAPTURE=/tmp/vkore-after.ppm \
  cargo test -p vkore-ui --release \
  tests::capture_and_benchmark_ui -- --ignored --exact --nocapture
```

The repository uses its pinned nightly toolchain. On a host where another
`cargo` shadows rustup, invoke the pinned toolchain binary explicitly.
