#!/opt/bluezoo/bin/python3
"""Run the bluezoo BlueZ mock with a controllable signal strength for LE devices.

bluezoo reports RSSI 0 for every device and never updates it, so a scanner sees each LE
peer exactly once: winebth only turns a Device1 change into a new advertisement when the
device has an RSSI and an advertisement property (RSSI included) changes. Real peripherals
advertise many times a second at a negative RSSI, and Zwift expects that, so this wrapper
gives every LE device a plausible RSSI and, depending on BLUEZOO_RSSI_MODE, either nudges
it on every bluezoo scan round (--scan-interval) or holds it still:

  jitter (default)  base +/- 1..2 dBm each round, so the device looks like it keeps
                    advertising; this is what tests/bluetooth.sh and tests/pairing.sh rely on.
  stable            set once and never changed, so nothing but winebth's own reporting can
                    keep re-reporting the device -- the "low signal strength" reproduction
                    (see tests/.kb/pairing-test.md).
  drift             move from the base towards BLUEZOO_RSSI_TARGET by BLUEZOO_RSSI_STEP
                    each round, for a slow strong-to-weak sweep.

Environment (all optional, defaults in brackets):
  BLUEZOO_RSSI_BASE    starting RSSI in dBm                 [-55]
  BLUEZOO_RSSI_MODE    jitter | stable | drift              [jitter]
  BLUEZOO_RSSI_TARGET  end RSSI for drift                   [the base]
  BLUEZOO_RSSI_STEP    dBm per round for drift              [1]

Arguments are those of bluezoo itself.
"""

import os
import random

from bluezoo import bluezoo
from bluezoo.device import Device

BASE_RSSI = int(os.environ.get("BLUEZOO_RSSI_BASE", "-55"))
RSSI_MODE = os.environ.get("BLUEZOO_RSSI_MODE", "jitter")
RSSI_TARGET = int(os.environ.get("BLUEZOO_RSSI_TARGET", str(BASE_RSSI)))
RSSI_STEP = abs(int(os.environ.get("BLUEZOO_RSSI_STEP", "1"))) or 1

if RSSI_MODE not in ("jitter", "stable", "drift"):
    raise SystemExit(f"unknown BLUEZOO_RSSI_MODE {RSSI_MODE!r} (jitter|stable|drift)")

original_init = Device.__init__
original_properties_sync = Device.properties_sync


def init(self, *args, **kwargs):
    original_init(self, *args, **kwargs)
    self.rssi = BASE_RSSI


def next_rssi(current):
    """The RSSI this round's PropertiesChanged should carry (equal to current to skip)."""
    if RSSI_MODE == "stable":
        return current
    if RSSI_MODE == "drift":
        if current < RSSI_TARGET:
            return min(current + RSSI_STEP, RSSI_TARGET)
        if current > RSSI_TARGET:
            return max(current - RSSI_STEP, RSSI_TARGET)
        return current
    return BASE_RSSI + random.choice([-2, -1, 1, 2])


async def properties_sync(self, device):
    await original_properties_sync(self, device)
    if not getattr(self, "is_le", False):
        return
    rssi = next_rssi(self.rssi)
    if rssi == self.rssi:
        return
    self.rssi = rssi
    # RSSI is read-only in bluezoo, so emit the change signal by hand like sdbus's setter does
    prop = self.RSSI.dbus_property
    self.properties_changed.emit(
        (prop.interface_name, {prop.property_name: (prop.property_signature, rssi)}, []))


Device.__init__ = init
Device.properties_sync = properties_sync

if __name__ == "__main__":
    bluezoo.main()
