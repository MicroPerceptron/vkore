# vkored

`vkored` holds vKore's machines and runs them: a library (`vkored`) the
desktop links in-process, and the `vkore` command line over it.

- **State** (`State`): one directory, `$VKORE_STATE`, `$XDG_STATE_HOME/vkore` or
  `~/.local/state/vkore`, private to the user. A machine lives in
  `machines/NAME/`: `machine.json` (its configuration), `disk.raw` (its volume),
  `vars.fd` (its own copy of the UEFI variables), `usb.raw`, the console logs
  (`serial.log`, `debugcon.log`, `qemu.log`), the QMP socket while it runs, the
  staged project's `manifest.json`, and `runs/*.jsonl`, one transcript per boot.
- **Machines** (`MachineConfig`, `MachineStore`): the ISO, its last SHA-256,
  CPUs, memory, disk size, IOMMU vendor, acceleration, framebuffer, firmware,
  an optional project to stage, and whether to create the screenshots folder.
  Names are ASCII letters, digits, `-` and `_`; configurations are validated
  when written and when read.
- **Sessions** (`Session`): a boot from preflight to finish. `Session::start`
  probes the host and its QEMU, checks the ISO (and says when it changed since
  the last boot), finds firmware, makes sure terminal 0's
  `pictures/screenshots` exists, stages the project, and starts QEMU on Kore's
  gate topology through `vkore-vm`. `poll` reports the boot's stages from
  Kore's own console markers and QEMU's events; `stop` presses the power button
  and ends QEMU after the grace period; `finish` collects what the machine
  changed in its project and, if the project's policy is automatic, applies it
  within its file and byte bounds. Every step is an `Event`, written to the
  run's transcript and handed to the caller.

## The command line

```text
vkore probe                               what this host and its QEMU can boot
vkore iso ISO                             an ISO's SHA-256 and paging mode
vkore machine create NAME --iso ISO       a machine (see --help for its options)
vkore machine list | show NAME
vkore machine run NAME                    boot; Ctrl-C asks it to stop
vkore machine changes NAME                what the last boot changed in its project
vkore machine apply NAME                  write those changes back
vkore machine remove NAME
vkore volume DISK info | ls | tree | cat | get | put | rm | format | shots
```

`DISK` is a raw image, a block device (Linux), or `@NAME` for a machine's own
disk. `vkore machine run` takes `--display window|headless|vnc=SOCKET`,
`--until kernel|shell|ready`, `--timeout`, `--screenshot` (press PrintScreen at
`--until`), `--screendump PATH`, `--stop-after SECONDS`, `--grace SECONDS` and
`--json` (events as JSON lines). `vkore volume DISK shots` lists Kore's
captures newest first with their record; `--get DIR` copies them out.

A volume command refuses a disk a running machine holds; stop the machine
first.
