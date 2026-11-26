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
  ready), the machine's **console**, its resources against the host, its
  configuration and a bounded event log.
- **Console**: a console image's COM2 shell, live. The engine's terminal model
  parses the guest's bytes, so the page only draws styled text; keys become
  terminal input, paste is bracketed when the guest asks, and the grid follows
  the panel's size. Shift+Tab leaves it.
- **Volumes**: a stopped machine's disk, read-only, through Kore's own
  filesystem engine (`vkore-volume`'s `Browser`), so you see what the machine
  sees. Terminal homes and the volume root, folders, and a preview of a file's
  first 32 KiB: text, a hex dump for anything else, and screenshots shown with
  the capture metadata Kore stamps on them.
- **Colour themes**: a palette button offers the logo studio's twelve palettes
  beside the default. A preset is eight colours (`src/themes/presets.ts`); the
  whole interface is derived from them for light and dark, and a test checks
  every preset and mode keeps text, muted text and buttons readable.

Creating and editing machines, writing to a volume (install, remove, export),
physical disks and the browser stay in the `vkore` command and the iced desktop
for now, and graphical images have no display here yet.

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

- The web side has twelve commands (`host_overview`, `machines_list`,
  `machine_get`, `machine_start`, `machine_stop`, `console_input`,
  `console_paste`, `console_resize`, `volume_open`, `volume_list`,
  `volume_preview`, `volume_close`) and two events (`machine-update`,
  `console-screen`). `build.rs` declares them and `capabilities/default.json`
  grants exactly those, plus event listening. No file, shell, network or window
  access is granted.
- The content security policy allows scripts, fonts and images from the app
  itself only, and no inline script. The page renders every engine string as
  text.
- Every command validates the machine name before the store is opened. Strings
  from disk or a guest (machine and image names, project names, engine messages)
  pass through `shown`: control, bidirectional and invisible characters are
  shown as visible `\u{..}` escapes, and each field is cut at 240 characters.
  Paths are reduced to file names. Listings, logs (200 lines), running machines
  (8), console input (4 KiB a command, 256 queued), pastes (16 KiB, plain text)
  and each console screen (64 K characters, newest kept) are bounded. A console
  has its own thread, which ends when its machine does.
- A machine's disk has one owner. A machine that is already running cannot be
  started again; a volume is never opened under a running machine; one volume is
  open at a time; and starting a machine closes its open volume first, under the
  same lock that opening takes, so the two cannot interleave. Volumes are only
  read: a listing returns at most 2000 entries, a preview reads at most 32 KiB,
  and an image is read whole only up to 2 MiB and 16 million pixels. A file name
  is sent twice, raw (to navigate by, never drawn) and escaped (to draw).

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
  browser on the scripted engine in light and dark, the console's thread is
  tested against a real Unix socket, and the volume page's backend against a real
  formatted DagFS image populated and read back through Kore's engine. But a real Kore boot, and a real console
  session, through this window have not been run: it is not the acceptance the repository asks
  for until a machine has booted the ISO here.
- The app icon is a placeholder; the brand mark is not exported for it yet.
- Linux and Windows webviews are untested; this was built on macOS.
