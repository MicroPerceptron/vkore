# Versioning vKore

vKore follows [Semantic Versioning 2.0.0](https://semver.org/). The public surface
includes the CLI, local HTTP API, persisted machine/settings/catalog schemas,
and exported Rust APIs. Guest protocols and DagFS remain owned by Kore/libkore;
the app version is not a guest protocol, image, QEMU, or filesystem version.

## One version

`workspace.package.version` in the root `Cargo.toml` is the source of truth.
Every vKore package inherits it with `version.workspace = true`. The desktop's
sidebar and the CLI obtain it from Cargo at compile time. Update `Cargo.lock`
with Cargo after a bump. The `iced-renderer-compat` patch retains its upstream
Iced package version and is deliberately excluded from the vKore release train.

This repository currently uses normal `MAJOR.MINOR.PATCH` releases. Pre-release
and build suffixes are valid SemVer, but are not enabled by our release policy.
Version 0.x signals an evolving public API. It does not promise 1.0 stability.

The desktop shows `v<version>-<short commit>` at the bottom of the sidebar.
Tracked source modifications append `-dirty`; untracked local files do not.
The build script watches Git HEAD, refs, index and tracked files, including linked
worktrees, so a new commit rebuilds the label even when source content is unchanged.
Source archives show `-unknown` unless the builder provides the full hexadecimal
`VKORE_BUILD_COMMIT`; in a checkout this must match HEAD. This label is diagnostic
metadata, not a change to the package's semantic version or release tag.

## Every product PR

Compare against the current target branch, add a new top entry in
`CHANGELOG.md`, and bump the shared version exactly once for the whole PR:

- `### Added`: new functionality increments MINOR and resets PATCH.
- `### Breaking`: incompatible changes increment MAJOR, resetting MINOR/PATCH.
  During 0.x development, they increment MINOR instead.
- `### Fixed`, `### Security`, or `### Changed`: compatible fixes, hardening,
  refactoring and polish increment PATCH when there is no higher-impact category.

Use the highest-impact category in the PR. Reviewers must check that the notes
accurately classify the behavioral/API change; CI cannot infer compatibility
from arbitrary source code. Multiple commits within one PR share its version.
If another product PR lands first, rebase and update the version/notes again.
Documentation-only and CI-only changes may retain the current version.

Run `python3 tools/check-version.py --base origin/main` after committing, or
omit `--base` to check manifests, lockfile and notes while editing. The base
comparison uses committed Git paths, just as CI does.

## CI and releases

The **Version policy** workflow checks every PR and main-branch push, without
fetching private desktop dependencies. It rejects inconsistent package/lockfile
versions, invalid version syntax, missing notes, unchanged versions on product
changes, regressions, and bumps inconsistent with the declared changelog
categories. Product paths include crates, assets, runtime packaging, Cargo
manifests/lockfile, the QEMU builder, and the Rust toolchain pin.

Tags use `vX.Y.Z`; tag CI verifies exact agreement with the manifest and
changelog. Create a tag only on a reviewed, validated commit. Never move a
published release tag or replace its contents: ship a new version instead.
This workflow validates versions; it does not publish releases or edit source.
The **Semantic versioning** job is the required status check for this policy.
Keep it required in the main branch protection settings; those settings are
managed separately from the workflow file.
