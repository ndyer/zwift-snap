#!/usr/bin/env python3
# D-Bus scan client for tests/bluetooth.sh.
#
# bluezoo scans for peer adapters only while the client that started the
# discovery stays connected, and it reports a peer only when that peer is
# discoverable. bluetoothctl cannot do either here: its management socket
# needs privileges the workshop container does not have (it hangs instead of
# failing), so both adapters are driven directly over D-Bus.
#
# - the central adapter gets a plain discovery: this is the client whose
#   presence keeps bluezoo scanning
# - the peer adapter gets a discovery with the "Discoverable" filter, which
#   is how the mock makes an adapter act as a discoverable peer (writing the
#   adapter property directly is denied by the mock)
#
# The process stays connected for --timeout seconds; tests/bluetooth.sh runs
# it in the background and stops it once the check has passed.

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
        description="Keep a BlueZoo discovery running for a while")
    parser.add_argument(
        "central", metavar="CENTRAL",
        help="object path of the scanning adapter, e.g. /org/bluez/hci0")
    parser.add_argument(
        "peer", metavar="PEER",
        help="object path of the adapter to expose as a discoverable peer, e.g. /org/bluez/hci1")
    parser.add_argument(
        "--timeout", type=int, default=90,
        help="seconds to stay connected; default: %(default)s")
    args = parser.parse_args()

    bus = sdbus.sd_bus_open_system()

    central = Adapter.new_proxy("org.bluez", args.central, bus)
    await central.start_discovery()

    peer = Adapter.new_proxy("org.bluez", args.peer, bus)
    await peer.set_discovery_filter(
        {"Discoverable": ("b", True), "Transport": ("s", "bredr")})
    await peer.start_discovery()

    print("discovery started", flush=True)
    await asyncio.sleep(args.timeout)


asyncio.run(main())
