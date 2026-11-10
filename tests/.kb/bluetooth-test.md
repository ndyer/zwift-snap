# Preface

The Bluetooth plumbing test and the bluezoo mock it depends on, including every
gotcha that had to be discovered live. Read this before changing `tests/bluetooth.sh`,
`tests/tools/bluetooth/`, or anything that starts bluezoo.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What the test checks

`tests/bluetooth.sh` checks the Bluetooth plumbing of the snap, not the game: wine's
Bluetooth driver (`winebth`, hosted by winedevice.exe) must reach BlueZ over the snap's
`bluez` plug, see the adapters, register its authentication agent and learn about a
discovered BLE smart trainer. It asserts only on the winebth trace (no GATT traffic).

## Why bluezoo

The workshop has no Bluetooth hardware, and bluetoothd's management socket needs
privileges the container does not have (`bluetoothctl scan` hangs on `mgmt_socket`),
so the test uses [bluezoo](https://github.com/Samsung/BlueZoo), a user-space BlueZ
D-Bus mock, in place of bluetoothd. The `setup-base` SDK hook installs `bluez`
(bluetoothctl, the D-Bus policy), a `/opt/bluezoo` venv (PyPI, not cached by pkgproxy)
and masks `bluetooth.service`; the test starts bluezoo itself, runs `zwift.wine` in a
throwaway prefix (`~/snap/zwift/common/bt-test`) with `WINEDEBUG=+winebth` and asserts
on the trace. Artifacts on failure: `~/zwift-bt-out/`.

## Gotchas that cost time to discover (all verified live)

- bluezoo must run as root (only root may own `org.bluez` per the bluez D-Bus policy),
  detached with `setsid` — a cancelled caller takes its process group down otherwise.
  The bluez package's D-Bus activation stub (`Exec=/bin/false`) must be removed, or the
  first call to the unowned `org.bluez` name fails with a confusing systemd error.
- bluezoo has no device emulation of its own: a device is another mock adapter. bluezoo
  runs two adapters, hci0 (the PC, `00:00:00:11:11:11`) and hci1 (the trainer,
  `00:00:00:22:22:22`). `tests/tools/bluetooth/bluetooth-trainer.py` makes hci1 a BLE trainer named
  "KICKR": an LE advertisement (FTMS 0x1826 + Cycling Power 0x1818, discoverable) and a
  GATT application with FTMS, Cycling Power and Device Information; it streams 150 W /
  85 rpm while notifications are on and logs every read/write/notify to `trainer.log`.
  bluezoo links the GATT application to the central's Device1 on Connect.
- bluezoo only scans while the client that started the discovery stays connected (and any
  client's StopDiscovery stops it for all). `bluetoothctl` cannot do that here (its
  management socket hangs), so `tests/tools/bluetooth/bluetooth-scan.py` keeps an LE-only discovery open on
  hci0 over plain D-Bus.
- bluezoo reports RSSI 0 and never changes it, but winebth re-reports an advertisement to
  the BLE watcher only when an LE property changes with an RSSI present — so Zwift saw
  KICKR once and dropped it. `tests/tools/bluetooth/bluezoo-run.py` wraps bluezoo, sets RSSI to -55 and
  jitters it on every scan round (one PropertiesChanged per second).
- winebth would add the trainer's own adapter as a second radio, and wine's watcher uses
  the first radio. `ROUVY_BT_ADAPTER=hci0` (a hci name or address, read in winebth's
  `dbus.c`) makes winebth use only hci0 and log "ignoring Bluetooth adapter hci1"; both
  tests export it.
- The snap app remaps `$HOME` to its private `~/snap/zwift/x1`, so a prefix in
  `~/snap/zwift/common` is `$HOME/../common/...` from inside the snap.
- Everything under `/tmp` inside the snap is private to each command and discarded —
  a log file written there is gone before it can be read.

The bluezoo setup (starting the mock as root, the trainer, the scan client, the prerequisites) lives in
`tests/tools/bluetooth/bluezoo.sh`, shared by `tests/bluetooth.sh` and the pairing test.
