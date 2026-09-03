# Host migration inventory

This repository's initial host implementation was mechanically migrated from `MicroPerceptron/kore` PR #722, commit `da97303982d1e75200b0e521539c43921b842bc0`.

## PR #668 audit

PR #668 head `6fbb8998655f5e2b92a4f9955f7081e6b5031c57` is an ancestor of the selected PR #722 head. The integration merge `38aef26d` brings that inference work into the dynamic-agent stack. No separate #668 patch is required.

The migrated `vkored` source therefore includes:

- the 32-sequence bounded broker/worker inference registry;
- KIR1/KIE1 framing, ordering, cancellation, and refusal behavior;
- deterministic round-robin scheduling and metrics;
- llama.cpp health/slot negotiation, continuous batching, HTTP/SSE streaming, and socket cancellation;
- per-sequence llama.cpp slot assignment and erase-on-retirement;
- restart poisoning and late-result rejection.

PR #722 adds prompt-scoped broker planning, generalized broker/worker schemas, assistant responses, capability-state observations, and the loopback live inspector on top of that foundation.

## Source mapping

| Kore PR #722 path | vKore path | Status |
| --- | --- | --- |
| `tools/hostlinkd` | `crates/vkored` | migrated |
| `tools/workspacectl` | `crates/vkore-workspace` | migrated |
| `tools/hostlink-ab` | `crates/vkore-hostlink-ab` | migrated |
| `tools/hostlinkd/src/qemu.rs` and host process lifecycle | `crates/vkore-vm` | migrated and extended |
| Product prompt/QEMU host gate | `scripts/kore-qemu-gate.sh` | migrated; Kore retains the thin guest build/boot target |
| `crates/hostlink` | pinned Kore dependency | guest/shared source of truth |
| `crates/hostlink-shmem` | pinned Kore dependency | shared source of truth |
| `crates/workspace-sync` | pinned Kore dependency | shared source of truth |
| `crates/block` | pinned Kore dependency | guest/shared source of truth |
| `services/fs` | pinned host-test dependency | guest authority remains in Kore |

## Compatibility rule

The pinned Kore revision is the compatibility identity until versioned shared crates are published through `libkore`. Updating it requires running every vKore host test plus the corresponding Kore HostLink, workspace, inference, prompt-broker, agent-tool, and QEMU gates.

Do not copy shared protocol structs into vKore. Do not remove the compatibility host tools from Kore until the vKore gates reproduce their current behavior on supported hosts.

## Migration corrections

Linux testing exposed an inherited `workspacectl` defect: `cap-std` retains a
directory as an `O_PATH` descriptor, which cannot be passed directly to
`flistxattr`. vKore now reopens `.` relative to that retained descriptor for
the metadata check. This preserves root confinement and makes the complete
workspace suite pass on Linux rather than silently skipping metadata.
