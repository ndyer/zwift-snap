#!/usr/bin/env python3
# D-Bus scan client for tests/bluetooth.sh and tests/pairing.sh.
#
# bluezoo scans for peer adapters only while the client that started the
# discovery stays connected. bluetoothctl cannot do that here: its management
# socket needs privileges the workshop container does not have (it hangs
# instead of failing), so the central adapter is driven directly over D-Bus.
#
# The discovery is LE only, like the one wine's advertisement watcher starts
# for Zwift, so the mock trainer (tests/bluetooth-trainer.py) is reported from
# its LE advertisement: name and service UUIDs. Note that bluezoo keeps one
# discovery per adapter: whichever client calls StopDiscovery stops it for all.
#
# The process stays connected for --timeout seconds; the tests run it in the
# background and stop it once their checks have passed.

import argparse
import asyncio

import sdbus
from sdbus import DbusInterfaceCommonAsync, dbus_method_async


class Adapter(DbusInterfaceCommonAsync, interface_name="org.bluez.Adapter1"):

    @dbus_method_async(input_signature="a{sv}", input_args_names=["filter"])
    async def set_discovery_filter(self, filter: dict) -> None:
        ...

    @dbus_method_async()
    async def start_discovery(self) -> None:
        ...


async def main() -> None:
    parser = argparse.ArgumentParser(
        description="Keep a BlueZoo LE discovery running for a while")
    parser.add_argument(
        "central", metavar="CENTRAL",
        help="object path of the scanning adapter, e.g. /org/bluez/hci0")
    parser.add_argument(
        "--timeout", type=int, default=90,
        help="seconds to stay connected; default: %(default)s")
    args = parser.parse_args()

    bus = sdbus.sd_bus_open_system()

    central = Adapter.new_proxy("org.bluez", args.central, bus)
    await central.set_discovery_filter({"Transport": ("s", "le")})
    await central.start_discovery()

    print("discovery started", flush=True)
    await asyncio.sleep(args.timeout)


asyncio.run(main())
