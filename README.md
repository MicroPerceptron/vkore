# vKore

vKore is the open-source host control plane for running [Kore](https://github.com/MicroPerceptron/kore) in guest mode.

The repository owns host resources and product integration: VM launch and supervision, the host side of HostLink, capability-rooted workspace access, llama.cpp inference supervision, the live inspection UI, transcripts, and host diagnostics. Kore remains the authority-bearing guest. vKore cannot mint guest capabilities or turn a host path, model string, tool name, or UI event into authority.

## Current extraction

The initial implementation is extracted from Kore PR [#722](https://github.com/MicroPerceptron/kore/pull/722) at commit `da97303982d1e75200b0e521539c43921b842bc0`. That commit already contains PR [#668](https://github.com/MicroPerceptron/kore/pull/668), including the bounded inference registry and llama.cpp supervisor.

Shared HostLink, workspace, block, and guest-filesystem contracts remain Kore-owned dependencies pinned to that exact revision. vKore does not maintain a second copy of their wire definitions.

The migrated packages are:

- `vkored`: HostLink host endpoint, QEMU argument construction, deterministic and llama.cpp inference backends, workspace composition, live observer, transcript, and daemon lifecycle.
- `vkore-vm`: caller-supplied QEMU/Kore-image launch, exact HostLink device topology, exit observation, and teardown.
- `vkore-workspace`: root-confined host workspace snapshot, live range service, writeback journal, and image import tooling.
- `vkore-hostlink-ab`: bounded HostLink transport measurement adapter.

Product packages added in vKore include:

- `vkore-addon-sdk`: capability-scoped host, guest, and split tool contracts and runtime.
- `vkore-reference-addon`: reference out-of-process host tool and Kore guest tool manifest.
- `vkore-policy`: authenticated approvals, standing policies, revocation, and audit history.
- `vkore-packages`: signed, content-addressed artifacts, updates, compatibility, and rollback.
- `vkored` interactive binaries: loopback harness server and headless `vkore chat` client.

See [the add-on SDK guide](docs/addons.md) for the public extension and security model.
See [the policy guide](docs/policy.md) for approval lifetimes and matching semantics.
See [the package guide](docs/packages.md) for installation, signing, offline use, and recovery.
See [the harness API guide](docs/harness-api.md) for interactive sessions, events, and CLI usage.
See [the project UI guide](docs/project-ui.md) for the setup wizard, local chat, and real inference configuration.

## Native Axiom UI

Start the deterministic end-to-end harness from this repository:

```powershell
cargo run -p vkored --bin vkore-harnessd
```

In a second terminal, launch the native client:

```powershell
cargo run -p vkore-ui -- --server 127.0.0.1:8787
```

The window is rendered by Axiom directly into its presentation surface; no
HTML, browser engine, WGPU, or viewport-sized staging buffer is used. There is
no login, token, or browser session step. The deterministic mode
exercises broker planning, prompt-scoped capability installation, guest-domain
tool calls, validated output, cancellation, teardown, and KV retirement without
requiring a model download.

## Build and test

Kore is currently private, so Cargo is configured to use the authenticated Git CLI for the pinned shared dependencies.
The workspace temporarily uses Kore's pinned nightly because the migrated
workspace image importer links Kore's host-testable filesystem service.

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Run the host daemon

Deterministic backend:

```powershell
cargo run -p vkored -- `
  --control-tcp 127.0.0.1:7070 `
  --bulk-tcp 127.0.0.1:7071 `
  --ui-listen 127.0.0.1:8787
```

Caller-supplied llama.cpp and model:

```powershell
cargo run -p vkored -- `
  --control-tcp 127.0.0.1:7070 `
  --bulk-tcp 127.0.0.1:7071 `
  --llama-server C:\path\to\llama-server.exe `
  --llama-model C:\path\to\model.gguf `
  --ui-listen 127.0.0.1:8787
```

Selected live workspace with bounded writeback:

```powershell
cargo run -p vkored -- `
  --control-tcp 127.0.0.1:7070 `
  --bulk-tcp 127.0.0.1:7071 `
  --workspace-root C:\path\to\project `
  --workspace-state C:\path\to\vkore-state `
  --workspace-write
```

The `vkored` page is a low-level loopback observer. `vkore-harnessd` serves the
loopback project wizard and interactive harness UI.

Managed VM launch is optional. Supply the QEMU base topology required by the
Kore image as repeated arguments; vKore adds the two HostLink devices and the
caller-selected ISO without invoking a shell:

```powershell
cargo run -p vkored -- `
  --control-tcp 127.0.0.1:7070 `
  --bulk-tcp 127.0.0.1:7071 `
  --qemu C:\path\to\qemu-system-x86_64.exe `
  --kore-iso C:\path\to\kore.iso `
  --qemu-arg -machine --qemu-arg q35 `
  --qemu-arg -m --qemu-arg 4G `
  --ui-listen 127.0.0.1:8787
```

When the managed VM exits, `vkored` observes its status, flushes the requested
transcript, and exits. If `vkored` stops first, it terminates the child VM.

## Cross-repository gate

The interactive product gate starts the loopback harness, creates a real
project through its HTTP API, submits `read X`, boots Kore, and requires the
broker proposal, capability installation, guest `cat` execution, tool result,
assistant response, and terminal event to return through HostLink:

```sh
KORE_ROOT=/path/to/kore-frontend-ingress ./scripts/kore-interactive-gate.sh
```

Set `VKORE_GATE_ARTIFACT_DIR` to retain the passing serial, debug, HostLink,
and event-stream witness. `MAKE` and `QEMU` may select non-default GNU Make
and QEMU executables.

The earlier deterministic QEMU security gate remains available. It builds
vKore's daemon/workspace tools, then asks Kore's guest-development Make target
to build and boot the guest artifact:

```sh
KORE_ROOT=/path/to/kore-pr722 ./scripts/kore-qemu-gate.sh
```

That gate proves the HostLink handshake, workspace import, two isolated agent
tool loops, prompt-scoped authority lifecycle, stale-generation protection,
and pre-expansion denial of the sibling secret.

Use a dedicated Kore worktree: if a local source rebuild changes the committed
service ELFs, the gate refreshes that worktree's measured-boot digests before
building the ISO. Native x86_64 or hardware-assisted x86 virtualization is
recommended. On this ARM64 Windows/WSL host, QEMU TCG reached both shells and
passed the HostLink isolation/versioned-reply checks, but PR #722's durable
app-store seed stalled at 1/23 and timed out before the final tool-loop oracle.

## Host support

| Host | Status | Transport and limitations |
| --- | --- | --- |
| Windows | tested | Loopback TCP HostLink; unit/conformance suites pass. Managed QEMU requires a caller-supplied Windows QEMU and compatible firmware arguments. |
| Linux | tested | Unix sockets and loopback TCP; full unit/conformance suites pass. The deterministic QEMU product gate is provided for x86_64 emulation or acceleration. |
| macOS | source-supported | Unix sockets and portable serial HostLink topology; run the same Cargo suites locally. Hardware acceleration and firmware paths are caller-specific and are not tested in this checkout. |

The real llama.cpp smoke remains opt-in and requires caller-owned paths:

```sh
KORE_LLAMA_SERVER=/path/to/llama-server \
KORE_LLAMA_MODEL=/path/to/model.gguf \
cargo test -p vkored --test llama_backend \
  real_llama_server_runs_generic_broker_and_agents_on_one_continuously_batched_model \
  -- --ignored --exact --nocapture
```

## Repository boundary

Remain in Kore:

- kernel capability lineage, tasks, mappings, and generations;
- guest HostLink routes and inference mailboxes;
- exact filesystem and executable grants;
- resident broker validation and atomic authority transactions;
- agent founding, budgets, cancellation, and teardown;
- ring-3 agent/tool runner and guest security gates.

Belong in vKore:

- VM and host transport lifecycle;
- selected host workspace access and writeback adapter;
- model/runtime supervision and KV-slot cleanup;
- UI, transcript, package management, approval UX, and add-ons;
- product-level host integration and compatibility tests.

See [MIGRATION.md](MIGRATION.md) for the extracted source inventory and compatibility contract.
