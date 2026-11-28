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
- **Workspace**: each machine opens as a set of tiles, as in the iced desktop:
  Console, Boot, Events, Resources, Configuration, and the Volume and Hardware
  editors below. Drag a tile's tab onto another tile to make it a tab there, or
  onto an edge of one to split it; drag the dividers to resize; expand a tile to
  fill the workspace, close it, and reopen it from the **Tiles** menu. The layout
  is kept per machine (in `localStorage`, when available) and **Reset layout**
  restores the default. Tiling is `dockview-react` (MIT, one dependency, pinned,
  vetted for licence and release age); a saved layout that names a tile this
  version does not have is ignored rather than trusted.
- **Agent**: a tile where a model operates the machine: it reads the console,
  boot events and a stopped disk, and types, starts or stops only with your
  approval. Off until you turn it on for that machine. It needs an
  OpenAI-compatible endpoint, hosted or local; [docs/agent.md](../../docs/agent.md)
  has the variables, what leaves this computer and the bounds. The tile is in the
  default layout; a layout saved before it existed gains it from the **Tiles**
  menu.
- **Sidebar**: the vertical menu: Overview, Machines, Volumes, and every machine's
  workspace with a state dot. It folds to icons with its toggle, or by itself in
  a narrow window.
- **Console**: a console image's COM2 shell, live. The engine's terminal model
  parses the guest's bytes, so the page only draws styled text; keys become
  terminal input, paste is bracketed when the guest asks, and the text reflows to
  whatever size the tile is. The engine's grid is never narrowed to fit a tile:
  doing so crops what the guest already printed. The guest's cursor is drawn as a
  line caret, and the terminal's colours follow light/dark and the colour preset.
  Shift+Tab leaves it.
- **Hardware**: an existing, stopped machine's CPUs, memory, CPU pinning with a
  host-core picker, native CPU waits, memory locking, page size (system, 2 MiB or
  1 GiB hugepages), guest memory nodes and network mode. The engine is the judge:
  the page asks `vkored::hardware::validate_host` as you edit and shows its words,
  and saving goes through `save_authorized`, which holds the machine's session
  lease, may wait on the desktop's authentication agent, and never changes what is
  saved when refused. The disk's size and storage reservation are fixed after
  creation, and assigned devices are shown but kept as they are.
- **Volumes**: a stopped machine's disk, read-only, through Kore's own
  filesystem engine (`vkore-volume`'s `Browser`), so you see what the machine
  sees. Terminal homes and the volume root, folders, and a preview of a file's
  first 32 KiB: text, a hex dump for anything else, and screenshots shown with
  the capture metadata Kore stamps on them.
- **Colour themes**: a palette button offers the logo studio's twelve palettes
  beside the default. A preset is eight colours (`src/themes/presets.ts`); the
  whole interface is derived from them for light and dark, and a test checks
  every preset and mode keeps text, muted text and buttons readable.

Creating machines, assigning devices, writing to a volume (install, remove, export),
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

## Launching on Linux: memory locking and polkit

A machine with memory pinning, hugepages or an assigned device needs a permission
the app does not have. The engine asks polkit for it by re-running **this
binary** under `pkexec` in a private helper mode (`--vkore-memory-helper`,
`--vkore-device-helper`, `--vkore-volume-reader`, `--vkore-volume-identify`). The
app answers those flags before it creates any window (`helper_mode`), exactly as
`vkore-ui` and `vkore` do, and a test runs the real binary to check that each
mode exits instead of starting the app. To give the prompts their own text,
install the policy for the binary you run:

```sh
python3 tools/install-linux-policy.py --executable <path to the vkore-tauri binary>
```

A desktop authentication agent must be running for the prompt to appear.

## Boundaries the code keeps

- The web side has twenty-one commands (`host_overview`, `machines_list`,
  `machine_get`, `machine_start`, `machine_stop`, `console_input`,
  `console_paste`, `console_resize`, `volume_open`, `volume_list`,
  `volume_preview`, `volume_close`, `hardware_get`, `hardware_check`,
  `hardware_save`, `agent_view`, `agent_enable`, `agent_disable`, `agent_send`,
  `agent_decide`, `agent_stop`) and three events (`machine-update`,
  `console-screen`, `agent-update`). `build.rs` declares them and `capabilities/default.json`
  grants exactly those, plus event listening. No file, shell, network or window
  access is granted. The agent's requests to its model are made by the Rust
  side, which keeps the key; the page has no network access.
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

`.npmrc` sets a 7-day `minimum-release-age`, so a fresh resolution takes nothing
published in the last week and agrees with a supply-chain policy that rejects
new releases; keep new dependencies' floors low enough for that to resolve.

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
- The agent has been run against a local `llama-server` on a pretend machine, and
  its tile driven on the scripted engine. It has not been run against a hosted
  provider, in this window, or on a real Kore machine.
- The app icon is a placeholder; the brand mark is not exported for it yet.
- Linux and Windows webviews are untested; this was built on macOS.
