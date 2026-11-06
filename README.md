# vKore

vKore is the open-source host side of [Kore](https://github.com/MicroPerceptron/kore),
Micro Perceptron's capability-backed kernel for on-device AI. It boots a Kore
ISO under QEMU on the same machine topology Kore's own production gate boots,
follows the boot by what Kore itself reports, and reads and writes Kore's
DagFS volumes from the host through Kore's own filesystem engine, the one the
kernel mounts, reached through [libkore](https://github.com/MicroPerceptron/libkore),
Kore's public API. There is no second implementation of anything Kore owns.

It comes as a desktop app (`vkore-ui`) and a command line (`vkore`).

**Status: alpha.** Verified against production x86 ISOs of Kore v0.40.0 and of
Kore's main at `7074bda2`, under QEMU 8.2 with TCG on Linux; see
[what is and is not verified](#what-is-verified).

## What it does today

- **Machines.** `vkore machine create` records a machine for an ISO;
  `vkore machine run` probes the host's QEMU, firmware and acceleration, checks
  the ISO (SHA-256, the paging mode its bootloader asks for), boots it, and
  reports the boot's stages from Kore's own console markers: kernel, shell,
  ready, degraded, failed. It stops the machine with the power button, and ends
  QEMU after a grace period if the guest does not power off.
- **Volumes.** A machine's disk, any raw Kore disk image, or a disk's block
  device (Linux) can be listed, previewed, copied out, and written into, from
  the host, while no machine runs on it. Writes go through Kore's
  all-or-nothing publication: a volume is either as it was or carries the whole
  change, never part of one.
- **Screenshots.** Kore's PrintScreen saves a PNG stamped with the build, the
  time and the screen it captured into `pictures/screenshots` in terminal 0's
  home. vKore makes sure that folder exists before each boot, and lists, previews
  and exports the captures with their record.
- **Projects.** A host directory can be staged into a terminal's home at every
  boot; after the machine stops, `vkore machine changes` shows what the guest
  changed and `vkore machine apply` writes it back, within explicit file and
  byte bounds.

## Quick start (Linux)

You need a Kore x86_64 ISO, `qemu-system-x86_64` (8.2 or newer) with OVMF
(`ovmf` on Debian and Ubuntu), and the Rust toolchain pinned in
[rust-toolchain.toml](rust-toolchain.toml); `rustup` installs it on first build.

```sh
cargo build --release -p vkored -p vkore-ui
```

Create a machine and boot it until Kore reports ready, then save a screenshot
and stop:

```sh
target/release/vkore machine create demo --iso /path/to/kore-x86-release.iso
```

```sh
target/release/vkore machine run demo --display headless --screenshot --stop-after 15
```

Kore writes a capture a few seconds after the press (under TCG, up to ten), and
a stop does not wait for one still being written: hence the 15 s.

Copy the captures out, with their record (the folder is created if it does not
exist):

```sh
target/release/vkore volume @demo shots --get ~/Pictures/kore
```

Or open the desktop on that machine's disk:

```sh
target/release/vkore-ui --open @demo
```

`--display window` (the default) gives the guest QEMU's own window, so you can
use Kore and press PrintScreen yourself. A stop presses the machine's ACPI
power button, and Kore powers itself off the way `pwctl shutdown` does: its
services asked to quiesce, its runtime torn down, then ACPI S5, in well under a
second. Kore up to v0.40.0 reads and writes the power-management registers a
byte at a time, which QEMU's chipset does not decode, so it never hears the
button; there a stop ends QEMU after `--grace` seconds (10 by default) and the
volume keeps its last committed state. Kore's main fixes that at `7074bda2`.
Machines, their disks and run transcripts live under `$XDG_STATE_HOME/vkore`
(or `--state`).

## The desktop

`vkore-ui` has two pages.

- **Volumes** opens a machine's disk (`@name`), an image, or a block device and
  lands in terminal 0's screenshots. Terminal homes and the volume root are one
  click away; a screenshot shows its size and Kore's capture record (build,
  time, mode, screen, region, uptime); text shows its first 32 KiB. **Open**
  hands a file to the host's own viewer, **Save** copies it out (never over an
  existing file), **Import** installs a host file or folder into the open
  directory, and **Remove** asks twice.
- **Machines** creates machines from an ISO, starts them in a window or
  headless, follows their boot, and stops them. Starting a machine whose disk
  the Volumes page holds closes the browser first and reopens it when the
  machine stops, so what you captured is what you see next.

The desktop draws with [Iced](https://iced.rs) over Axiom, Micro Perceptron's
software renderer; there is no browser, daemon or login. Every string that
comes from outside the binary (a file name a guest wrote, a log, a host path)
is set so that it cannot break the frame or pass for something it is not:
characters the bundled faces cannot draw, controls, direction overrides and
invisible characters show as `\u{…}` escapes. A file a guest named `gnp.exe`
behind a right-to-left override shows as `\u{202e}gnp.exe`, not `exe.png`.

See [docs/desktop.md](docs/desktop.md).

## How it fits together

```text
   vkore-ui (desktop)        vkore (CLI)
            \                  /
             vkored: machines, their state, boot sessions, transcripts
            /                  \
   vkore-vm                     vkore-volume
   QEMU: probe, gate topology,  DagFS from the host: browse, preview,
   firmware, ISO, QMP,          export, install, project staging
   readiness from Kore's            |
   own console markers          kore-host (libkore)
            |                       |
     qemu-system-x86_64         kore-vol + kore-svc-dagfs: Kore's own
            |                   volume procedure and filesystem engine
       Kore guest  ─── disk ───┘
```

What vKore relies on from Kore, and the exact revisions it was verified
against, are in [docs/compatibility.md](docs/compatibility.md).

## Integrity and security

- **Kore's engine, or nothing.** vKore never parses or writes DagFS bytes
  itself. Locating, recovering, publishing and verifying a volume is Kore's
  procedure, exported unmodified through libkore (`native/` there is a
  byte-exact export with provenance).
- **One owner per disk.** A volume is offline: vKore refuses a disk a running
  machine holds (Kore's image lock), and within one process a disk has one
  opener at a time.
- **Exports stay where they are sent.** Copies out are created inside the
  destination with no symlink followed, written to a partial file, synced, and
  only then published under their name, never over an existing file unless
  asked.
- **Bounded by construction.** Previews read at most a file's head (a PNG's
  record precedes its pixels, so none are decoded), the PNG decoder's
  allocations are capped, project staging and write-back carry file and byte
  limits, and QMP lines are bounded.
- **Probe, don't guess.** QEMU's devices and properties, firmware descriptors,
  acceleration and the ISO's paging mode are read from the host and the medium.
  What Kore or QEMU cannot take is refused before a machine starts, in words.

## What is verified

On Linux x86_64 in a container (QEMU 8.2.2, TCG, no KVM), with the engine
libkore exports from Kore's main at `7074bda2`:

- A production ISO built from Kore's main at `7074bda2`: `vkore machine run`
  boots it to ready (19/19 services, `first-shell: ready`, 45/45 apps seeded,
  homes present), presses PrintScreen, and stops it gracefully: Kore hears the
  power button and powers off through its own shutdown path, 0.35 s after the
  press. `vkore volume @NAME shots --get` copies out the capture, stamped with
  the build that took it and byte-identical to the volume's copy.
- A production Kore v0.40.0 ISO: the same run to ready (44/44 apps), and its
  capture copied out; a stop ends QEMU after the grace period.
- A disk v0.40.0 formatted and wrote, booted by the `7074bda2` ISO: ready, with
  the captures of both builds read back side by side.
- A GPT disk on a loop device with Kore's user and system partitions: an
  install through the partition node, exports byte-identical through both the
  partition and the whole-disk node, and nothing outside the user partition
  changed.
- The desktop under Xvfb: browsing a machine's disk, previewing a capture and
  its record, saving it out byte-identical, and starting, following and
  stopping the machine.
- `cargo test --workspace`, including Kore's volume engine end to end on real
  images, and the opt-in host test that boots each ISO through `vkore-vm`.

Not verified yet: KVM or HVF acceleration, physical hardware (a physical disk
included), macOS (`vkore-vm` supports HVF; nothing here has run it), and AMD
hosts.

## Workspace

| Package | What it owns |
| --- | --- |
| `vkored` | Machines and their state, boot sessions and transcripts, the `vkore` CLI |
| `vkore-vm` | QEMU: host probing, Kore's gate topology, firmware, ISO inspection, QMP, readiness |
| `vkore-volume` | DagFS volumes from the host over `kore-host`: browse, preview, export, install, project staging |
| `vkore-ui` | The desktop: Volumes and Machines, Iced over Axiom |
| `vkore-policy` | Authenticated approvals, standing policy, revocation, tamper-evident audit |
| `vkore-packages` | Signed, content-addressed artifacts, compatibility checks, rollback |
| `vkore-addon-sdk` | Capability-scoped host, guest and split tool contracts |
| `vkore-git-addon`, `vkore-gmail-addon`, `vkore-reference-addon` | Add-ons over the SDK |
| `iced-renderer-compat` | Keeps the desktop off renderer backends it does not use |

The policy, package and add-on services are retained from vKore's first
architecture. Their guest path was HostLink, a host-guest channel that never
landed on Kore's main line and has been retired here; they return to the
guest when a channel does. See [MIGRATION.md](MIGRATION.md).

## Build and test

```sh
cargo fmt --all -- --check
```

```sh
cargo clippy --workspace --all-targets -- -D warnings
```

```sh
cargo test --workspace
```

The boot test is opt-in and uses your own QEMU and ISO:

```sh
VKORE_TEST_QEMU=$(command -v qemu-system-x86_64) VKORE_TEST_ISO=/path/to/kore-x86-release.iso \
  cargo test -p vkore-vm --test host -- --nocapture
```

The desktop depends on Axiom, which is not public yet; Cargo fetches it with
your Git credentials (see [.cargo/config.toml](.cargo/config.toml)). Every
other crate builds from public sources.

## How we work

vKore follows the doctrine Kore and libkore follow: precedent is evidence, not
authority; take proven work as a dependency rather than re-earning it; one
narrow goal at a time, accepted only on a real Kore boot; and quality (correct,
secure, efficient, fast) is the product, not a pass after it. The working
rules are in [AGENTS.md](AGENTS.md).

## Documentation

- [The desktop](docs/desktop.md)
- [Compatibility: Kore, libkore, QEMU and the guest contract](docs/compatibility.md)
- [Migration: from HostLink to libkore](MIGRATION.md)
- [Approval and standing-policy model](docs/policy.md)
- [Add-on SDK and execution domains](docs/addons.md)
- [Package installation, updates and rollback](docs/packages.md)

vKore is licensed under the [Mozilla Public License 2.0](LICENSE). See
[NOTICE.md](NOTICE.md) for attribution.
