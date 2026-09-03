# Package installation, updates, and recovery

vKore manages the host artifacts needed by a project without taking ownership
of workspaces, conversations, policies, credentials, or user model libraries.
Those data classes are never stored under the managed object directory and are
never removed by package cleanup.

## Manifest and trust

Every artifact has a versioned manifest containing its package ID, semantic
version, class, release channel, source and project URLs, publisher, license,
BLAKE3 content digest, sizes, platform constraints, protocol ranges,
dependencies/conflicts, entry points, declared permissions, and health check.
Kore packages additionally reference the guest-produced boot-manifest digest;
vKore does not replace that manifest. Models record family, quantization,
context, template, estimated RAM/VRAM, provenance, and redistribution status.

Official and downloaded packages require an Ed25519 signature from a key
explicitly mapped to the publisher in `TrustPolicy`. A signature establishes
publisher identity and artifact integrity, not code safety. Caller-owned local
QEMU, Kore, llama.cpp, GGUF, add-on, and guest-tool artifacts may be registered
unsigned only when local-development trust explicitly permits it.

To publish a release, serialize `PackageManifest` as JSON, sign those exact
bytes, attach the lowercase hexadecimal signature, publish the artifact without
mutation, and place its public key in the intended release channel's trust
configuration. Reusing one package ID/version for different bytes is refused.

## Download and compatibility

Pinned HTTP/HTTPS downloads use a `.part` file and resume with a byte range.
The completed size, content digest, publisher signature, license/provenance,
host OS/architecture/features/acceleration, and HostLink/workspace/add-on/
inference protocol ranges are checked before the object is registered. A store
size ceiling gives a deterministic low-disk refusal before copying.

`HostPlatform::detect` reports the current OS, architecture, and available VM
acceleration path (`whpx`, `hvf`, or `kvm`). Projects may add measured RAM/VRAM
to the platform record to produce `Fits`, `Tight`, `DoesNotFit`, or `Unknown`
model-fit status before launch.

Stable and beta channels are separate. A project may select the newest release
in a channel or pin an exact version. Models and runtimes remain pinned per
project until the user changes them.

## Activation and rollback

Artifacts live in a content-addressed object store. Activation updates a small
atomic record containing current and previous versions. vKore refuses runtime
or image changes while that class has active workloads. It then activates the
candidate, runs a bounded component-specific health check, and restores the
previous record automatically if launch, HostLink negotiation, model startup,
or another required check fails.

Add-on updates receive a structured permission diff. New destinations,
credential kinds, filesystem scopes, host applications, or tool effects require
a separate authenticated approval before activation. Installed identity,
version, and digest remain available to tool-capability and audit records.

For recovery, stop or drain active workloads and call rollback for the affected
artifact class. Both automatic and explicit rollback run the previous version's
health check. Installation, activation, rollback, unregister, and cleanup are
recorded in `audit.jsonl`.

## Offline and cleanup

An offline tar bundle contains exactly `manifest.json` and `artifact`; import
streams the artifact into temporary storage and performs the same signature,
digest, platform, protocol, and size checks before registration. Unexpected
archive entries are refused.

Disk reporting separates installed objects and incomplete downloads.
Unregistering a package retains its object. Cleanup first returns exact
unreferenced digests, then deletes only the explicit subset supplied by the
user. Active and rollback versions are never candidates.
