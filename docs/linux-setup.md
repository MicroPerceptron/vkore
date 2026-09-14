# Run the PR #39 development stack on Linux

Open the **vKore repository** in your editor and terminal. Run the native app
from a Linux desktop session (X11 or Wayland). The guest is an x86-64 Kore ISO
running under QEMU TCG, including when the host is ARM64. This is the development stack for PR #39; see
[real chat validation](real-chat-validation.md). Linux execution has not yet
been tested on the destination device.

## Repositories and access

Authenticate GitHub CLI with an account that can read the MicroPerceptron
repositories, including private Kore: `gh auth login`, then `gh auth setup-git`.
Clone these three repositories into sibling directories:

```sh
mkdir -p "$HOME/src"
cd "$HOME/src"
gh repo clone MicroPerceptron/vkore vkore -- --branch codex/wip-live-chat-session-fixes
gh repo clone MicroPerceptron/kore kore-vkore-real -- --branch codex/frontend-response-lifecycle
gh repo clone MicroPerceptron/llama.cpp llama.cpp -- --branch codex/harness-implemenation
git -C kore-vkore-real checkout --detach 2b1307786a0a354864a14b98578f73e413cae208
git -C llama.cpp checkout --detach 0b953005451976d78c8227b16ad3d42a07c2053b
```

Cargo fetches the pinned Axiom and Iced forks automatically; you need repository
access but do not need separate working copies. Keep Cargo.lock and the pinned
Rust toolchain. Existing clones can fetch and select these branches instead;
preserve any local work before switching branches.

The llama.cpp pin matches the server's reported source revision on the Mac.
That checkout also contains unrelated, uncommitted MoE cache experiments;
those experiments are not part of this dense Qwen 9B setup or this handoff.

## Linux prerequisites

For Debian/Ubuntu, the starting package set is:

```sh
sudo apt-get update
sudo apt-get install git gh build-essential cmake ninja-build pkg-config \
  python3 curl coreutils qemu-system-x86 qemu-utils ovmf libssl-dev \
  libx11-dev libx11-xcb-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig1-dev
```

Install Rust through rustup if it is not already installed, then:

```sh
rustup toolchain install nightly-2026-07-13 --profile minimal --component rustfmt,clippy
```

Other Linux distributions need equivalent packages. QEMU must provide the
`max` x86 CPU model, q35/HMAT/NUMA, Intel IOMMU and bochs-display devices used by
Kore. The Mac currently uses QEMU 11.1.0; an older distribution version may need
upgrading. `make dev-check` verifies paths and artifact pins, while actual boot
is the check for QEMU compatibility. Have enough free RAM for the 5.3 GiB model,
its 32K context and three slots, the 2 GiB guest, and the desktop/build tools.

The Kore Makefile reads an installed QEMU UEFI firmware descriptor. If preflight
reports missing OVMF, inspect `/usr/share/qemu/firmware/`. You can select the
installed x86-64 descriptor explicitly without changing tracked Kore files:

```sh
export MAKEFLAGS="OVMF_JSON=/usr/share/qemu/firmware/YOUR-X86-64-DESCRIPTOR.json"
```

Use a non-Secure-Boot descriptor where available. The launcher now respects raw
and qcow2 firmware formats.

## Download the exact ISO

Use the asset **kore-x86-release.iso** from the Kore development prerelease
**vkore-pr39-20260914**. Do not use an older Kore release or run `make artifacts`
for this handoff: rebuilding changes packaging metadata and can change the hash.

```sh
cd "$HOME/src"
mkdir -p kore-vkore-real/target
gh release download vkore-pr39-20260914 --repo MicroPerceptron/kore \
  --pattern kore-x86-release.iso --pattern SHA256SUMS \
  --dir kore-vkore-real/target
(cd kore-vkore-real/target && sha256sum -c SHA256SUMS)
```

Expected ISO SHA-256:

```text
7dec906af7a1ddffb6a5fdd3e3b4dbde37ac9951bd3d0ebf8286c4b50526fdfd
```

The ISO is packaged from Kore commit `2b1307786a0a354864a14b98578f73e413cae208`. Its
tracked x86 userland artifacts were rebuilt from source commit
`2db155cfb9ac6621bfdd723966157c22022fae4d`; the shell build stamp identifies
that source revision. `config/real-stack.lock.mk` binds the packaged commit to
the exact ISO bytes and requires a clean tracked Kore worktree.

## Build llama.cpp and get the model

For a CPU build:

```sh
cd "$HOME/src/llama.cpp"
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build --target llama-server -j "$(nproc)"
mkdir -p gguf
curl -fL --retry 3 -o gguf/Qwen3.5-9B-Q4_K_M.gguf \
  https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/3885219b6810b007914f3a7950a8d1b469d598a5/Qwen3.5-9B-Q4_K_M.gguf
printf '%s  %s\n' \
  03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8 \
  gguf/Qwen3.5-9B-Q4_K_M.gguf | sha256sum -c -
```

You may copy the same model file from the Mac instead. Model weights are not
stored in these Git repositories. For an NVIDIA GPU with drivers and the CUDA
toolkit installed, configure with `-DGGML_CUDA=ON` and pass `--gpu-layers 99`
to the setup helper below. Consult the pinned llama.cpp `docs/build.md` for
other GPU backends. CPU inference works as a configuration option but can be
slow and may exceed the configured 900-second prompt timeout.

## Configure this device and launch

Use paths without spaces. Choose a narrow workspace directory; the project is
configured for automatic writeback inside that directory, matching the Mac.
The following creates an empty test workspace. Application state is separate.

```sh
mkdir -p "$HOME/src/vkore-testworkspace"
cd "$HOME/src/vkore"
python3 scripts/setup-real-project.py \
  --kore-root "$HOME/src/kore-vkore-real" \
  --llama-server "$HOME/src/llama.cpp/build/bin/llama-server" \
  --model "$HOME/src/llama.cpp/gguf/Qwen3.5-9B-Q4_K_M.gguf" \
  --workspace "$HOME/src/vkore-testworkspace"
make dev-check
make dev-real
```

The helper creates ignored `Makefile.local` and a project JSON under
`~/.local/state/vkore-real/projects/`. It refuses to overwrite existing setup.
It defaults to CPU inference; add `--gpu-layers 99` for a configured GPU build.
No transcripts, credentials, or Mac-specific runtime state need to be copied.

The native window opens project `qwen9b-q4km-real`. There is no website to open.
Keep the launch terminal alive. Closing the window stops its harness and child
processes. Close any existing launcher before launching another on port 8787.
For a headless machine, use `make dev-debug PROMPT="hello"` instead; this runs
one actual model/guest prompt and retains diagnostics.

## Verification of this handoff

The September 14 validation rebuilt the guest, passed the host suites and all
three architecture checks, passed the kernel boot test and production gate
with the two-CPU/2-GiB vKore configuration, and exercised the native Mac UI.
`make dev-check` verified the published source/image pins. The setup helper
was checked in isolation, including state-directory creation and refusal to
overwrite existing configuration. See [real chat validation](real-chat-validation.md)
for the live prompt results and remaining bounds. Linux end-to-end execution
has not yet been verified on the destination device.
