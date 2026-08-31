# vkore-workspace

`vkore-workspace` is the trusted host adapter for Kore workspace replication. It
opens exactly the root selected on the command line with `cap-std`; the
snapshot contains relative canonical names and bytes, never a host path or
file descriptor.

```sh
cargo run -p vkore-workspace -- snapshot ./selected workspace.kws
cargo run -p vkore-workspace -- verify workspace.kws

# Produce the raw Kore filesystem image consumed by the VM's NVMe device.
cargo run -p vkore-workspace -- image-import workspace.kws workspace.raw

# A proposed guest result must descend from the imported base.
BASE_ID=$(cargo run -q -p vkore-workspace -- verify workspace.kws | awk '{print $2}')
cargo run -p vkore-workspace -- snapshot ./proposed desired.kws --parent "$BASE_ID"
cargo run -p vkore-workspace -- diff workspace.kws desired.kws

# Review the printed operations, then approve that exact change-set digest.
cargo run -p vkore-workspace -- apply ./selected workspace.kws desired.kws result.kwr \
  --approve CHANGE_SET_ID
```

The scanner refuses symlinks, hardlinks, sparse files, devices, sockets,
FIFOs, ACL/xattr-bearing objects, path ambiguity, nonportable names, mutation
during scanning, and every frozen size/count overflow. Export revalidates the
whole base and each affected object, writes replacement files through a
private temporary name, records bounded partial completion where a host
filesystem cannot provide a transaction-wide atomic commit, and returns a
previous result unchanged on retry.

Linux, macOS, and Windows use the same canonical manifest. Shared mounts are
not involved. `image-import` is also host-portable: it implements Kore's
ordinary direct-block seam over a capability-selected host file, then uses the
same filesystem formatter, importer, and durable publisher as the founded VM
service. QEMU may consume the raw file directly or a host may convert it to
qcow2. Executable state is descriptive snapshot metadata only; Kore's separate
executable-binding capability remains required to execute bytes.

`make run-boot-gate-x86-workspace` builds the fixed two-seat fixture, boots it,
requires both founded agent shells to prove their exact private roots, then
boots a second VM over the same disk and requires the proofs again. The second
boot reconstructs new grants; no capability identifier is recovered from
disk.
