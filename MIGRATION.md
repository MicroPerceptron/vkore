# Migration

## From HostLink to libkore

vKore began as Kore's host tooling, extracted from Kore PR #722
(`da97303982d1e75200b0e521539c43921b842bc0`): a harness daemon, a desktop
client of it, and HostLink, a host-guest channel over virtio-serial and
ivshmem that carried prompts, inference and workspace traffic into a Kore
guest. It pinned Kore crates by commit for the shared protocol.

HostLink never reached Kore's main line. Kore #722, #909 and #933 closed
unmerged and #1015 is open without activity; Kore main today has no virtio
guest drivers and no host-guest channel at all. A release of Kore (v0.40.0)
therefore cannot speak to the HostLink half of vKore, and the pinned crates
had drifted to a Kore that no longer exists.

So the HostLink half is retired, and vKore is rebuilt on what Kore v0.40.0
actually offers a host:

| Kore offers | vKore uses it for |
| --- | --- |
| A bootable ISO and its production gate's QEMU topology | `vkore-vm`: boot it on that topology, probed per host |
| Console lines with stable markers | readiness: kernel, services, shell, ready, degraded, failed |
| QMP (HID input, screendump, power) | PrintScreen, screendumps, stop |
| DagFS volumes, with Kore's engine exported through libkore | `vkore-volume`: browse, preview, export, install, stage a project |

Kore code reaches vKore only through libkore (`kore-host`), whose `native/`
tree is a byte-exact export of a Kore commit. vKore pins one libkore revision
and nothing from Kore directly; see [docs/compatibility.md](docs/compatibility.md).

## What changed

| Before | Now |
| --- | --- |
| `vkored`: HostLink endpoint, inference registry, harness daemon, `vkore chat` | `vkored`: machines, their state, boot sessions and transcripts, the `vkore` CLI |
| `vkore-vm`: QEMU with HostLink devices, pause and resume | `vkore-vm`: Kore's gate topology, host probing, firmware, ISO inspection, QMP, readiness |
| `vkore-workspace`: snapshots imported over HostLink, bounded write-back | `vkore-volume`: the same bounds, staged into a terminal's home through Kore's engine before a boot and collected after it |
| `vkore-hostlink-ab`: serial against ivshmem measurement | removed |
| `vkore-ui`: projects, chat, approvals, agent topology over the harness API | `vkore-ui`: Volumes and Machines, in-process, no daemon |
| `scripts/`, `vkore-axiom` launchers, `docs/harness-api.md` | removed |
| Kore crates pinned by commit (`kore-hostlink`, `workspace-sync`, block and filesystem crates) | `kore-host` from libkore, one pin |

The policy, package and add-on crates (`vkore-policy`, `vkore-packages`,
`vkore-addon-sdk` and the add-ons) are unchanged. They are host services whose
guest path was HostLink; until a channel exists they are exercised by their own
tests only.

## Taking a channel back up

When Kore's main line lands a host-guest channel (HostLink as in #1015, or its
successor), it comes to vKore the way the volume engine did: Kore owns the
protocol and both ends' shared types, libkore exports them, and vKore depends
on the export. vKore does not carry a copy of a Kore protocol, and does not
reintroduce one ahead of Kore. The open HostLink pull requests in libkore (#22)
and vKore (#39) predate this and should be rebased onto that path or closed.

## Earlier corrections kept

Linux testing of the original workspace code found that `cap-std` keeps a
directory as an `O_PATH` descriptor, which `flistxattr` cannot take; it was
reopened relative to that descriptor. `vkore-volume`'s export uses `cap-std`
and `cap-fs-ext` (`open_dir_nofollow`) for the same root confinement.

## Downloaded images and a local API

The next frontend pass adds managed HTTPS ISO acquisition and a loopback HTTP
API in `vkored`, shared by the desktop and CLI. Neither requires a new guest
contract or a libkore pin change. The API uses host-side OpenAI request/response
types and a `Backend` interface, not a proposed guest protocol. The production
adapter remains disconnected: no models are advertised and inference returns
503. Reconnecting it must still follow the Kore-owned, libkore-exported channel
path above. The old agent UI and HostLink implementation remain retired.

The desktop image selector uses the embedded [official catalog v1](docs/image-catalog.md),
with required size, digest and compatibility metadata. The runtime resolves
image IDs itself. No official artifacts are populated yet; remote catalog
updates and their signature/trust protocol are outside v1.
