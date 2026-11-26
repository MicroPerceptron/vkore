# The desktop

`vkore-ui` is vKore's desktop: Kore machines, and their volumes browsed from the
host through Kore's own engine. The desktop shares its local machine state with
the `vkore` command line. Signing in is optional for local use.

```sh
vkore-ui [--state DIR] [--open DISK|@MACHINE] [--theme system|light|dark]
```

`--open` lands at that volume's root; `--state` is the same directory the
command line uses.

The sidebar groups Machines and Volumes, with Settings at the bottom. Shared
page headings, native vector icons, status badges and cards work in both color
schemes. Creation and detail panels scroll independently on smaller windows;
visible scroll thumbs show when more controls are below the fold.

Pages share a centered content area up to 1440 logical pixels wide, with
24-pixel gutters. Settings stacks its cards on compact windows and places them
side by side when its content area reaches 1240 pixels. The machine creation
form grows up to 800 logical pixels. Window sizes and breakpoints use logical
pixels: the initial window is 1280×800 and the minimum is 900×560, independent
of display density.

## Settings

Open **Settings** in the sidebar, then choose **System**, **Light** or **Dark** under
**Appearance**. Each choice includes a preview of the workspace.

The **Text** section offers four sizes (90%, 100%, 115% and 130%) and three
bundled interface-font presets: Space Grotesk, DM Sans and JetBrains Mono.
Changes apply immediately across pages, with a live sample and **Reset text
settings**.

Typography is passed explicitly through the views, so windows/tests do not
share a mutable font preference. Page titles retain Archivo; paths, filenames and
logs retain JetBrains Mono and the same escaping rules. No host fonts or arbitrary
font paths can be selected. Text and control heights scale together; the logo
keeps its brand proportions.
The choice applies immediately to all pages, fields, status colors and the
launch splash, and is saved in the state directory for the next launch.
**System** is the default and follows changes to the host color scheme: native
notifications on macOS, and the desktop settings portal on Linux. If the host
reports no preference, it uses dark mode and says so in Settings. Existing
saved Light/Dark choices remain fixed. The logo keeps its original colors in
both modes. Window chrome follows the choice where supported by the host.

`--theme system`, `--theme light` or `--theme dark` overrides the saved preference for
that launch. Choosing an appearance in Settings saves it for subsequent launches.

**This device** shows the active state directory and explains where account
credentials are stored. Appearance, text preferences and the saved API port
live in `state.sqlite3` under that directory. The first open imports the existing
`appearance`, `typography` and `api-port` files and leaves them available to older
builds. Invalid legacy values use defaults with a message in **This device**,
which also reports database open errors. Credentials live in the OS keyring.

Under **Local API**, enter a port from 1 to 65535 (default 8080) and choose
**Save port**. **Start API** also saves the field before starting. The saved port
is remembered for the next launch, but the server never starts automatically.
The desktop binds only to `127.0.0.1` and displays its actual listening URL.
Changing or saving a port while the API runs leaves that listener in place;
**Stop API**, then **Start API**, applies the new port. A port already in use or
a failure to save is reported in the Local API panel. Closing the desktop stops
the API and its requests. The settings page scrolls on smaller windows.

Guest inference remains disconnected; starting a machine does not advertise a
model. See [the API contract](inference.md). The desktop's saved port is separate
from the CLI's explicit `vkore serve --listen` option.

## Plugins

Open **Plugins → Add plugin** and choose a local `.wasm` file. The library
accepts validated WebAssembly components, including WASI-P2 bundles, and core
Wasm modules, including WASI-P1 binaries. Open a machine and use its
**Plugins** tile to load a bundle. **Reload** replaces that machine's copy
from the source file after validation. If a reload fails, the previous copy
remains loaded. **Unload** releases the attachment.

The library and attachments are scoped to the current desktop session.
Loading validates the format and keeps the bundle available in the host
workspace. The machine integration ABI and guest execution are future work.

### Animated canvas sample

The library includes a bundled WASI-P2 sample. In a desktop built with
`gpu-canvas`, click **Preview** to run it in the Plugins page. From the
repository root:

```sh
cargo run --locked -p vkore-ui --features gpu-canvas
```

The [canvas WIT world](../crates/vkored/wit/gpu-canvas/canvas.wit) gives the
component one draw import. The example supplies the WGSL shader and updates
its animation uniform. vKore opens Metal on macOS or Vulkan on Linux, renders
a 512×288 texture at up to 15 frames per second, and reads it back for the
existing Axiom compositor. The pane displays the selected adapter and pauses
rendering when you navigate away.

Use a component you trust for the experiment. The worker limits Wasm memory
and fuel, and its WASI context has no network access, directory grants or
inherited process state. The host compiles the supplied shader and submits it
through the system GPU driver. Browser WebGPU and the proposed `wasi:webgpu`
interfaces require their own bindings.

To rebuild the bundled example after changing its source:

```sh
rustup target add wasm32-wasip2
cargo build --locked --release --target wasm32-wasip2 --manifest-path examples/gpu-canvas/Cargo.toml
cp examples/gpu-canvas/target/wasm32-wasip2/release/vkore_animated_canvas.wasm \
  crates/vkore-ui/assets/plugins/animated-canvas.wasm
```

To verify frames through the desktop renderer, set `VKORE_TEST_GPU_CANVAS` to
the bundled file and run `cargo test -p vkore-ui --features gpu-canvas gpu_component_frames_render_in_the_native_canvas_pane -- --nocapture`.

### Boa JavaScript sample

The `javascript-canvas` example compiles the Boa JavaScript engine into a
WASI-P2 component. Boa evaluates the bundled script to compute the animation
uniform each frame. The component uses the same canvas interface as the bundled
sample.

```sh
cargo build --locked --release --target wasm32-wasip2 --manifest-path examples/javascript-canvas/Cargo.toml
cargo run --locked -p vkore-ui --features gpu-canvas -- --gpu-canvas \
  examples/javascript-canvas/target/wasm32-wasip2/release/vkore_javascript_canvas.wasm
```

The resulting file can also be added to the plugin library and attached to a
machine. Use `--gpu-canvas` to execute it in the desktop preview.

### Native Servo browser

In a machine workspace, open **Plugins → Open Servo browser**. The tile works
with a stopped machine. Enter an HTTPS address, click inside the page to type,
or open a second tab. Each tab keeps its own page, history and form contents.

Tabs carry the page's icon and title, with a turning mark while the page loads;
the full title and address are a tooltip. The toolbar has back, forward and
reload, and an address bar that shows a lock for a secure page. A line under the
toolbar follows the load, the link under the pointer appears under the page, and
the pointer takes the shape the page asks for. The renderer name at the foot
comes from Servo’s active graphics context.

Build the companion executable once and launch the desktop from the repo root:

```sh
cargo build --locked --manifest-path tools/servo-browser/Cargo.toml
VKORE_SERVO="$PWD/tools/servo-browser/target/debug/vkore-servo" cargo run --locked --release -p vkore-ui
```

For distribution, place `vkore-servo` beside `vkore-ui`. The standard desktop
starts the helper only when the browser tile opens. The helper uses the host's
network and runs under the desktop user's account. It embeds native Servo 0.8
and uses its own SQLite dependency. The machine's guest OS, network adapter,
accelerator assignment and vKore download credentials are independent of it.

One helper owns up to four WebViews. Navigation, input and presentation
acknowledgments travel over private pipes, and so does what the page says about
itself: its title (256 characters at most), its icon (reduced to 32 by 32
pixels in the helper), the link under the pointer, its pointer shape and its
load progress. Every one of these is bounded before it is read. The helper
reads the commands already waiting and merges consecutive pointer moves, wheel
deltas and resizes, so a burst costs one pass through the engine. The helper
and the desktop must come from the same release: each checks the protocol
version (4) and the tile reports an incompatible helper. The OS composites the browser
surface into the tile. The desktop's Axiom renderer continues to draw its
controls. Page pixels stay on the graphics path. The physical tile size and
display scale determine the viewport, up to 16 megapixels.

| Desktop | Presentation |
| --- | --- |
| macOS | The helper copies each rendered frame to an IOSurface on the GPU. A private Mach port delivers the surface to the tile’s Core Animation layer. |
| Linux, X11 or XWayland | An embedded graphics window has an empty input region. vKore handles keyboard and mouse input. |
| Linux, native Wayland | EGL exports a GPU snapshot via DMA-BUF. A private socket carries the descriptor and the compositor imports it into a subsurface. Requires the driver’s single-plane export, Linux DMA-BUF v3 and viewporter. |
| Windows (experimental) | Desktop Window Manager composites the helper's live client area. Windows desktop integration still needs a runtime test. |

If the helper stops while it has been running, the desktop starts another and
reopens the tabs where they were. The tab that was in front is the likeliest
cause, so it comes back stopped, with a button to reload it when you choose, and
so does every tab if the helper stops again within twenty seconds of a restart.
After three restarts in two minutes it stops trying and says so. A helper that
never started (a missing binary, an unsupported display) is not retried.

An unsupported driver or compositor produces a visible error. To check the
installed helper against a real desktop compositor, set `VKORE_SERVO` to its
absolute path and run `cargo run -p vkore-servo-wire --features presentation --example servo_surface_smoke`.
To measure the helper's command handling without any network, run
`cargo run --release -p vkore-servo-wire --features presentation --example servo_latency`
with `VKORE_SERVO` set the same way. It reports the paint round trip and the
time to get through a burst of 600 pointer moves, on the blank tab the helper
opens with. Setting `VKORE_LIVE_URL` to an HTTPS address also loads it and prints
each event the page produces with its timing; that one uses the network.
Two environment variables help when a page misbehaves. `VKORE_SERVO_CONSOLE=1`
forwards the page's console messages to the helper's stderr (cut, and with
control characters replaced), which shows a script error directly.
`VKORE_SERVO_WEBGL=off` makes WebGL unavailable, so a page that tests for WebGL
falls back. For example, github.com throws without constructable stylesheets
(enabled in the helper) and, with WebGL on, never finishes loading: its 3D hero
runs and the helper stays alive and silent (Servo's WebGL then panics when the
helper shuts down, which does not matter once it is being ended). With WebGL off
it finishes loading in about five seconds. The OpenGL "texture unloadable"
line Apple's driver prints once comes from page painting, not from either.
The smoke test opens a test window and loads example.org. Append
`-- 'https://www.google.com/search?q=rust+language'` to test a search navigation.
On macOS the test also checks the tile for visible page pixels. The smoke test reports the final page, including redirects. The repo’s Cargo
configuration sets an 8 MiB stack per Stylo
worker at build time so the development helper can lay out complex pages.

### Kitesurf browser sample

Kitesurf lays out and renders webpages inside a WASI-P2 component. The
desktop sends pointer input and displays the returned frames. The preview
uses a 720×420 viewport that scales with the tile.

Build the component from the pinned Kitesurf source with Node.js, Git, and the
vKore Rust toolchain installed.

```sh
rustup target add wasm32-wasip2
examples/kitesurf-browser/build.sh
cargo run --release --locked -p vkore-ui
```

Add `artifacts/kitesurf-browser.wasm` through **Plugins → Add plugin**, then
open a machine and use **Load → Open browser** in its **Plugins** tile. The
browser opens alongside the other machine tiles. Move it by dragging the
title bar, resize it with the divider, or expand it with **+**.

Enter a public HTTPS address or a domain such as `example.com` and press
**Go** or Enter. Follow links by clicking the page and scroll with a wheel
or trackpad. **Back** returns to the preceding page. **New tab** opens
another WASI instance with independent page state, up to four tabs. The
bundled pages remain available at `index.html` and `about.html`. Rebuild the
bundle from this checkout to enable web navigation in an existing setup.

The preview renders HTML, CSS, and common raster images. Websites that
require JavaScript, forms, cookies, or sign-in can load incompletely. The
bundled demo still runs its script through the Boa DOM bridge.

The tile runs on the host with the machine stopped or running. The desktop
allows one browser preview at a time. The standard desktop build includes
browser preview. A build with `--no-default-features` can store the bundle
and shows that preview is disabled. The debug build can take about two
minutes to compile the component on first open.

The [browser WIT world](../crates/vkored/wit/browser/browser.wit) exchanges
input and bounded RGBA frames. A host import fetches HTTPS documents and
supported resources with download limits and public-address checks on each
connection. The component has no direct socket or filesystem access. A
future native Kore host can implement the same fetch and presentation
interfaces.

To exercise the real component without a window, set `VKORE_TEST_BROWSER`
to the component path and run
`cargo test -p vkored --features browser-preview bundled_kitesurf_runs_script_and_follows_links`.
Set `VKORE_TEST_BROWSER_WEBSITE` to a public HTTPS page address and run the
`kitesurf_opens_a_public_https_page` test for a live navigation check.

## Volumes

Use **Choose image…** for the system file dialog, or **Choose physical disk…**
on Linux for a list of host disks and partitions with their sizes, partition
names and filesystem labels (when the host reports them). Device paths remain
visible so similarly named partitions can be distinguished. You can also
enter a path (a raw image, a Linux block device, or `@NAME`) or use a machine
button under the field. Opening starts at the volume root; it does not assume
that a screenshots folder exists. The header shows what is open
and the recovered DagFS format version and generation. Generation grows with
every publication Kore or vKore makes; format version identifies the recovered
root, not the version of the app or its current writer.

- **Places.** Terminal 0's and terminal 1's homes, and the volume root. A home
  nothing has written to yet says so; the machine creates it on its first boot.
  The breadcrumb walks back up.
- **Preview.** Selecting a file reads its head, at most 32 KiB, and never
  decodes a pixel. A screenshot shows its size and Kore's capture record: the
  kernel build that drew it, when, the mode (screen, region or window), the
  app and region, density, uptime, and any other `kore:` keys. Text shows its
  first 32 KiB with its line breaks; anything else shows its length and first
  bytes.
- **Open** copies the file to a private directory (`$XDG_RUNTIME_DIR`, mode
  0700) and hands it to the host's viewer (`xdg-open`, or `open` on macOS).
- **Save** copies the selected file, or the directory you are in, into the host
  folder in the field (your Downloads folder by default), creating the folder
  if it does not exist. It never replaces a file there.
- **Import** installs a host file or directory into the directory you are in,
  inside a terminal's home. It is one publication: the volume has all of it
  or none of it.
- **Remove** deletes the selected entry, after asking a second time.

A disk a running machine holds cannot be opened; stop the machine first. While
the browser has a disk open it holds the disk's lock, and another vKore cannot
open it; a machine started from this window closes the browser first.

Readable disks are identified automatically through Kore's engine without
writing to them. The chooser distinguishes detected DagFS, blank media, no
DagFS found, and volumes that could not be verified (including busy or
unsupported volumes). A host filesystem label or type is descriptive metadata,
not proof of DagFS. Linux names/types come from one bounded, unprivileged
`lsblk` query; missing host metadata does not prevent disk selection.

If identification needs permission, **Identify DagFS…** retries those devices
in one read-only authorization batch. Each probe releases its engine handles
before the next disk, and a cancelled request preserves the choices. Opening a
disk still establishes its own authorized browsing session; an identification
result does not grant continuing access.

### Linux disk permissions

If a physical disk denies ordinary read access, opening it requests authorization
through the desktop polkit agent (`pkexec`). The main desktop stays unprivileged;
no permissions or ownership are changed. Cancelling leaves the disk closed.
Polkit and a running desktop authentication agent are required for this path.

An authorized session is **read-only**: browse, preview, open in a host viewer,
and export. Import and Remove are disabled in both the UI and worker. Close the
volume to release authorization and its disk lock. Normal writable image-file
sessions retain their existing Import and Remove actions. This feature is a
browser through Kore's engine, not an OS filesystem mount.

The private `--vkore-volume-reader` entry point admits only the selected Linux
block device, verifies the polkit caller and device identity, and keeps the
engine's shared lock for the session. Its bounded protocol has no disk writes,
host file paths, or command-execution operations. Exports stream back in 64 KiB
chunks and are written by the ordinary desktop user using the existing confined,
non-overwriting exporter. Disconnects release the mount. The separate `--vkore-volume-identify` entry point verifies the same caller
and device identities and accepts at most 256 physical devices for read-only
metadata recovery. macOS supports image
files; its physical devices are not supported by the current engine.

The native image chooser uses MIT-licensed `rfd` 0.17 with platform FFI confined
to that dependency. Linux uses the XDG desktop portal (with its runtime desktop
backend, or Zenity fallback), without adding a GTK development-library dependency.
The chooser does not infer a volume format or bypass Kore's validation.

## Machines

The **Machines** page lists your machines. Choose **New machine** to open
the creation form, or select a machine to open its workspace. **All machines**
returns to the list from either view.

Each workspace has Console, Overview, Boot activity, and Plugins tiles.
Drag the empty part of a tile's title bar onto another tile to swap them.
Drag the dividers to resize the neighboring tiles. The tile header controls
collapse a tile or expand it to fill the workspace. Use **Show …** to reopen
a collapsed tile and the restore control to return an expanded tile to the
grid. Each machine keeps its layout for the current desktop session.

Under **Configure hardware → Networking**, choose **Isolated** (the default)
or **Shared (e1000e)**. Shared mode uses QEMU’s IPv4 NAT and applies at the next
launch. The guest image needs IPv4 support to use it. The USB adapter remains
isolated and no inbound ports are forwarded.

The list shows each machine and where its boot is: starting, kernel up,
services, shell ready, ready, degraded or failed. Select one to see its ISO and
the SHA-256 it last booted, its resources, and the project it stages.

- **New machine** takes a name, an image from the **official image dropdown**,
  and optionally a project folder to stage into terminal 0's home. The bundled
  [release catalog](image-catalog.md) supplies the URL, required SHA-256 and
  exact size; the desktop has no URL or checksum fields. No official artifacts
  have been published yet, so the dropdown and Create button remain disabled.
  Existing machines still work, and the CLI retains its explicit local/custom
  ISO workflow. Once releases are supplied, downloads run in the background
  with progress and **Cancel import**. vKore verifies the ISO, size and digest
  before creation; failed or cancelled transfers leave no partial image.
  Downloads live under the state directory's `images/`, named by SHA-256, and
  survive machine removal.
- **Start in a window** gives the guest QEMU's own window, where you use Kore
  directly and press PrintScreen to capture. **Start headless** runs it without
  one.
- **Stop** presses the power button, and Kore shuts itself down. A Kore from
  before `7074bda2` on Kore's main (v0.40.0 among them) does not hear the button
  under QEMU, so after 10 s vKore ends QEMU and says so. Either way the volume
  keeps its last committed state.

The pane under the controls follows the boot as it happens and keeps the last
run's record after the machine stops. Starting a machine whose disk the
Volumes page holds closes the browser first, and opens it again when the
machine stops.

## Screenshots, end to end

1. Start the machine in a window and press PrintScreen in Kore. Kore saves the
   capture into `pictures/screenshots` in terminal 0's home, a folder vKore
   creates before each boot because Kore's capture app saves only into one
   that exists.
2. Stop the machine once Kore has saved the capture: it writes it a few seconds
   after the press (up to ten under TCG), and a stop does not wait for one
   still being written. The Volumes page reopens the volume root. Choose **Terminal 0**,
   then `pictures/screenshots`; captures are sorted by their UTC names.
3. Select a capture to read its record; **Save** it, or **Open** it in your
   image viewer.

The command line does the same: `vkore volume @NAME shots` lists them with
their record and `--get DIR` copies them out.

## Text from a guest

File names, logs and capture records come from the guest, and host paths come
from the host. The desktop shows them as they are, not as they would like to
look:

- a character the face cannot draw shows as its escape, `\u{…}`, rather than
  a blank box or a dropped frame;
- controls, direction overrides, zero-width characters and look-alike spaces
  show as escapes too, so `\u{202e}gnp.exe` cannot pass for `exe.png`;
- a backslash shows doubled, so an escape cannot be forged.

Logs keep their line breaks and tabs. Only the faces the desktop bundles
(Archivo, Space Grotesk, JetBrains Mono, and Axiom's DM Sans) are used, so it
draws the same on every host whatever fonts the host has installed.

## Limits

- Images are previewed by their record only: the renderer draws RGBA
  bitmaps, but the desktop does not decode a screenshot yet. **Open** shows
  them in the host's viewer.
- The desktop needs Axiom, which is not public yet.
- Terminal resize updates the host grid; the serial connection does not yet
  notify guest applications of geometry changes.

## Guest terminal

The connected COM2 console is a VT terminal. Click its screen to focus it; the
accent border shows where keyboard input goes. Characters, Enter, Backspace,
Tab, arrows, Home/End, Delete, function keys and Ctrl shortcuts go directly to
the guest. There is no host command editor or local echo: Kore handles editing,
history, completion and Ctrl-C. Application cursor mode follows the guest.

Ctrl+Shift+V (or Cmd+V) pastes plain text, honoring guest bracketed-paste mode.
Multiline paste can execute commands when the guest has not enabled that mode.
Embedded control sequences and oversized pastes are refused. Input is bounded
to 16 KiB, preserves typeahead and partial writes, and is never replayed after a
disconnection. Clipboard reads that finish after focus changes are discarded.

Click elsewhere, change pages, deactivate the window, or press Ctrl+Shift+Esc to
release the keyboard. Stopped sessions show a read-only screen. The serial
grid adapts to the pane width, window height and text-size preference, using
the bundled mono face's actual cell advance. The terminal occupies 60% of the
page height, with bounded scrollback and a 160–1600 logical-pixel height. The
grid is bounded to 512 columns and 256 rows. Light, dark and system appearance
apply to the terminal surface, cursor and ANSI palette too; explicit RGB colors
from the guest remain unchanged.

Resizing preserves terminal modes and uses the VT parser's screen-resize
semantics (shrinking can crop existing columns; new output wraps at the new
width). It does not inject a guest terminal-size protocol into COM2: programs
that maintain their own geometry are not yet notified. Stopped screens retain
their last grid dimensions. Cursor positioning, colors and alternate screens use
the existing VT parser. Guest control strings cannot change the host clipboard
or window. All visible guest text still uses the bundled mono face and the
usual control/unsupported-glyph escaping.

## An experimental web desktop

[`apps/vkore-tauri`](../apps/vkore-tauri/README.md) is a second, experimental desktop: the same engine behind a Tauri web front end in a different theme. It lives in its own Cargo workspace, is not built by CI, and covers an overview, the machine list and a machine's boot and resources so far.
