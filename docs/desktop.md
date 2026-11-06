# The desktop

`vkore-ui` is vKore's desktop: Kore machines, and their volumes browsed from the
host through Kore's own engine. It runs in-process (no daemon, no browser, no
login) and shares its state with the `vkore` command line.

```sh
vkore-ui [--state DIR] [--open DISK|@MACHINE]
```

`--open` lands on that disk's screenshots; `--state` is the same directory the
command line uses.

## Volumes

Open a disk by typing it (a raw image, a block device on Linux, or `@NAME`) or
with one of the machine buttons under the field. The header shows what is open
and the volume's generation, which grows with every publication Kore or vKore
makes.

- **Places.** Terminal 0's and terminal 1's homes, and the volume root. A home
  nothing has written to yet says so; the machine creates it on its first boot.
  The breadcrumb walks back up.
- **Preview.** Selecting a file reads its head, at most 32 KiB, and never
  decodes a pixel. A screenshot shows its size and Kore's capture record: the
  kernel build that drew it, when, the mode (screen, region or window), the
  app and region, density, uptime, and any other `kore:` keys. Text shows its
  first 32 KiB with its line breaks; anything else shows its length and first
  bytes.
- **Open** copies the file to a private directory (`$XDG_RUNTIME_DIR`, mode
  0700) and hands it to the host's viewer (`xdg-open`, or `open` on macOS).
- **Save** copies the selected file, or the directory you are in, into the host
  folder in the field (your Downloads folder by default), creating the folder
  if it does not exist. It never replaces a file there.
- **Import** installs a host file or directory into the directory you are in,
  inside a terminal's home. It is one publication: the volume has all of it
  or none of it.
- **Remove** deletes the selected entry, after asking a second time.

A disk a running machine holds cannot be opened; stop the machine first. While
the browser has a disk open it holds the disk's lock, and another vKore cannot
open it; a machine started from this window closes the browser first.

## Machines

The list shows each machine and where its boot is: starting, kernel up,
services, shell ready, ready, degraded or failed. Select one to see its ISO and
the SHA-256 it last booted, its resources, and the project it stages.

- **New machine** takes a name, an ISO and optionally a project folder to stage
  into terminal 0's home.
- **Start in a window** gives the guest QEMU's own window, where you use Kore
  directly and press PrintScreen to capture. **Start headless** runs it without
  one.
- **Stop** presses the power button, and Kore shuts itself down. A Kore from
  before `7074bda2` on Kore's main (v0.40.0 among them) does not hear the button
  under QEMU, so after 10 s vKore ends QEMU and says so. Either way the volume
  keeps its last committed state.

The pane under the controls follows the boot as it happens and keeps the last
run's record after the machine stops. Starting a machine whose disk the
Volumes page holds closes the browser first, and opens it again when the
machine stops.

## Screenshots, end to end

1. Start the machine in a window and press PrintScreen in Kore. Kore saves the
   capture into `pictures/screenshots` in terminal 0's home, a folder vKore
   creates before each boot because Kore's capture app saves only into one
   that exists.
2. Stop the machine once Kore has saved the capture: it writes it a few seconds
   after the press (up to ten under TCG), and a stop does not wait for one
   still being written. The Volumes page reopens the disk on the screenshots,
   newest last by name (names are UTC times).
3. Select a capture to read its record; **Save** it, or **Open** it in your
   image viewer.

The command line does the same: `vkore volume @NAME shots` lists them with
their record and `--get DIR` copies them out.

## Text from a guest

File names, logs and capture records come from the guest, and host paths come
from the host. The desktop shows them as they are, not as they would like to
look:

- a character the face cannot draw shows as its escape, `\u{…}`, rather than
  a blank box or a dropped frame;
- controls, direction overrides, zero-width characters and look-alike spaces
  show as escapes too, so `\u{202e}gnp.exe` cannot pass for `exe.png`;
- a backslash shows doubled, so an escape cannot be forged.

Logs keep their line breaks and tabs. Only the faces the desktop bundles
(Archivo, Space Grotesk, JetBrains Mono, and Axiom's DM Sans) are used, so it
draws the same on every host whatever fonts the host has installed.

## Limits

- Images are previewed by their record only; the renderer does not draw
  bitmaps yet. **Open** shows them in the host's viewer.
- The desktop needs Axiom, which is not public yet.
- Only x86_64 Linux has been run (under Xvfb, with QEMU's TCG).
