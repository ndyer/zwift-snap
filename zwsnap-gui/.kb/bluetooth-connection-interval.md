# Preface

The opt-in BLE connection-interval re-tuning in `zwsnap-gui/src/bt_tuning.rs`: why it
exists, how it is gated on the `bluetooth-control` plug, the delayed re-send that makes
the override stick, and what is deliberately not testable in CI. Read this before changing
`bt_tuning.rs`, the `zwift` app's `bluetooth-control` plug, or the environment variables it
reads.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Why the override exists

An LE peripheral can advertise a deliberately slow "Peripheral Preferred Connection
Parameters" characteristic (0x2a04) purely to save battery. Heart-rate belts are the common
case: they cap their update rate at ~1 Hz, which Zwift shows as choppy HR. BlueZ reads 0x2a04
during post-connect service discovery and, ~2 s after the link is established, issues
`MGMT_OP_LOAD_CONN_PARAM` (0x0035) to the kernel, which then performs an LE Connection Update
to the peripheral's slow preference.

The watcher counters that: when the privileged `bluetooth-control` plug is connected, it
watches the kernel's management socket for freshly connected LE devices and sends its own
0x0035 to re-tighten the interval to ~15 ms (2 s supervision timeout, zero latency). It is
**opt-in** because tightening the interval hurts battery life and because sending 0x0035 to
the kernel requires `CAP_NET_ADMIN`, which is exactly what `bluetooth-control` grants.

## Gating and lifecycle

- `bluetooth-control` is a snapd built-in plug with `deny-auto-connection: true`; the user
  opts in with `sudo snap connect zwift:bluetooth-control`. It is listed on the `zwift` app's
  own `plugs` in `snap/snapcraft.yaml`, *not* the shared `&plugs` anchor, so `wine` and
  `winecfg` do not get it.
- `bt_tuning::spawn` runs on its own thread for the whole launcher lifetime (started in
  `application.rs::launch` right after `logger.init()`), and stops via the shared
  `Cancellation` when the launcher exits.
- The first thing `watch` does is run `snapctl is-connected bluetooth-control`; anything but
  exit 0 is treated as "not connected" and the module logs one `info` line and returns. Any
  failure to open the socket is logged and ignored, so Zwift always still starts.
- A single `AF_BLUETOOTH`/`BTPROTO_HCI`/`HCI_CHANNEL_CONTROL` socket is opened once, bound to
  `HCI_DEV_NONE` (all controllers), and reused for both reading events and writing commands;
  an `FdGuard` closes it on every exit path. There is no need to subscribe an event mask: the
  default mask delivers all events and the loop filters on opcode in-process.

## The timing fix (delayed re-send)

Sending 0x0035 on the connect event alone is too early: BlueZ reads 0x2a04 after our request
and issues its own 0x0035, overwriting ours back to the slow interval. So `on_connected`
sends immediately **and** pushes a `Pending` entry that `fire_due_reesends` re-sends after
`Params::resend` (default 4 s) has elapsed, landing the override *after* BlueZ and making it
stick. `MGMT_EV_DEVICE_DISCONNECTED` prunes the pending entry so a disconnected device is not
re-sent to.

## Environment variables

Resolved once from the environment in `Params::from_env`, defaulting to the fast-HR profile:

- `ZWIFT_BT_INTERVAL` — min/max interval in 1.25 ms units (default `12` = 15 ms).
- `ZWIFT_BT_TIMEOUT` — supervision timeout in 10 ms units (default `200` = 2 s).
- `ZWIFT_BT_RESEND_MS` — re-send delay in milliseconds (default `4000`).

## Wire format

`build_load_conn_param` encodes `MGMT_OP_LOAD_CONN_PARAM` (0x0035) as a 23-byte command: the
6-byte `mgmt_hdr` (`__le16 opcode, index, len`), then `param_count=1` and one
`mgmt_conn_param` (7-byte `mgmt_addr_info` address + type, then `__le16 min_interval,
max_interval, latency, timeout`), with min=max=interval and latency=0. The address is passed
through in on-the-wire byte order. All layouts match `include/net/bluetooth/mgmt.h`.

## Testing

The encoder, address wire-order reversal, interval/timeout scaling, defaults, and the
BR/EDR rejection path (via the pure `le_device_addr` helper) are unit-tested in
`bt_tuning.rs`. There is **no** CI plumbing test for the live override: bluezoo (the
`tests/bluetooth.sh` mock) only emulates the `org.bluez` D-Bus interface, not the raw kernel
management socket that `bluetooth-control` unlocks, and the workshop's own `hci0` is
rfkill-blocked, so `btmgmt`/`hcitool` cannot open `mgmt_socket` there either. The only
exercisable path in CI is "not connected → no-op", which the gate covers by construction.
End-to-end verification needs real BLE hardware plus `sudo snap connect zwift:bluetooth-control`.

## Related knowledge

The `graphics-profile.md` note about "not adopted from bpauli/zwift-bootstrap" (ANT+ and the
30–50 ms BLE interval being impossible under strict confinement) is the origin of this
feature: `bluetooth-control` is the one interface that *does* make the interval change
possible, so it is the exception to that earlier conclusion.
