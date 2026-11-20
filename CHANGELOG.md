# Changelog

Release versions follow [Semantic Versioning](https://semver.org/).
See [the versioning policy](docs/versioning.md) for contribution and release checks.

## [0.6.0] - 2026-11-19

### Added
- Discover Linux GPUs and assign Intel xe SR-IOV virtual functions from hardware setup or the CLI. Done/Save can provision an absent VF pool through system authentication without detaching the host GPU or resizing existing pools.
- Choose a single guest NUMA node for inference or the two-node gate layout. New hardware recommendations use one node; existing configurations keep their layout.

### Fixed
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
