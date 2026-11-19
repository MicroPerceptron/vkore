# Changelog

Release versions follow [Semantic Versioning](https://semver.org/).
See [the versioning policy](docs/versioning.md) for contribution and release checks.

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
