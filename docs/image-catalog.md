# Official image catalog v1

This is the host-side release catalog and download contract, not a guest
transport protocol. Kore owns image builds; vKore owns catalog admission and
host downloads. No Kore or libkore wire types are introduced.

## Live archive service

The desktop loads `https://dev.microperceptron.com/api/v1/archives` in a
background worker at startup and offers Refresh, loading, empty and error states.
No API key or authorization header is sent. Metadata is trusted through this
fixed first-party HTTPS origin; it is not independently signed and has no
anti-rollback guarantee. The service must protect its metadata and publish new
artifact IDs when bytes change. Arbitrary remote catalogs cannot replace it.

The response has `archives: [...]` and `page: {next, nextUrl}`. Each record maps:

| Archive field | vKore meaning |
| --- | --- |
| `id`, `distro`, `version` | Stable ID and display label |
| `isa`, `variant`, `fileName` | Public x86_64 ISOs with headless/inference or desktop profiles are selectable |
| `requiresAuth` | Authenticated-only entries are omitted until authentication is supported |
| `url` | Same-origin `/dl/{id}/{fileName}` HTTPS artifact URL |
| `sha256`, `size` | Required full lowercase SHA-256 and exact nonzero byte count |
| `minimum` (optional) | CPU/RAM/disk requirements described below |
| `koreRevision`, `dagfsFormatVersion` (optional) | Validated provenance and supported volume format when supplied |

Missing provenance stays unknown; it is never populated from vKore's own
libkore pin. ISO inspection and launch preflight remain required. Extra service
fields (timestamps, BLAKE3, checksum sidecar URLs) are ignored. The inline SHA-256
is authoritative; no second checksum endpoint can substitute another identity.
Metadata redirects are refused. Pagination follows only same-origin
`/api/v1/archives` next URLs, at most eight pages, 64 unique archive IDs and
256 KiB in total. A malformed compatible entry, duplicate ID or failed page
rejects the complete refresh. An unsuccessful refresh preserves the displayed
selection; creation still requires a successful fresh catalog lookup.

The runtime resolves the selected ID again before download, and the desktop's
selected digest must still match. The UI cannot substitute a URL or omit
verification. Existing machines work without the service. The CLI also exposes
`vkore machine images` and `vkore machine create NAME --image ID`; explicit
`--iso PATH_OR_HTTPS_URL` imports remain available.

## Bundled v1 schema

The earlier bundled document parser remains available for release fixtures.
`crates/vkored/assets/official-images.json` is empty and is no longer the desktop's
source of images. Unlike the live service, this strict schema requires source
and format provenance.


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

1. Resolve the selected ID from the freshly fetched, validated catalog. Invalid
   ID syntax fails before network access; unknown or changed IDs fail before
   downloading or creating a machine. Reuse a cached `images/SHA256.iso` only
   after checking its regular-file type, exact length, ISO structure and digest.
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
   Cancel stops the import; retry starts a new temporary download. Partial-file
   resume is not implemented. The UI reports download and verification progress.
6. Create the machine atomically with the verified path and digest. Existing
   configurations and disks are not replaced. Boot preflight still re-inspects
   the ISO using its existing behavior; catalog admission is not a boot result.

Version, profile and source revision describe the selected artifact. Resource
sizes remain machine configuration; the catalog does not invent guest endpoints,
boot markers, model readiness or QEMU hardware requirements ahead of their owners.

### Resource minimums

An image may include `"minimum": {"cpus": 2, "memory_mib": 2048, "disk_mib": 1024}`.
Omitting it uses those baseline requirements. Entries may raise, never lower,
them (up to 128 CPUs, 1 TiB RAM and 4 TiB storage). The runtime persists and
enforces these minimums during import, configuration and launch. See
[hardware setup](hardware.md) for defaults, placement and platform support.
