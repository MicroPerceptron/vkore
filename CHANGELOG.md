# Changelog

Release versions follow [Semantic Versioning](https://semver.org/).
See [the versioning policy](docs/versioning.md) for contribution and release checks.

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
