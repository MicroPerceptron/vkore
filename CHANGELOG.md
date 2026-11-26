# Changelog

Release versions follow [Semantic Versioning](https://semver.org/).
See [the versioning policy](docs/versioning.md) for contribution and release checks.

## [0.11.0] - 2026-11-25

### Added
- The native Servo browser tile shows what each page says about itself. Tabs carry the page's icon and title, with a turning mark while it loads and the full title and address as a tooltip. The toolbar adds forward and reload, the address bar shows a lock for a secure page, a line under the toolbar follows the load, the link under the pointer is shown under the page, and the pointer takes the shape the page asks for.
- The helper and desktop speak browser protocol 4: title, icon, pointer shape, link status, load phase, forward and reload. Titles, icons and link text are bounded before they are read, and icons are reduced to 32 by 32 pixels in the helper.
- If the Servo helper stops while it has been running, the desktop restarts it and reopens the tabs where they were. The tab that was in front comes back stopped, with a button to reload it, so a page that stops the engine cannot do it again in a loop; after three restarts in two minutes it gives up and says so.
- `VKORE_SERVO_CONSOLE=1` forwards a page's console messages to the helper's stderr, and `VKORE_SERVO_WEBGL=off` makes WebGL unavailable so pages fall back instead of running Servo's WebGL, which keeps a heavy scene such as GitHub's 3D hero from ever finishing loading.
- `servo_latency`, an opt-in measurement of the helper's paint round trip and its handling of a burst of pointer input, with no network unless `VKORE_LIVE_URL` is set, in which case it loads that address, paints as the desktop does, and prints each event with its timing.

### Changed
- The helper reads the commands already waiting and merges consecutive pointer moves, wheel deltas and resizes, so a burst of 600 pointer moves followed by a paint takes about 2 ms instead of about 31 ms on a blank tab.
- The desktop paints the browser tile as soon as the helper reports a new frame and redraws its interface only when something it shows has changed, rather than waiting for the interface's next paced redraw.

### Fixed
- Enable constructable stylesheets (`adoptedStyleSheets`) in the Servo helper. GitHub's web components throw without it, so github.com never finished loading.
- Build `vkore-servo-wire` on macOS with the system archiver, so a GNU `ar` earlier on `PATH` no longer produces an archive the linker rejects.

## [0.10.1] - 2026-11-25

### Changed
- Give the desktop depth and lead it with marks instead of words. Cards are lit from above and cast soft shadows, higher for what floats; the primary action is filled with the accent, selections and danger controls are faint tints, and state chips wash in their own colour. Hovers ease in and out instead of snapping.
- The icon set grows to 38 glyphs. Machine state is a beacon that spins while booting and pulses while running, with a five-segment boot track; commands are glyph buttons with tooltips; a machine's resources show as metric cards with rings, and long paths and hashes truncate with the whole value in a tooltip. Tile controls, page headers, the sidebar, Plugins, browser tabs, Account and Volumes use the same marks; a volume's facts are chips, its files carry folder and file glyphs, and the disk chooser shows each disk's DagFS state as a tinted chip. The hardware editor gets glyph section titles, stat chips, glyph steppers, info-led notes and tinted cautions and errors. Settings gets a lead per card, a pulsing state for the local API, a tick on the chosen theme and a chip for the credential store. Every control now eases on hover. Surfaces are solid fills with tight shadows, which keeps every frame on Axiom's SIMD raster path: a full-window frame at 2880 by 1800 takes about 7 ms where gradients and wide blurs took about 135 ms. The frame loop now repaints only the rectangles that changed against the frame on screen, so a spinner frame repaints under 1% of the window and takes about 6.5 ms instead of about 12.5 ms at that size; the result is checked pixel for pixel against a full repaint. Animated marks request frames only while they move.

## [0.10.0] - 2026-11-24

### Added
- Restore unexpired download sign-in across desktop and CLI launches from macOS Keychain or Linux Secret Service. Credentials are scoped to the state directory. The CLI adds `account status`, `account sign-in` and `account sign-out`.
- Show the active state directory, credential store and local sign-in status in Settings.
- Save desktop appearance, text preferences and the local API port in SQLite. Import existing preferences on first open and show the database location in Settings.

## [0.9.0] - 2026-11-23

### Added
- Machine hardware setup offers isolated networking or shared IPv4 through QEMU’s e1000e adapter for guests with an IP stack. Existing machines keep isolated networking.
- Native disk-image picker and a Linux physical-disk/partition chooser showing host partition names, filesystem labels, device paths and capacities.
- Read-only DagFS identification through Kore's engine, showing each recovered volume's actual format version. Permission-blocked disks can be identified together with one authorization request; labels never count as proof of DagFS.
- Desktop authorization for read-only browsing and exporting Linux physical disks that deny ordinary read access. A one-disk helper retains Kore's engine lock without elevating the UI or changing disk permissions; authorized sessions cannot import or remove files.

### Fixed
- Offer read-only authorization when Linux denies access to the parent disk needed to identify a DagFS partition. Report the recovered DagFS version in `volume info`.
- Hide the Machines download sign-in prompt while an authorized account session is active.
- Copy the sign-in code to the system clipboard directly from the Account page.
- Open volumes at their root instead of assuming a screenshots folder exists. Clear stale contents and disk ownership after failed opens, and prevent overlapping open requests.

## [0.8.0] - 2026-11-23

### Added
- Optional Account controls in Settings: browser Google sign-in, versioned agreement review, session status, cancellation and sign-out. Credentials remain in memory and expire within one hour; local features remain available offline.
- CLI `machine create --sign-in` performs the same browser authorization without putting credentials in command-line arguments or state files.

### Security
- Require authenticated, server-recorded agreement acceptance before new image transfers. The shared runtime gates desktop and CLI downloads, restricts credentials to the official service and refuses redirects and query-token shortcuts. Verified cached images and local ISO imports remain available without authentication.
- Requires the matching Placement server account API and mandatory download gate before remote downloads become available.

## [0.7.0] - 2026-11-20

### Added
- Turn the guest console into a directly focused terminal: type into the screen, use editing/control keys, and paste with Ctrl+Shift+V or Cmd+V. Remove the command field and Send button; render the VT grid and cursor without reflow, with bounded scrollback and typeahead. Release input focus on outside clicks, navigation, disconnect/reconnect and window deactivation; reject stale clipboard delivery.
- Adapt the terminal grid to pane width, window height and text size, and give it a light ANSI palette that follows Appearance. Preserve terminal modes and bound resize dimensions without injecting resize commands into the guest shell.

## [0.6.1] - 2026-11-20

### Fixed
- Record text whose line opens with glyphs that draw nothing (a space, a zero-width character) and then falls back to another face, instead of panicking in Axiom's recorder; the panic poisoned the shared font system and failed every later frame. Axiom is pinned at `139ddf7`.

### Changed
- Leave font confinement to Axiom, which from first use keeps the shared font system to its DM Sans and the registered faces; the desktop no longer removes host faces itself. A host copy of a bundled family, such as Ubuntu's `fonts-jetbrains-mono`, still cannot stand in for it.
- Serve the RGBA images a frame publishes from the desktop backend, beside its geometry masks, and refuse a frame with image errors as text and geometry errors are. Screenshots are still previewed by their record.

## [0.6.0] - 2026-11-19

### Added
- Native CPU waits (MONITOR/MWAIT) default on for supported pinned Linux KVM configurations, with host capability checks and verification before the guest runs; hardware setup follows CPU pinning, and CLI creation offers `--cpu-pm` / `--no-cpu-pm` overrides.
- Load official images from the live archive service without API keys, with desktop refresh, verified downloads, progress, cancellation and cache reuse. The CLI adds `machine images` and `machine create --image ID`.
- Allocate Linux hugepage backing at launch through the hardware helper, including empty pools and guests without accelerators; restore the temporary allocation allowance and release surplus memory after guest exit.
- Host-aware RAM and disk capacity presets, including intermediate sizes, plus precise decimal GiB input for RAM.
- Discover Linux GPUs and assign Intel xe SR-IOV virtual functions from hardware setup or the CLI. Done/Save can provision an absent VF pool through system authentication without detaching the host GPU or resizing existing pools.
- Choose a single guest NUMA node for inference or the two-node gate layout. New hardware recommendations use one node; existing configurations keep their layout.

### Fixed
- Resolve relative state directories before storage discovery and keep absolute paths for workers and privileged launches, including new directories and `..` components.
- Configure the native TLS connector explicitly for catalog and image HTTPS requests.
- Compact host memory before hugepage preparation and keep full guest RAM when hugepages cover only part of it. Fall back to mixed transparent/base pages without changing NUMA layout, and report realized backing in boot activity.
- Keep a guest line longer than the console grid as one line, in the screen and in scrollback, so the console pane reflows it instead of breaking it mid-word at column 120.
- Default new machines to 32 GiB disks in both desktop and CLI flows; report insufficient reservation space instead of silently shrinking the default.
- Scale QEMU startup negotiation patience with guest RAM (10 seconds plus 2 seconds per GiB) so large memory configurations are not killed by the former fixed 10-second deadline.
- Budget locked memory for every assigned accelerator, including GPU VFs, so combined NPU/GPU launches can map all guest RAM. Keep the physical-address check based on actual guest RAM.
- Use device-neutral `pt` names for QEMU passthrough devices and ports.
- Cap guest physical address bits at the negotiated Intel VFIO DMA width so firmware places RAM-backed PCI BARs within that address space.

## [0.5.0] - 2026-11-19

### Added
- Desktop console pane for headless images, with bounded recent output and explicit one-line command submission.
- Detect the console service in the ISO and attach its COM2 shell socket; COM1 remains the boot log.
- Repeatable `machine create --passthrough PCI_ADDRESS` for CLI device selection.
- Expand desktop disk sizing with GiB input, model-sized presets, a logarithmic slider, 32 GiB recommendations, and sparse capacities independent of currently free space.
- Add narrowly scoped Linux polkit descriptions for concise hardware/memory prompts, with an installer for exact source-build or packaged binary paths.
- Linux whole-NPU passthrough controls with persisted PCI identity, exclusive IOMMU-group admission, and QEMU VFIO/IOMMUFD launch support.
- A polkit-authorized device guardian that passes opened device handles to unprivileged QEMU, checks function-level reset, and restores host drivers after guest exit or failed launch.

### Fixed
- Accept Fedora OVMF descriptors that advertise optional SEV/TDX capabilities without enabling a confidential guest. Preserve Secure Boot, enrolled-key and SMM exclusions.
- Interpret console ANSI colors, cursor edits and erase commands incrementally instead of displaying escape codes; retain bounded terminal history and safe bundled-font rendering.
- Wait for desktop-started guests to stop and collect changes before closing; cancel pending starts and block new work during shutdown. A second close explicitly leaves immediately.
- Keep the machine status current, show assigned NPU addresses, and use image-neutral shell readiness wording.
- Test API shutdown using owned connections and backend release, avoiding assertions against reusable ephemeral ports.
- Negotiate Intel passthrough address width using paused, unprivileged QEMU probes with the selected devices; disable incompatible snoop control while retaining DMA translation and interrupt remapping.
- Isolate the device guardian from terminal Ctrl-C so the CLI owns graceful shutdown; explicit guardian signals and parent disconnect still restore host drivers.

### Breaking
- `BootMedium`, `MachineSpec`, `MachinePaths` and session events gain console information.
- `Resources` adds passthrough selections and `QemuCaps` adds probed VFIO/IOMMUFD support. Older saved configurations still default to no passthrough.

## [0.4.0] - 2026-11-19

### Added
- Read-only NPU discovery on Linux (Intel/AMD PCI devices), Windows (DXCore NPU adapters), and macOS (Core ML Neural Engine devices).
- Hardware setup device cards showing host identity and separate dedicated, SR-IOV and virtio-accel route assessments, including unavailable and unverified states.
- Independent cross-platform discovery tests and native inventory CI on Linux, Windows and macOS.

### Breaking
- `HostResources` adds an `npus` inventory field. Saved machine configurations and guest protocols are unchanged.

## [0.3.0] - 2026-11-19

### Changed
- Move the desktop version to the bottom of the sidebar and include the build commit, with explicit dirty and unknown-source labels.
- Check hardware permissions together on Done/Save; prompt only when elevation is needed and preserve the draft if authorization fails.

### Fixed
- Show the actual native host page size and explain why macOS QEMU uses automatic system pages; do not offer Linux-only hugepage controls on unsupported hosts.
- Distinguish unavailable hugepage backends from inaccessible or depleted Linux pools.
- Allow saved memory pinning to be turned off after moving a machine to a host that cannot support it.

### Breaking
- `HostResources` adds `system_page_bytes` and `hugetlb_supported` diagnostics. Rust consumers constructing snapshots must supply these fields; saved machine configurations remain compatible.

## [0.2.0] - 2026-11-19

### Added
- Official-image catalog and verified downloads with per-image resource minimums.
- Host-aware hardware configuration with CPU topology, core selection, memory and storage controls, Linux pinning and desktop authorization.
- Local OpenAI-compatible API controls and port configuration in Settings; guest inference transport remains pending.
- System, light and dark appearance, responsive desktop layout, and a visible app version.
- Persistent text-size controls and bundled Space Grotesk, DM Sans and JetBrains Mono interface-font presets with a live preview.
- Verified, relocatable QEMU runtime bundles built independently of Cargo.

### Fixed
- Correct macOS memory availability reporting and preserve reserved storage across guest TRIM.
- Bound QMP transport, match replies by ID, and clean up failed guest launches.

### Breaking
- Machine configurations now require at least 2 CPUs, 2 GiB RAM and 1 GiB storage, including existing configurations.
- Public Rust configuration/specification types gain resource fields. Consumers must update struct literals.

## [0.1.0]

### Added
- Initial vKore desktop, machine/session management, CLI, add-ons and libkore-backed volume access.
