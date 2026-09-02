# Native Axiom UI upgrade evidence

These captures use the same populated vKore conversation fixture at 1280×800
on an Apple M1. The release-mode probe performs 10 warm-up frames followed by
60 measured frames. It lives in `crates/vkore-ui/src/main.rs` as the ignored
`tests::capture_and_benchmark_ui` test.

| Metric (median; p95) | Before: direct full-frame path | After: retained structure v1 | Change |
| --- | ---: | ---: | ---: |
| Display-list record | 0.244 ms; 0.310 ms | 0.044 ms; 0.064 ms | 81.9% lower median |
| Changed-frame CPU raster | 32.508 ms; 32.745 ms | 5.413 ms; 5.609 ms | 83.4% lower median |
| Exact stable-frame submit | 32.508 ms full repaint | 0.002 ms retained comparison | 99.994% lower median |
| Approximate stable record + submit | 32.753 ms | 0.046 ms | 708× faster |
| Presentation bytes | 4,096,000 | 4,096,000 changed; 0 stable | Stable copy eliminated |

The raster measurement excludes the platform present call. Presentation byte
counts are explicit backend counters. A changed frame still renders directly
into the mapped XRGB surface. An exact stable `Frame<System>` bypasses target
acquisition, rasterization, and presentation entirely.

- [Before](before.png): legacy `axrd::axui_direct::render_xrgb` at vKore commit
  `a781d5d6ffbd2c36c4c2462b12a37abe092acf84`.
- [After](after.png): deterministic `axui-raster` structure path with retained
  frame admission and bounded resources.

Reproduce the after measurement from the repository root:

```sh
VKORE_UI_CAPTURE=/tmp/vkore-after.ppm \
  cargo test -p vkore-ui --release \
  tests::capture_and_benchmark_ui -- --ignored --exact --nocapture
```

The repository uses its pinned nightly toolchain. On a host where another
`cargo` shadows rustup, invoke the pinned toolchain binary explicitly.
