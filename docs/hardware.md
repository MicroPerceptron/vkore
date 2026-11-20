# Machine hardware

The Machines page has a **Configure hardware** panel for new machines and
stopped machines. Changes to an existing machine take effect after **Save
hardware**. CPU, RAM and disk controls use the host snapshot; refresh that
snapshot after changing host resources. Launch always probes again.

The processor panel shows a core map grouped by socket and reported core type.
Each tile is one physical core; its CPU buttons are the available hardware
threads, including SMT siblings. Selecting threads updates the guest CPU count,
with the image minimum enforced. The count slider and stepper preserve current
choices and add threads spread over cores when needed. Placement shortcuts can
spread the selection or restrict it to reported performance/efficiency cores;
a smaller pool reduces the guest count, never below the image minimum.

## Admission and defaults

`vkored::hardware` owns policy. Every image requires at least 2 guest CPUs,
2048 MiB RAM and 1024 MiB storage. Official catalog entries can raise those
minimums with `minimum: { cpus, memory_mib, disk_mib }`; import persists the
requirements alongside the image identity. CPU count is bounded by 128 and
available host logical CPUs. RAM is bounded by physical host memory, available
memory at launch when pinned, and the guest's 1 TiB limit. Unpinned
allocations above currently available RAM show a memory-pressure warning. The two guest NUMA nodes require
an even split; hugepages additionally require both halves to be page-aligned.

Recommended allocations use half the available physical cores (not SMT thread
count) and half physical RAM, rounded up only as needed to satisfy image
minimums and page alignment. Pinning and disk reservation default on where
supported. 2 MiB hugepages are selected only when the current user can create a
hugetlb memfd and the reserved pool has sufficient uncommitted pages; otherwise
the form explicitly selects system pages. Refreshing never silently replaces
an existing selection. A changed or depleted resource causes a preflight error.

Existing schema-2 configurations without resource fields retain their prior
unpinned/system-page/sparse behavior; new CLI and desktop creation use host
recommendations. All configurations are subject to the baseline minimums.
`vkore hardware` reports the snapshot as JSON. CLI creation also offers
`--cpus`, `--memory-mib`, `--host-cpus`, `--no-cpu-pinning`,
`--no-memory-pinning`, `--pages system|2m|1g`, `--sparse` and `--passthrough PCI_ADDRESS`.

## Host discovery and CPU placement

Discovery uses sysinfo for CPU identity, memory and mounted disks. Linux adds
package/core/SMT and NUMA topology from sysfs, restricted by the process's allowed
CPU set. Intel hybrid P/E labels come from the kernel's `cpu_core/cpus` and
`cpu_atom/cpus` PMU lists, never guesses based on clock speeds. Unknown core
kinds stay generic. Apple performance-level counts are read through sysctl;
macOS does not expose an equivalent exact CPU affinity mechanism, so exact
selection and CPU pinning are disabled with an explanation.

One chosen host logical CPU corresponds to each guest vCPU in selection order.
Recommendations spread across physical cores before using SMT siblings. The
Linux launcher queries typed QMP CPU records while QEMU is paused, checks every
TID belongs to its own child, applies affinity, reads it back, then resumes.
Any failure kills and reaps the child. Pinning constrains guest vCPU threads; it
does not reserve host cores exclusively or bind QEMU's I/O threads. The guest's
synthetic two-node topology remains separate from host NUMA topology.

## Memory and authentication

On Linux, memory pinning selects QEMU `-overcommit mem-lock=on`; hugepages use
`memory-backend-memfd,hugetlb=on,hugetlbsize=...,prealloc=on` on each guest NUMA
node. This is physical residency, not host NUMA placement. macOS QEMU memory
locking and hugetlb selection are unavailable. The memory panel instead shows
**Automatic** with the probed native page size (16 KiB on the tested Apple
Silicon host). The Linux-only hugepage buttons are omitted and the limitation
is explained. This is host RAM backing, not the guest's page-table granularity.
The native size comes from rustix's safe platform query, not an architecture
constant. On Linux, unavailable choices explain whether there is no accessible
pool or not enough pool capacity.

A saved unsupported page choice remains visible as a warning until the user
selects Automatic. Unsupported saved memory pinning can still be switched off;
it cannot be re-enabled. Launch validates both choices and never silently
substitutes them. The `vkore hardware` snapshot includes the native page size
and whether the launcher's explicit hugetlb backing is supported.

The packaged macOS QEMU 11.0.1 advertises RAM/file/shm memory backends, but no
`memory-backend-memfd`; its RAM backend has no hugepage-size property. It also
rejects `mem-lock=on` with an unimplemented locking error. No authentication
prompt can add a missing backend. See [QEMU's memfd backend](https://github.com/qemu/qemu/blob/v11.0.1/backends/hostmem-memfd.c)
and [native page sizing](https://github.com/qemu/qemu/blob/v11.0.1/util/mmap-alloc.c).

The desktop checks the complete hardware selection when **Done** (new machine)
or **Save hardware** (existing machine) is clicked. It validates CPU, memory,
and storage before requesting permissions together. The editor stays open with
the draft intact if validation or authentication fails; controls are frozen while
a request is pending. Saved machines hold their session lease through this step,
so they cannot be launched or edited concurrently.

For configurations without NPU passthrough, memory locking is the only setting
requiring elevation. The shared
completion/launch authorization path raises the current vKore process's
memory-lock limit to guest RAM plus a bounded 1 GiB emulator allowance. No popup
appears when pinning is off or the process already has enough permission. It
first uses the existing hard limit without prompting; if
necessary it re-executes vKore through `/usr/bin/pkexec --disable-internal-agent`
in its private `--vkore-memory-helper` mode with a bounded memory size. The helper
checks the polkit identity against its parent and changes only that parent's
memlock limit; it does not accept a target PID or a command to execute. The registered desktop polkit agent
presents authentication. No password enters vKore, no shell is invoked, no
privileged QEMU is launched, and no persistent system configuration is changed.
QEMU inherits the limit. Limits last only for this app process; launch rechecks
and may request authorization after restarting vKore. Cancellation, denial, missing agent and a two-minute
timeout return visible errors. QEMU's actual locking failure remains fatal.
The CLI reports a needed authorization rather than prompting on a hidden worker.

Host hugepage pools are not globally reconfigured. Unusable page sizes remain
disabled, with system pages available. This avoids silently consuming or
reassigning a pool another workload relies on.

## Linux authentication descriptions

Polkit's default `pkexec` prompt includes the executable and arguments. To show
short application descriptions instead, register the exact binaries you run:

```sh
python3 tools/install-linux-policy.py --executable target/release/vkore-ui
```

Repeat `--executable target/release/vkore` in the same command if you also use
the CLI. The installer asks for administrator authentication once using the
standard system prompt; the installed policies then say **Authentication is
required to prepare hardware for a vKore machine** or **Authentication is
required to lock memory for a vKore machine**. The authentication agent may
still show the executable under its expandable technical details.

The policy is root-owned in `/usr/share/polkit-1/actions/`, matches each exact
canonical executable path **and** private helper argument, and uses
`auth_admin` without retained authorization. Inactive/remote sessions are not
granted access. It does not add passwordless rules or change prompts for other
applications. Reinstall with every desired binary after moving a checkout or
switching between debug and release paths. Rebuilding at the same path needs
no reinstall. Without installation, the original generic system prompt remains.

Packagers can use `--output org.microperceptron.vkore.policy` to generate the
file without elevation and install it root-owned with mode 0644 alongside the
application. Only use per-request authorization for policies pointing into a
user-writable source checkout; never change these actions to `auth_admin_keep`
or `yes`. See [polkit's executable/argument annotations](https://www.freedesktop.org/software/polkit/docs/latest/pkexec.1.html).

## Storage and ownership

The panel displays mounted storage devices, filesystem, SSD/HDD classification,
capacity and free space. Machine disks remain in the vKore state filesystem;
the displayed reservation capacity is queried for that path, including APFS
mount aliases. Selecting an arbitrary physical disk is outside this UI.

New desktop drafts target **32 GiB**, reduced to whole GiB of currently free
space when necessary, while respecting the image minimum. The capacity field
accepts GiB (fractional values round up to a whole MiB); presets cover 16, 32,
64, 128, 256, 512 and 1024 GiB. The slider uses a logarithmic scale so smaller
allocations remain practical on large hosts. Backend admission caps disks at
4 TiB. Reserved disks must fit current free space. With reservation off, sparse
disks can exceed currently free space, with a warning that future guest writes
will need that space. Switching reservation on never silently shrinks a draft.

At first launch, reservation allocates the entire disk in a temporary file,
syncs it, and publishes without replacing another file. rustix uses Linux
`fallocate`; on macOS it tries `F_ALLOCATECONTIG` before a full noncontiguous
allocation. On filesystems such as APFS that release unwritten reservations
on close, blank blocks are materialized in bounded 1 MiB chunks before
publication. No sparse fallback occurs on allocation failure. A filesystem may
fragment an allocation, and COW filesystems or SSD translation cannot promise
physical contiguity. Reserved disks disable guest discard/unmap and automatic
zero detection so TRIM cannot release the reservation. Sparse allocation remains
an explicit opt-out.

After creation, size and reservation mode are fixed. A file truncate is not a
safe DagFS resize protocol. This implementation changes no DagFS bytes and
requires no Kore/libkore changes. A session lock serializes launches and hardware
edits across vKore processes for the whole boot/staging/writeback lifecycle;
volume ownership and engine locking remain intact.

## Dependencies and evidence

- `sysinfo = 0.40.4`: MIT, maintained upstream, Rust 1.95 minimum; only `system`
  and `disk` features. It wraps platform calls with upstream unsafe FFI; no
  process enumeration or periodic sampling is used. Platform Objective-C bindings
  are reused on macOS. This release includes the upstream macOS memory-accounting correction;
  the earlier formula could subtract compressed storage and saturate at zero.
- Existing `rustix = 1.1.4`: safe affinity, resource-limit and allocation APIs.
  The CPU/memory/storage APIs need no application unsafe code. fs4 was evaluated but its allocation path
  delegates to rustix, so another wrapper dependency adds no value here.
- hwlocality was evaluated for topology/binding; its alpha API and additional
  libhwloc C runtime were unnecessary for this bounded CPU/memory/storage scope.

References: [sysinfo](https://docs.rs/sysinfo/0.40.4/sysinfo/),
[Linux hybrid topology](https://github.com/torvalds/linux/blob/master/tools/perf/Documentation/intel-hybrid.txt),
[QEMU memory options](https://www.qemu.org/docs/master/system/qemu-manpage.html),
[polkit authentication](https://www.freedesktop.org/software/polkit/docs/latest/pkexec.1.html).

The public CI hardware job checks the unchanged `vkore-vm`, `vkore-volume` and
`vkored` sources in an isolated workspace on Linux and macOS. Cargo otherwise
resolves the desktop's private Axiom pin even for `-p vkore-vm`, and the public
runner has no credentials for it. Full workspace Clippy/tests and desktop
rendering are separate local gates; backend CI does not claim desktop coverage.

The memory panel distinguishes immediately free RAM from the OS estimate of
free-or-reclaimable RAM. On macOS the latter follows XNU's non-compressed
available-memory accounting (active + inactive + free, with speculative pages
already in free). It does not mean those pages are unused or guarantee an
allocation without memory pressure. See
[Apple's accounting](https://github.com/apple-oss-distributions/xnu/blob/main/doc/vm/memorystatus_notify.md#available-memory).

## Neural processor discovery

Hardware setup includes a **Neural processors** section. It reports host devices
and three separate routes: dedicated assignment, SR-IOV virtual functions, and
shared execution through virtio-accel. Linux Intel/AMD physical NPUs now have a
**Full passthrough** switch. SR-IOV provisioning and shared guest connections
remain unavailable. Discovery itself stays read-only; editing the switch never
changes a host driver.

`vkore-devices` is an independently portable inventory crate; `vkore-vm` includes
its snapshot in `HostResources.npus`, and `vkored::hardware::npu_routes` owns the
route assessment shown by the UI. The existing **Refresh host** action replaces
the inventory. `vkore hardware` reports it as JSON; `cargo run -p vkore-devices
--example inventory` reports only devices and also runs on Windows. This does
not make the current Unix-dependent desktop or VM launcher Windows-compatible.

- **Linux:** libudev supplies canonical PCI device paths. Known Intel ivpu and
  AMD XDNA vendor/device IDs identify unbound and VFIO-bound NPUs too; known
  native drivers recognize newer IDs. PCI class alone never identifies an NPU.
  Probes read revision, driver, IOMMU group members, physical-function links,
  `sriov_totalvfs` and `sriov_numvfs`. This first Linux inventory does not cover
  non-PCI/platform NPUs or unknown vendors. ID references are the upstream
  [Intel driver](https://github.com/torvalds/linux/blob/master/drivers/accel/ivpu/ivpu_drv.h)
  and [AMD driver](https://github.com/torvalds/linux/blob/master/drivers/accel/amdxdna/amdxdna_pci_drv.c),
  not a copy of Kore's driver tables. Detection says nothing about guest support.
- **Windows:** Microsoft's DXCore NPU hardware-type attribute enumerates
  driver-exposed NPUs, including devices without a Direct3D user-mode driver.
  Generic ML capability alone is insufficient (GPUs can expose it). The system
  DXCore DLL is loaded on demand; older Windows returns an unavailable report.
  A missing driver can hide an NPU. No PCI topology or SR-IOV capability is
  inferred from a device name or vendor ID. LUIDs identify this snapshot only.
- **macOS:** Core ML's `MLModel.availableComputeDevices` reports Neural Engine
  devices available to Core ML. Availability is checked before sending the
  macOS 14+ selector; core count is queried only when that property exists.
  This matches the Core ML backend's discovery boundary, rather than assuming
  that every Apple processor has an accessible ANE. Core ML may schedule some
  operations on the CPU. The ordinal is not a persistent hardware identifier.

Successful empty discovery, partial discovery and unavailable discovery are
separate states. Ancillary Linux metadata failures retain identified devices
and report warnings. Snapshots retain at most 64 NPUs and 16 warnings; strings
are capped at 256 characters, individual sysfs reads at 4096 bytes, and PCI
scanning at 4096 devices. Host strings use the desktop's escaped data font.

An IOMMU group is evidence of a candidate route, not proof of isolation/reset
safety or QEMU/image compatibility. SR-IOV is reported only from actual sysfs
capabilities or an existing VF relationship. Runtime installations and model
capabilities are not probed: OpenVINO, XDNA and Hexagon are candidate backends,
not claims of working virtio-accel access. VFIO-bound or driverless Linux NPUs
explicitly need host setup before a native runtime can use them.

### Dependency decision

- `udev` 0.9.3 (MIT) supplies owned libudev enumeration handles on Linux. Its
  native dependency is libudev; it adds no background monitor here.
- `windows` 0.62.2 (MIT/Apache-2.0) supplies DXCore types and COM ownership.
  `libloading` 0.8.9 keeps DXCore optional at process startup. The newer NPU
  attribute GUID is taken from Microsoft's published DirectX-Headers because
  this binding's SDK metadata does not yet contain it.
- `objc2-core-ml` 0.3.2 (Zlib/Apache-2.0/MIT) supplies the Core ML device APIs.
  Only device-related features are enabled; no Metal backend, model compilation
  or vendor runtime is bundled. Retained objects and the autorelease pool stay
  inside each probe. Platform FFI calls have local safety explanations.
- `pci-info` 0.4.0 was inspected but not adopted: its documented location/domain
  limitations make it unsuitable as an assignment identity source, its Linux
  backend adds no value over authoritative udev paths, and it misses integrated
  ANE devices. Maintaining a second inventory would also require ambiguous
  PCI-to-DXCore correlation. `hwlocality`/libvirt add broader native management
  dependencies without replacing the NPU-specific enumeration needed here.

CI builds/tests this crate independently on Linux, Windows and macOS and runs
its read-only inventory example. A CI runner with no NPU validates empty-device
handling, not physical NPU access. Native-device, guest-inference and performance
validation remain separate evidence.

## Linux whole-NPU passthrough

Enable **Full passthrough** on a device card, then finish with **Done** or
**Save hardware**. From the command line, `vkore machine create
--passthrough 0000:00:0b.0` records the same selection (repeat it for more
devices; `vkore hardware` lists the addresses), and `vkore machine run` asks
for the same authorization at launch. Turning it off removes the device from subsequent launches.
The configuration records PCI address, vendor ID and device ID; launch refuses
missing or changed identities. Missing saved selections remain visible and can
be disabled, including on another operating system. Existing configurations
have no assignments by default. At most four NPUs may be selected.

This first implementation admits Intel/AMD physical NPUs in a single-device
IOMMU group, with no enabled VFs, and a supported native driver (`ivpu`,
`intel_vpu`, `amdxdna`), no driver, or `vfio-pci`. Shared groups and virtual
functions are refused. The host must report PCI function-level reset (`flr` or
`af_flr`); vKore temporarily restricts reset to that method, never bus reset.
Close host NPU workloads before setup/launch. The privileged check scans open
native accelerator and VFIO device handles and refuses a busy device, but this
snapshot cannot prevent another host application from opening it afterwards.

Launch requires Linux KVM, a kernel with IOMMUFD and VFIO cdev support, and QEMU
with the `vfio-pci` `fd`/`iommufd` properties and the `iommufd` object's `fd`
property. These are probed instead of assumed from a version number. This
avoids the legacy VFIO container's incompatibility with Intel first-stage
virtual-IOMMU translation. The packaged Linux QEMU smoke gate checks these
properties. No fallback disables the guest IOMMU, isolation or interrupt safety.
Guest driver and nested-IOMMU compatibility still need validation on each
NPU/image combination. See [QEMU's IOMMUFD FD passing](https://www.qemu.org/docs/master/devel/vfio-iommufd.html)
and [Linux VFIO](https://www.kernel.org/doc/html/latest/driver-api/vfio.html).

For Intel passthrough, the guest uses second-level DMA translation with
scalable mode, caching and interrupt remapping enabled. Snoop control is off:
QEMU's VFIO notifier rejects it. PASID/first-level translation is not advertised
by this assignment profile. Non-passthrough profiles retain their existing
settings.

The authorized guardian realizes the selected devices in a small **paused QEMU
probe** before the actual guest starts. It tries the supported 48-bit and 39-bit
second-level address widths in descending order, selecting the widest one whose
QMP status reaches `prelaunch`. QEMU checks the effective kernel IOVA ranges
for every attached device; no CPU-width or PCI-model heuristic substitutes for
that check. A 41-bit host therefore selects 39 bits if that candidate realizes.
Configurations whose RAM plus the below-4-GiB address-hole allowance cannot fit
are rejected. If neither candidate works, launch fails with diagnostics.

Each probe uses 128 MiB of RAM, read-only firmware code, and no guest disk,
writable firmware variables, networking or running guest CPUs. Probes and the
actual launch run unprivileged with the same leased NPUs. Each attempt owns
fresh VFIO/IOMMUFD descriptors and is reaped before the next opens them; failure
or cancellation returns through driver restoration. The chosen width is logged
in the QEMU log. This is capability negotiation during an authorized device
handoff, so Done/Save remains nondestructive and does not run these probes.

References: [QEMU's effective VFIO address width](https://github.com/qemu/qemu/blob/v10.2.0/hw/vfio/device.c#L358-L373),
[VT-d host checks and supported widths](https://github.com/qemu/qemu/blob/v10.2.0/hw/i386/intel_iommu.c),
[the VFIO snoop-control restriction](https://github.com/qemu/qemu/blob/v10.2.0/hw/i386/intel_iommu.c#L3714-L3719).

**Done/Save hardware** batches the device checks and locked-memory budget in
one polkit request. It may load `vfio-pci` and `iommufd`, but does not detach a
device. With no passthrough selected, the existing memory-only authorization
behavior is preserved through a named private helper. Starting a machine obtains fresh authority and may ask again
according to the host's polkit policy. No passwords are collected by vKore.
VFIO pins guest RAM for DMA even with the general memory-pinning option off;
admission checks available RAM and the guardian sets its own bounded memlock
budget (guest RAM plus 1 GiB), inherited by QEMU.

The CLI and desktop have an early, private helper entry point, re-executed with
`pkexec --disable-internal-agent`. It independently checks the polkit caller,
selected identities, isolation, reset and driver state, and takes root-owned
per-device leases under `/run/vkore-vfio`. A bounded request describes the
unprivileged QEMU launch. Only sysfs operations, module loading, resource limits
and opening the selected device handles happen with privilege. QEMU drops to
the caller's UID/GID/groups, with `no_new_privs`, before exec. It receives only
the selected VFIO cdev FDs and an IOMMUFD; global device ownership and ACLs are
unchanged. No shell command or arbitrary program is executed as root.

The helper owns QEMU's lifetime. After guest exit it closes/reaps QEMU before
restoring the original driver, override and reset method. Failed preparation
rolls back partial changes. Parent-pipe closure (including desktop crashes),
Explicit INT, TERM and HUP stop/reap the child before restoration. The guardian
has its own process group so terminal Ctrl-C reaches the CLI, which requests
normal QMP shutdown, rather than racing the guardian's forced cleanup. A failed restoration
is reported in the QEMU log and as a guardian error; it is not reported as a
successful handoff. Forcibly killing the privileged guardian itself with
SIGKILL cannot run cleanup and may leave a device VFIO-bound; this release has
no persistent recovery journal. Reboot restores nonpersistent sysfs overrides.

The implementation reuses the existing `rustix` and `ctrlc` dependencies.
`rust-vmm/vfio-ioctls` was evaluated, but QEMU owns VFIO/IOMMUFD ioctls and DMA
mapping here. Adding a second VFIO device manager would compete with QEMU;
the host helper needs only sysfs lifecycle operations and FD passing.

Tests cover admission, saved identity, stale selection removal, QEMU arguments,
partial-failure rollback, restoration retries, and guest termination on parent
disconnect. Those tests and the ordinary Kore boot gate do not establish a
physical NPU passthrough or inference result. That acceptance still requires a
Linux host, a compatible image, and observation of host-driver restoration.

### Gigatron validation boundary

The [Gigatron report on PR #58](https://github.com/MicroPerceptron/vkore/pull/58#issuecomment-7027330485)
tested Lunar Lake NPU `8086:643e` with QEMU 10.2.1 and a specifically hashed Kore
0.40.0 ISO. The unchanged `369e78c` launch failed before boot. A diagnostic
wrapper using `aw-bits=39,snoop-control=off` reached readiness, executed the
Aftershock NPU lighting workload and reopened it successfully. A separate
normal-stop run exited zero and restored the original driver, override and
reset method. Interrupted wrapper runs restored the driver but exited 143;
they do not establish clean interrupted shutdown.

The subsequent [Biggertron report](https://github.com/MicroPerceptron/vkore/pull/58#issuecomment-7028057059)
validated the unmodified `c911e95` negotiation on Panther Lake NPU `8086:b03e`:
48 bits was refused on its 41-bit host and 39 bits selected. The headless guest
reported the NPU bound and accelerator service up. CLI Ctrl-C completed graceful
QMP shutdown and restored `intel_vpu`; desktop parent disconnect also restored
the driver. Those runs validate bring-up and restoration, not an inference
workload. The desktop/headless-console fixes pushed afterwards and the new
console pane have separate host tests; they still need a fresh hardware run.
`IOMMU_IOAS_MAP failed: Bad address, PCI BAR?` and PCI BAR peer-to-peer warnings
remain visible. Successful lighting does not establish P2P support, inference
numerical parity, throughput, other NPUs, or other image/QEMU combinations.

## Headless shell in the desktop

An ISO that loads `kore-svc-console` gets a separate COM2 socket, selected from
the medium rather than the catalog or the display-window option. The desktop
connects automatically and shows a Console pane on the running machine. Enter
a printable ASCII command and press Enter or Send; each submission sends exactly
one line. ANSI colors, cursor movement, carriage returns, backspace and erase
commands update a bounded terminal screen instead of appearing as escape text.
The screen has 24 rows of 120 columns and retains 104 history rows. Unsupported
characters and invisible text are still escaped in the bundled mono face.
The terminal area stays dark in both app themes; ANSI bold uses bright colors
without loading another font. Input remains line-based, not a full interactive
terminal keyboard.

Input is bounded to 1024 bytes with only one pending line; a disconnected or
partially sent command is never automatically replayed. Socket reads and writes
are nonblocking and bounded per UI tick. The transcript remains visible after
the run ends. CLI runs leave the socket free for an external client such as
`socat`; the desktop occupies the connection while its pane is connected.

Closing the desktop requests normal shutdown and waits for every session to
finish stopping and collecting its workspace changes. New work and pending
starts are cancelled during this drain. Closing again explicitly exits without
waiting and can lose unpublished guest work.

The terminal model reuses MIT-licensed `vt100` 0.18.1 with default features
disabled, which keeps `vte`'s OSC parser buffer fixed at 1024 bytes. Its cell
contents, CSI parameters, screen dimensions and scrollback are bounded; no
`vte/std` feature is enabled in the dependency graph. The reviewed `vt100` source
contains no unsafe blocks and adds only vte/unicode-width to existing helpers.
Default callbacks ignore clipboard, title, resize and reply requests: guest
escapes cannot invoke host actions. Parsing persists across socket reads; the
UI receives styled text, never executable escapes.
