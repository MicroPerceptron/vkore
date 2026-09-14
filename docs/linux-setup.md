# Run the September 14 development snapshot on Linux

Open the **vKore repository** in your editor and terminal. Run the native app
from a Linux desktop session (X11 or Wayland). The guest is an x86-64 Kore ISO
running under QEMU TCG, including when the host is ARM64. This is a development
checkpoint with known chat recall and broker-refusal/cleanup problems; see
[real chat validation](real-chat-validation.md). Linux execution has not yet
been tested on the destination device.

## Repositories and access

Authenticate GitHub CLI with an account that can read the MicroPerceptron
repositories, including private Kore: `gh auth login`, then `gh auth setup-git`.
Clone these three repositories into sibling directories:

```sh
mkdir -p "$HOME/src"
cd "$HOME/src"
gh repo clone MicroPerceptron/vkore vkore -- --branch codex/test-all-ui-agent-fixes
gh repo clone MicroPerceptron/kore kore-vkore-real -- --branch codex/vkore-linux-handoff-20260914
gh repo clone MicroPerceptron/llama.cpp llama.cpp -- --branch codex/harness-implemenation
git -C kore-vkore-real checkout --detach 3916ae8bc98bc16c685524b7bbb6d8ce65a760e2
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
**vkore-dev-20260914**. Do not use an older Kore release or run `make artifacts`
for this handoff: rebuilding changes packaging metadata and can change the hash.

```sh
cd "$HOME/src"
mkdir -p kore-vkore-real/target
gh release download vkore-dev-20260914 --repo MicroPerceptron/kore \
  --pattern kore-x86-release.iso --pattern SHA256SUMS \
  --dir kore-vkore-real/target
(cd kore-vkore-real/target && sha256sum -c SHA256SUMS)
```

Expected ISO SHA-256:

```text
36f49b9f4f84ab05c22c31f3ab7b080e66682ffd842bf9200d81064a2f936e71
```

The ISO was built from `fd383fb8705e63400e7b78762185f3208ee355c2`
plus tracked patch digest
`70a32975fe1dbef9614d317ef9eb7febfc1c1fad5e15c1f8aaebbf839b77f39b`.
Commit `3916ae8bc98bc16c685524b7bbb6d8ce65a760e2` records those tracked
source files and guest binaries, plus the previously untracked authority test.
The existing ISO retains the original build identity. The handoff lock binds
this committed snapshot to those exact ISO bytes and requires a clean tracked
Kore worktree. It does not claim the ISO was rebuilt from the new commit.

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

The existing vKore workspace tests and Python process-session tests passed on
the Mac. The native release app launched and the backend health endpoint passed.
The Linux setup helper is checked in isolation, and the firmware-format change
is shell-syntax checked. This is not a new Linux end-to-end acceptance pass.
Prior Kore gate outcomes and the outstanding host-test compilation failure are
recorded in `docs/real-chat-validation.md`; the guest was not rebuilt for this
source/artifact publication.
