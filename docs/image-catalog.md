# Official image catalog v1

This is the host-side release catalog and download contract, not a guest
transport protocol. Kore owns image builds; vKore owns catalog admission and
host downloads. No Kore or libkore wire types are introduced.

## Trust and delivery

The catalog is UTF-8 JSON embedded in the vKore binary from
`crates/vkored/assets/official-images.json`. Updating it requires a vKore release
through the same trusted distribution path as the executable. Users select an
image ID; the runtime resolves the URL and digest from its own bundled catalog.
The desktop does not accept arbitrary links, checksums, or replacement catalogs.
A SHA-256 authenticates bytes against this trusted catalog; HTTPS alone does not
make an arbitrary artifact official.

No official artifacts have been supplied yet. The shipped catalog is valid and
empty, and creation from the desktop is disabled. There are no example URLs,
placeholder digests, or mock downloads in production. Existing machines remain
usable; the CLI's explicit local/custom ISO workflow remains available to developers.

A remotely updated catalog is outside v1. It requires a separately specified
signature envelope, trusted keys, expiry, rollback protection and key rotation
before a remote response can replace bundled release metadata.

## Schema

The top-level object contains exactly `schema_version: 1` and `images: []`.
The document is at most 256 KiB and contains at most 64 entries. Unknown fields,
unknown versions, invalid entries and duplicate IDs fail the whole catalog.
Every image entry requires these fields:

| Field | Contract |
| --- | --- |
| `id` | Stable immutable artifact ID; 1–48 ASCII letters, digits, `-`, `_`; no leading `-`. A new artifact gets a new ID. |
| `name` | User-facing name, 1–64 printable ASCII characters. |
| `version` | Release/build label, 1–64 printable ASCII characters. |
| `profile` | `desktop` or `inference`; describes the image, does not advertise an inference backend. |
| `architecture` | `x86_64` in v1. This is the guest architecture, not the host architecture. |
| `format` | `iso` in v1, an uncompressed bootable Kore ISO. |
| `url` | Absolute HTTPS URL, at most 2048 bytes, with a host and no credentials or fragment. No tokens, signed expiring links or secrets may be embedded in releases. |
| `sha256` | Exactly 64 lowercase hexadecimal characters, the digest of the complete uncompressed ISO. Required. |
| `bytes` | Exact ISO length in bytes, greater than zero and at most 64 GiB. Required even when HTTP omits Content-Length. |
| `kore_revision` | Full 40-character lowercase hexadecimal source commit that built the ISO. |
| `dagfs_format_version` | Must match the filesystem format supported by vKore's pinned libkore engine. |

The source revision records provenance; matching the volume format alone does
not prove console, firmware, accelerator or inference compatibility. Publishers
must boot and validate each artifact against the intended vKore release. vKore's
existing QEMU/firmware/paging preflight continues to probe the actual medium and
host. SR-IOV readiness and inference throughput are not asserted by the catalog.

## Acquisition and publication

1. Resolve the selected ID from the bundled, validated catalog. Unknown IDs
   fail before a network request or machine creation.
2. Fetch over HTTPS, including redirects (at most five). HTTP must return 200;
   a Content-Length, if present, must match the catalog's exact byte count.
   Compression is not negotiated or decoded.
3. Stream into a private temporary file, bounded by the catalog byte count and
   the existing 64 GiB ceiling. An absent Content-Length does not relax bounds.
   Short and oversized bodies fail. Existing connection/read/total deadlines
   and cancellation remain in force.
4. Sync the file, inspect the ISO through `vkore-vm`, and compare its full
   SHA-256 with the catalog digest. Never create a machine for failed verification.
5. Publish without replacement as `images/SHA256.iso`. An existing cache entry
   must be a regular file of the correct length and digest before reuse. Errors
   remove partial files; completed cache entries may outlive machine creation.
6. Create the machine atomically with the verified path and digest. Existing
   configurations and disks are not replaced. Boot preflight still re-inspects
   the ISO using its existing behavior; catalog admission is not a boot result.

Version, profile and source revision describe the selected artifact. Resource
sizes remain machine configuration; the catalog does not invent guest endpoints,
boot markers, model readiness or QEMU hardware requirements ahead of their owners.
