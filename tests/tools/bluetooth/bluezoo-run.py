#!/opt/bluezoo/bin/python3
"""Run the bluezoo BlueZ mock with a live signal strength for LE devices.

bluezoo reports RSSI 0 for every device and never updates it, so a scanner sees each LE
peer exactly once: winebth only turns a Device1 change into a new advertisement when the
device has an RSSI and an advertisement property (RSSI included) changes. Real peripherals
advertise many times a second at a negative RSSI, and Zwift expects that, so this wrapper
gives every LE device a plausible RSSI and nudges it on every bluezoo scan round
(--scan-interval), which makes the device look like it keeps advertising.

Arguments are those of bluezoo itself.
"""

import random

from bluezoo import bluezoo
from bluezoo.device import Device

BASE_RSSI = -55

original_init = Device.__init__
original_properties_sync = Device.properties_sync


def init(self, *args, **kwargs):
    original_init(self, *args, **kwargs)
    self.rssi = BASE_RSSI


async def properties_sync(self, device):
    await original_properties_sync(self, device)
    if not getattr(self, "is_le", False):
        return
    rssi = BASE_RSSI + random.choice([-2, -1, 1, 2])
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
