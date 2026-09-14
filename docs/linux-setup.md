# Run the shared-assistant development stack on Linux

Open the **vKore repository** in your editor and terminal. Run the native app
from a Linux desktop session (X11 or Wayland). The guest is an x86-64 Kore ISO
running under QEMU TCG, including when the host is ARM64. This implements issue #24 on top of PR #39; see
[shared assistant validation](issue24-validation.md) and [the architecture decision](adr/0024-shared-assistant.md). Linux execution has not yet
been tested on the destination device.

## Repositories and access

Authenticate GitHub CLI with an account that can read the MicroPerceptron
repositories, including private Kore: `gh auth login`, then `gh auth setup-git`.
Clone these three repositories into sibling directories:

```sh
mkdir -p "$HOME/src"
cd "$HOME/src"
gh repo clone MicroPerceptron/vkore vkore -- --branch codex/issue24-shared-assistant
gh repo clone MicroPerceptron/kore kore-vkore-real -- --branch codex/issue24-resident-broker
gh repo clone MicroPerceptron/llama.cpp llama.cpp -- --branch codex/harness-implemenation
git -C kore-vkore-real checkout --detach 3fca0fe1f8578d59c16c465089967ecb268b7f79
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

Use the asset **kore-x86-vkore-headless-release.iso** from the Kore development prerelease
**vkore-issue24-20260914**. Do not use an older Kore release or run `make artifacts`
for this handoff: rebuilding changes packaging metadata and can change the hash.

```sh
cd "$HOME/src"
mkdir -p kore-vkore-real/target
gh release download vkore-issue24-20260914 --repo MicroPerceptron/kore \
  --pattern kore-x86-vkore-headless-release.iso --pattern SHA256SUMS \
  --dir kore-vkore-real/target
(cd kore-vkore-real/target && sha256sum -c SHA256SUMS)
```

Expected ISO SHA-256:

```text
6ae17033d2fca456567c35626618fcf18d60106e401e60276863b4a2df570aa0
```

The ISO is packaged from Kore commit `3fca0fe1f8578d59c16c465089967ecb268b7f79`. Its
tracked x86 userland artifacts were rebuilt from source commit
`5173ebce12f6b6631792e8967142b87fcf4c6789`; the shell build stamp identifies
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

Use paths without spaces. The following creates a personal-assistant project
without a physical focus folder. Application settings own the single model
server and the 2-CPU/2-GiB VM. Both begin preparing as the app opens.

```sh
cd "$HOME/src/vkore"
python3 scripts/setup-real-project.py \
  --kore-root "$HOME/src/kore-vkore-real" \
  --llama-server "$HOME/src/llama.cpp/build/bin/llama-server" \
  --model "$HOME/src/llama.cpp/gguf/Qwen3.5-9B-Q4_K_M.gguf"
make dev-check
make dev-real
```

The helper creates ignored `Makefile.local`, application `settings.json`, and
a project JSON under `~/.local/state/vkore-real/projects/`. It refuses to overwrite existing setup.
It defaults to CPU inference; add `--gpu-layers 99` for a configured GPU build.
No transcripts, credentials, or Mac-specific runtime state need to be copied.

To work on a particular folder, create it first and add `--workspace /absolute/path`
to the setup command. This optional focus project enables automatic writeback
inside that folder. Opening a focus project still requires the broker to grant
its workers specific files or a directory.

After launch, use Settings → Document access → Add folder to authorize folders
for read-only personal search. No folders are authorized by default. Supported
text, PPTX slide text and DOCX document text can be discovered; PDF/OCR and
embedded images are not supported. The filesystem tab shows effective task
grants, not the searchable library.

The native window opens project `qwen9b-q4km-real` (named Personal assistant
when no focus folder was supplied). There is no website to open. Watch the
status bar for model, VM and broker readiness; the brief native animation is
independent. Prompts entered during startup stay queued for their chat. New
projects and chats reuse the same processes. Settings’ active model applies
application-wide; changing it explicitly restarts preparation. If preparation
fails, correct the reported configuration and use Retry.
Keep the launch terminal alive. Closing the window stops its harness and child
processes. Close any existing launcher before launching another on port 8787.
For a headless machine, use `make dev-debug PROMPT="hello"` instead; this runs
one actual model/guest prompt and retains diagnostics.

## Verification of this handoff

The September 14 validation rebuilt the guest, passed the host suites and all
three architecture checks, ran the kernel boot suite with four CPUs/4 GiB and
the production gate with the two-CPU/2-GiB vKore configuration, and exercised
the native Mac UI. See the validation report for exact gate results and the
two-CPU kernel-test distribution failures.
`make dev-check` verified the published source/image pins. The setup helper
was checked in isolation, including state-directory creation and refusal to
overwrite existing configuration. See [shared assistant validation](issue24-validation.md)
for the live prompt results, startup timings and remaining bounds. Linux end-to-end execution
has not yet been verified on the destination device.
