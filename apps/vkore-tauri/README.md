# vKore on Tauri (experimental)

A second desktop for vKore: the same engine behind a web front end, in a
different theme (warm sand, deep teal, coral, serif headings) and with the
MicroPerceptron mark animated from the logo studio's presets. It is an
experiment beside the iced desktop (`crates/vkore-ui`), not a replacement for
it, and it is not part of the release train or CI.

## What it does today

- **Overview**: how many machines run, what they reserve against this host, and
  the host's processor, acceleration, QEMU and free storage.
- **Machines**: every configured machine as a card with its state, resources and
  a live boot track; start and stop.
- **Machine**: boot progress by stage (firmware, kernel, services, shell,
  ready), its resources against the host, its configuration and a bounded event
  log.

Creating and editing machines, volumes, the console and the browser stay in the
`vkore` command and the iced desktop for now.

## How it is built

- `src-tauri/` is a Rust crate in **its own Cargo workspace**, like
  `tools/servo-browser`: the webview stack is never built by the CLI, the iced
  desktop or CI. It depends on `vkored` and `vkore-vm` by path and starts a
  machine exactly as the iced desktop does (`Session::start`, `poll`, `stop`,
  `finish`) on one worker thread per machine. Closing the window stops every
  running machine first, so no QEMU outlives the app.
- `src/` is React, TypeScript and Vite, with Lucide icons and locally bundled
  Fraunces and Outfit faces. Nothing loads from the network.
- `src/mock.ts` is a scripted engine used only by `vite dev` outside the app,
  so the interface can be driven in an ordinary browser. It is compiled out of
  the production build.

## Boundaries the code keeps

- The web side has five commands (`host_overview`, `machines_list`,
  `machine_get`, `machine_start`, `machine_stop`) and one event
  (`machine-update`). `build.rs` declares them and `capabilities/default.json`
  grants exactly those, plus event listening. No file, shell, network or window
  access is granted.
- The content security policy allows scripts, fonts and images from the app
  itself only, and no inline script. The page renders every engine string as
  text.
- Every command validates the machine name before the store is opened. Strings
  from disk or a guest (machine and image names, project names, engine messages)
  pass through `shown`: control, bidirectional and invisible characters are
  shown as visible `\u{..}` escapes, and each field is cut at 240 characters.
  Paths are reduced to file names. Listings, logs (200 lines) and running
  machines (8) are bounded.
- A machine's disk has one owner: a machine that is already running cannot be
  started again, and nothing here opens a volume.

## Run it

Needs Node 22 with pnpm, and the Rust toolchain the repository pins. QEMU is
found as the CLI finds it (`VKORE_QEMU`, then the packaged runtime, then
`PATH`); the state directory is the CLI's (`VKORE_STATE`, then the platform
default).

```sh
cd apps/vkore-tauri
pnpm install --frozen-lockfile
pnpm tauri dev          # the app, with hot reload
pnpm dev                # the interface alone, on a scripted engine, at http://127.0.0.1:1420
```

Checks:

```sh
pnpm typecheck && pnpm test && pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

## Not yet done

- The window has been built and launched, and the interface has been driven in a
  browser on the scripted engine in light and dark, but a real Kore boot through
  this window has not been run: it is not the acceptance the repository asks
  for until a machine has booted the ISO here.
- The app icon is a placeholder; the brand mark is not exported for it yet.
- Linux and Windows webviews are untested; this was built on macOS.
