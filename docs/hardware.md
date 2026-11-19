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
`--no-memory-pinning`, `--pages system|2m|1g` and `--sparse`.

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

Memory locking is currently the only setting requiring elevation. The shared
completion/launch authorization path raises the current vKore process's
memory-lock limit to guest RAM plus a bounded 1 GiB emulator allowance. No popup
appears when pinning is off or the process already has enough permission. It
first uses the existing hard limit without prompting; if
necessary it invokes `/usr/bin/pkexec --disable-internal-agent /usr/bin/prlimit`
with only the current PID and numeric limit. The registered desktop polkit agent
presents authentication. No password enters vKore, no shell is invoked, no
privileged QEMU is launched, and no persistent system configuration is changed.
QEMU inherits the limit. Limits last only for this app process; launch rechecks
and may request authorization after restarting vKore. Cancellation, denial, missing agent and a two-minute
timeout return visible errors. QEMU's actual locking failure remains fatal.
The CLI reports a needed authorization rather than prompting on a hidden worker.

Host hugepage pools are not globally reconfigured. Unusable page sizes remain
disabled, with system pages available. This avoids silently consuming or
reassigning a pool another workload relies on.

## Storage and ownership

The panel displays mounted storage devices, filesystem, SSD/HDD classification,
capacity and free space. Machine disks remain in the vKore state filesystem;
the displayed reservation capacity is queried for that path, including APFS
mount aliases. Selecting an arbitrary physical disk is outside this UI.

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
  No new application unsafe code. fs4 was evaluated but its allocation path
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
