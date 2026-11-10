# Preface

The BLE behaviour the snap depends on lives in a wine fork, not upstream wine. Read
this before changing `winebth.c` or reasoning about why a trainer connects but never
answers — the failure modes recorded here cost real time to find.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Bluetooth discovery vs. connect (wine fork)

BlueZ drops LE connections while a discovery is running, so `winebth.c` in the
`zwift-radios-rebased` wine branch stops LE discovery before it connects a device and starts
it again once the connection is up. The state is per radio (`le_discovery_refs`,
`discovery_paused`, guarded by `discovery_cs`) and driven by the `connecting` flags of the
radio's devices, so a device destroyed mid-connect cannot leak a pause. Discovery is paused
*before* `props_cs` is taken, because stopping it blocks on the event loop thread, which needs
`props_cs`. `tests/pairing.sh` asserts the pause/resume lines and the shutdown disconnect from
the `+winebth` trace.

## Disconnect on teardown

The disconnect-on-teardown runs in `remote_device_pdo_pnp`'s `IRP_MN_REMOVE_DEVICE`, not in the
FDO's: the PnP manager removes a device's children *before* the device itself, so the FDO's own
removal pass runs after `remote_device_pdo_pnp`'s `IRP_MN_SURPRISE_REMOVAL` has already
`list_remove()`d every device from the FDO's list. A disconnect loop there finds an empty list,
which is what it did: the trace showed the whole PnP teardown and no `Disconnecting` line. The
FDO loop is kept as a fallback for devices whose PDO was never created. A disconnect on a
device BlueZ already dropped is harmless — it fails with "Not connected".

That code is only reachable through a *clean* stop of the winedevice service group, and no
ordinary shutdown gets there:

- `wineserver -k` SIGKILLs every process, so nothing runs at all.
- wineserver's graceful path only sets `shutdown_event`; no client consumes it
  (`make_process_system` has no caller in the tree).
- `ZwUnloadDriver` refuses while devices still exist.
- Root PnP devices are removed only by `pnp_manager_stop_driver()`, called from the `done:`
  path of `wine_ntoskrnl_main_loop` — i.e. when `winedevice`'s `stop_event` is set by a plain
  `SERVICE_CONTROL_STOP`/`SERVICE_CONTROL_SHUTDOWN` delivered to a winedevice group service.

So a killed prefix does leave the peer advertising the connection. `tests/pairing.sh`'s
`stop_game` therefore stops the group explicitly (`wine sc stop Winedevice<N>` for the group
services that exist) and only then kills the prefix; the services are synthesised per
load-order group as `Winedevice<N>` (winebth's group is `WinePlugPlay`), and `<N>` depends on
driver start order and is not stable, so it stops every candidate. Verified end to end: the
trace goes `remote_device_pdo_pnp … IRP_MN_REMOVE_DEVICE` → `Disconnecting <device> on device
removal` → `bluez_device_disconnect` on `/org/bluez/hci0/dev_…` → bluezoo
`Disconnecting device[00:00:00:22:22:22]`.

## Tracing the driver

Driver debug output inherits the stderr of whoever started `winedevice`, so redirecting the
`wineboot` that launches it to `/dev/null` throws away every `winebth` trace. Use `-v ":trace:"`
to grep a trace: a `^[0-9a-f]{4}:trace` pattern hides the non-trace lines such as
`err:ntoskrnl:ZwUnloadDriver`. A keep-alive user process (`wine cmd /c "ping -n 400
127.0.0.1" &`) keeps the wineserver from exiting between experiments and resetting the PIDs
in the log, and `sc query` enumeration prints nothing in wine — read the `Group` values from
the prefix registry instead.

## Getting a rebuilt driver into a prefix

A rebuilt `winebth.sys` reaches an existing prefix by itself: the driver binary is copied into
`drive_c/windows/system32/drivers/` at driver install time, and `wineboot` re-runs that install
(`install_root_pnp_devices`) whenever `wine.inf` is newer than the prefix's
`.update-timestamp` — which a rebuilt snap always is. Confirm with `strings
…/system32/drivers/winebth.sys` before wondering why a fix did not take; the stale copy is
what a fix appears to do nothing against.

## The D-Bus reply-wait trap

Waiting for a D-Bus reply in that driver is a trap. `bluez_dbus_pending_call_wait` used to
`set_notify` + `sem_wait`, leaving the dispatch to `bluez_dbus_loop` — but the LE-discovery
resume runs *on* that loop's thread (after `CONNECT_FINISHED`), so nothing could ever
dispatch the reply. The loop deadlocked, the pause was never undone, and every later GATT
request — Zwift's model-number read after connecting the trainer — stayed pending forever,
which looks from outside like a trainer that connected but never answers. It now uses
`p_dbus_pending_call_block`, which pumps the connection in the calling thread and is still
bounded by the pending call's timeout. There is exactly one dispatcher (the
`p_dbus_connection_read_write_dispatch` call in `bluez_dbus_loop`); keep it that way when
touching the wait, and remember that a blocked event-loop thread stops BLE event delivery to
wine entirely.
