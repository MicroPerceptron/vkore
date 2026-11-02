# Compatibility

vKore has one compatibility identity: the libkore revision it pins. libkore's
`native/` tree is a byte-exact export of a Kore commit (recorded in its
`native/provenance.json`), so that pin also names the Kore source vKore's
volume layer runs.

## Current identity

| | Revision | What it is |
| --- | --- | --- |
| libkore | `83d8347082302fbc604381b15ce1125509ff4c2b` | `kore-host`, the host facade, over the exported `kore-vol` closure (26 packages) |
| Kore source | `ab6e7b157659cc4158ca30bb70a14bb64b0c6820` | Kore main `e5e0b96a` (v0.40.0) plus four `tools/kore-vol` commits: the procedure as a library, and a mount that releases every handle it opens |
| DagFS on disk | format version 5 | `kore_host::volume::FORMAT_VERSION`; a volume another version wrote is refused, never rewritten |
| Kore ISO | v0.40.0, SHA-256 `029ddd7e1e1cc1819c7e3ecf7ec22ffc3f0f6e01105972cd4d968313465aab23` | `make iso-sealed-boot-x86 BOOT_FEATURES=boot PAGING_PROFILE=4k48 MODE=release` at Kore `1b6cb322`; `ab6e7b15` changes only a host tool the ISO does not carry |
| QEMU | 8.2.2, TCG | the oldest QEMU vKore has booted Kore on; newer ones add capabilities vKore probes for |

The Kore `v0.40.0` tag points at an older tree (Cargo version 0.30.0); the
Cargo bump to 0.40.0 is `61dc1b84` on main. vKore names Kore by commit.

## What vKore relies on from Kore

**The medium.** A UEFI ISO booted by Limine. vKore reads `limine.conf` through
the ISO's Joliet tree (a bounded reader of its own; ISO crates were vetted and
rejected for this) to learn the paging mode the kernel is built for, and
refuses a five-level ISO when this QEMU cannot realize LA57 for the guest.

**The machine.** Kore's production gate boots one topology, and vKore boots
the same one: q35 with HMAT; an IOMMU with interrupt remapping (Intel VT-d, or
AMD-Vi with `--vendor amd`); two NUMA nodes, the second behind a PCIe expander
bridge; the volume on NVMe; two xHCI controllers with a USB keyboard on port 1
and a mouse on port 3; USB storage, e1000e and USB networking; a 32-bit
`bochs-display` framebuffer; OVMF from pflash; `-no-reboot -nodefaults`.
Properties that differ between QEMU releases (`x-scalable-mode` against
`scalable-mode`, `acpi-generic-initiator` from QEMU 9) are probed from the
binary, not assumed.

**The console.** COM1 to a file and `debugcon`. The boot's stages come from
Kore's own lines:

| Stage | Kore says |
| --- | --- |
| kernel | `kore <version>: console up` |
| services | `first-userland-boot: services founded (N/N)` |
| shell | `first-shell: ready` |
| ready | the shell, `app-store: seeded N/M`, and `home-skeleton: homes: P/A` |
| degraded | `kore: BOOT DEGRADED` |
| failed | `error[kernel-panic]`, `error[kernel-fault]`, `first-shell: FAIL`, `bootstrap-testimony: FAIL`, or a `first-userland-boot:` founding or readiness `FAIL` |

Any other `: FAIL` row is a warning: a witness failed, the boot did not.

**Input and power.** QMP: `send-key` (PrintScreen is `print`), `screendump`,
`system_powerdown`, `quit`. Kore v0.40.0 does not act on the ACPI power button
under q35, so a stop ends QEMU after its grace period. Every QMP command carries
an id, so a reply that comes late (a busy TCG guest can hold QEMU's main loop for
seconds) is dropped instead of being taken for the next one.

**The volume.** A blank disk is formatted by the machine at its first boot, with
terminal homes at `work/face-N`. Kore's capture app saves into
`pictures/screenshots` only if the folder exists, so vKore creates it in
terminal 0's home before each boot (`screenshots_folder` in the machine's
configuration turns that off). A capture's record is in `tEXt` chunks ahead of
its image data: `Software`, `Creation Time`, and `kore:kernel-build`,
`kore:uptime-ms`, `kore:screen`, `kore:mode`, `kore:app`, `kore:rect`,
`kore:density-percent`, plus any other `kore:` keys Kore adds (v0.40.0 also
writes `kore:cursor` and `kore:freeze-us`).

**The engine's bounds.** One open volume per disk per process (Kore's image lock
is a per-process POSIX record lock); 512 objects, 128 entries a directory, 32
levels, names of at most 32 bytes; 32 handles per engine session.

## Updating the pin

1. Land the change in Kore (behaviour belongs to its owner), export it with
   libkore's `tools/sync-native.py`, and run libkore's gates.
2. Set the new libkore revision in [Cargo.toml](../Cargo.toml) and run
   `cargo update -p kore-host`.
3. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
   -D warnings` and `cargo test --workspace`.
4. Build an ISO from the matching Kore revision and run the boot test
   (`VKORE_TEST_QEMU`, `VKORE_TEST_ISO`; see the README), then a screenshot round
   trip: `vkore machine run NAME --screenshot --stop-after 5` and
   `vkore volume @NAME shots --get DIR`.
5. Record the new identities here. A table row no run has verified is not
   written.
