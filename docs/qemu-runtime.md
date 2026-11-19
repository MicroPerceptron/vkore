# Packaged QEMU runtime

vKore remains a Cargo workspace. QEMU is an external, supervised process;
its own configure/Meson/Ninja build runs only in the packaging pipeline.
There is no QEMU C ABI or emulator implementation linked into the desktop.

## Build and layout

`packaging/qemu.lock.json` pins QEMU 11.0.1, its upstream source URL and
SHA-256 (matching the installed Homebrew 11.0.1 source receipt used during
integration). A pin update requires rebuilding both host bundles, capability
probes, and a real Kore ISO boot. Version numbers do not replace device probes.

Use Python 3.12 or newer, a C/C++ toolchain, Meson 1.9.1, setuptools 80.9.0, wheel 0.45.1, Ninja and pkg-config.
Linux additionally needs glib, pixman, libslirp, zlib and GTK development
packages plus patchelf. macOS needs Xcode command-line tools and Homebrew
glib, pixman and libslirp. The workflow lists the exact setup commands.
Allow at least 4 GiB of build space. Work and output directories must be new.

```sh
python3 tools/build-qemu-runtime.py \
  --work-dir /tmp/vkore-qemu-build \
  --output /tmp/qemu-runtime --jobs 3
python3 tools/build-qemu-runtime.py --verify /tmp/qemu-runtime
```

`--source-archive` accepts an already downloaded release tarball; its checksum
must still match the lock. configure invokes Meson; Ninja builds and installs.
The target is x86_64 system emulation with TCG, native acceleration (KVM on
Linux, HVF on macOS), slirp, VNC and the host window backend. Installed firmware,
VGA BIOS and runtime data come from this same source release. Non-system shared
libraries are copied and loader paths rewritten. The bundle has no dependency
on the original build directory. Linux keeps the distribution's glibc/loader;
build on the oldest supported distribution. macOS keeps Apple system libraries.

The pipeline verifies checksums and version after relocation, then negotiates
QMP with a stopped Q35 machine using the packaged UEFI code and a private vars
copy. This is an emulator smoke test, **not a Kore boot acceptance test**.

Install the resulting directory beside the application:

```text
bin/vkore
bin/vkore-ui
libexec/vkore/qemu-runtime/
  bin/qemu-system-x86_64
  lib/...
  share/qemu/...
  licenses/...
  source.json
  bundle.json
```

A macOS app can instead put it at `Contents/Resources/qemu-runtime` beside
`Contents/MacOS/vkore-ui`. Native binaries are ad-hoc signed after relocation;
distribution signing/notarization must be applied before generating the final
manifest, since signing changes checksummed bytes. CI artifacts are development
bundles, not signed application releases.

Selection order is `VKORE_QEMU`, `VKORE_QEMU_BUNDLE`, an installed sibling/app
bundle, then `qemu-system-x86_64` on PATH. The bundle override names the runtime
root, not its executable. Explicit machine firmware settings retain precedence.
A selected bundle must match the host OS/architecture and pass its manifest
checks. Missing/corrupt assets are errors; they never trigger a silent fallback.
The manifest detects corruption; release authentication is the distributor's
signature, not a self-signed checksum file.

QEMU remains GPL-licensed and the libraries/firmware retain their licenses.
The CI artifact includes the exact upstream QEMU source archive and QEMU
license texts. Before public distribution, include the applicable notices and
corresponding source for every bundled dependency as well, plus the signed
application's release manifest. Do not label the combined runtime MPL-only.

## Typed management and supervision

`qapi-qmp = 0.15.0` provides generated command, response and event types;
`qapi-spec = 0.3.2` supplies their contracts. These MIT crates are passively
maintained, so their older schema is used only for commands supported by our
probes/guest contract. Unknown newer events retain their raw envelope for the
transcript. No transport or async runtime from qapi-rs is adopted. Its generated
enum helpers contain an unsafe table-indexing implementation; table indices
and variants are generated together. vKore adds no unsafe code here. Additional
build dependencies are qapi-codegen and qapi-parser; runtime types use Serde
and base64, already in the workspace. Allocation occurs on the management path,
not the guest's inference path.

The existing transport still correlates every response by id and drops late
responses. Lines are bounded to 1 MiB, queued events to 256 and 4 MiB total.
`rustix` (already a workspace transitive dependency, MIT/Apache-2.0) supplies
portable polling with an overall read deadline, avoiding repeated socket-timeout
updates after peer shutdown on macOS. Partial lines survive timeouts.

The supervisor starts guest CPUs paused, negotiates QMP, checks the stopped
state and explicitly resumes. Failed setup kills and reaps the child; failure
logs are bounded to a 64 KiB tail. Runtime firmware templates remain read-only,
and each machine retains its private variable store. This establishes the
pre-resume boundary for future core placement and privileged resource setup;
it does not yet add the hardware-assignment UI or a privileged helper.

## Validation

```sh
python3 -m unittest discover -s tools -p 'test_qemu*.py' -v
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
VKORE_TEST_QEMU=/path/to/qemu-system-x86_64 \
VKORE_TEST_ISO=/path/to/kore.iso cargo test -p vkore-vm --test host -- --nocapture
```

The QEMU runtime workflow builds Linux x86_64 and macOS arm64 bundles separately
from Cargo. A successful smoke test does not prove SR-IOV, physical hardware
performance or the current Kore boot contract; those require the real image
and appropriate host devices.
