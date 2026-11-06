# vKore

vKore is the open-source host side of Kore: it boots Kore ISOs under QEMU and
reads and writes Kore's volumes through Kore's own engine. It speaks for Kore in
public, so it holds Kore's standards: good, secure, efficient and fast, all
four. Missing one is missing the goal.

## Where things belong

- Kore reaches vKore only through libkore (`kore-host`), pinned by one
  revision. Never copy Kore code, protocol types or on-disk formats here, and
  never parse or write DagFS bytes outside the engine. A behaviour vKore needs
  from the engine is changed in Kore's `tools/kore-vol` (or its owner),
  exported with libkore's `tools/sync-native.py`, and arrives by bumping the
  pin. [docs/compatibility.md](docs/compatibility.md) records the identity and
  how to move it.
- Kore's console markers, QMP and volumes are the whole guest contract today.
  A host-guest channel comes the same way as the engine did (Kore owns it,
  libkore exports it), never ahead of Kore. See [MIGRATION.md](MIGRATION.md).
- `vkore-vm` owns QEMU; `vkore-volume` owns volumes; `vkored` owns machines and
  sessions; `vkore-ui` and the CLI are views over them and hold no policy of
  their own.

## How to decide

- Precedent is evidence, not authority. What another hypervisor front end,
  file browser or Linux does is data about what it cost; reason from what Kore
  and the host actually offer.
- Take proven work as a dependency before writing it, and vet it first:
  licence, maintenance, `unsafe`, allocation behaviour, what it pulls in. The
  choices so far, and the ones rejected, are in the commit history (cap-std,
  png, sha2, clap, ctrlc in; three ISO crates out, for a bounded Joliet reader).
- Probe, don't guess: QEMU properties, firmware, acceleration and the ISO's
  paging mode are read from the host and the medium. Refuse what cannot work,
  before it starts, in words; never degrade silently.
- One narrow goal at a time, finished before the next. A real Kore boot is the
  acceptance: unit tests, fakes and argument are provisional until a machine
  has booted the ISO and the round trip has been seen.

## Rules the code keeps

- A disk has one owner: no volume opens under a running machine, and one opener
  per disk per process (`vkore-volume`'s claim). A long-lived reader must not
  leak engine handles; a session has 32.
- Everything is bounded: reads (a preview reads a file's head), decoders,
  staging and write-back (files and bytes), QMP lines, transcripts. Nothing
  unbounded comes from a guest or a disk.
- Exports never follow a symlink, never overwrite unasked, and publish only a
  synced file.
- QMP commands carry ids; a late reply is dropped, never taken for the next one.
- In the desktop, text from outside the binary goes through `widgets::data` (or
  `fonts::shown`): it shapes in one registered face, escapes what that face
  cannot draw, and shows controls and invisible characters. Axiom confines
  shaping to the faces it can draw -- its DM Sans and the faces
  `fonts::register` registers -- so a host font can never stand in.
  The render tests (no text refusals, shared kernels equal to the scalar
  reference) stay green.

## Gates

```sh
cargo fmt --all -- --check
```

```sh
cargo clippy --workspace --all-targets -- -D warnings
```

```sh
cargo test --workspace
```

A change to `vkore-vm`, `vkored`'s sessions or the libkore pin also runs the
opt-in boot test with a real ISO (`VKORE_TEST_QEMU`, `VKORE_TEST_ISO`), and a
change to the desktop is looked at, under Xvfb if there is no display. Say what
was run and what was not; emulator results are not hardware results.

Commit messages say what changed and why, in the present tense, and carry the
attribution lines the session gives.
