#!/usr/bin/env python3
# Mock BLE smart trainer for tests/bluetooth.sh and tests/pairing.sh.
#
# bluezoo has no API to create a remote device: a device is another mock adapter seen from
# the scanning one. This peripheral turns a spare bluezoo adapter (hci1) into a trainer by
# registering, over plain BlueZ D-Bus, an LE advertisement (LEAdvertisingManager1) and a
# GATT application (GattManager1). bluezoo then reports it to a scanning adapter (hci0) as
# an LE device with the advertised name and service UUIDs, and links the GATT services
# when that side connects.
#
# Services, modelled on a Wahoo KICKR:
#   Fitness Machine (FTMS, 0x1826), Cycling Power (0x1818), Device Information (0x180A).
# While a client has notifications on, fake ride data (~150 W, ~85 rpm) is sent every
# second. Every read, write and notification start/stop is logged to stdout, which the
# tests assert on. Runs in the bluezoo venv (it needs sdbus) until it is killed.

import argparse
import asyncio
import struct
import sys
import time
from typing import Any, Callable, Optional

import sdbus
from sdbus import (DbusInterfaceCommonAsync, DbusObjectManagerInterfaceAsync,
                   dbus_method_async, dbus_property_async)

APP_PATH = "/org/zwiftsnap/trainer"
ADV_PATH = f"{APP_PATH}/advertisement0"


def uuid16(short: int) -> str:
    return f"0000{short:04x}-0000-1000-8000-00805f9b34fb"


FTMS = uuid16(0x1826)
CYCLING_POWER = uuid16(0x1818)
DEVICE_INFORMATION = uuid16(0x180A)
# The GAP appearance of a cycling power sensor.
APPEARANCE_CYCLING_POWER_SENSOR = 0x0484


def log(message: str) -> None:
    print(f"trainer: {message}", flush=True)


class BluezGattManager(DbusInterfaceCommonAsync, interface_name="org.bluez.GattManager1"):
    @dbus_method_async(input_signature="oa{sv}")
    async def RegisterApplication(self, application: str,
                                  options: dict[str, tuple[str, Any]]) -> None:
        raise NotImplementedError


class BluezAdvertisingManager(DbusInterfaceCommonAsync,
                              interface_name="org.bluez.LEAdvertisingManager1"):
    @dbus_method_async(input_signature="oa{sv}")
    async def RegisterAdvertisement(self, advertisement: str,
                                    options: dict[str, tuple[str, Any]]) -> None:
        raise NotImplementedError


class Advertisement(DbusInterfaceCommonAsync, interface_name="org.bluez.LEAdvertisement1"):
    def __init__(self, name: str, uuids: list[str]):
        super().__init__()
        self.name = name
        self.uuids = uuids

    @dbus_method_async()
    async def Release(self) -> None:
        log("advertisement released")

    @dbus_property_async(property_signature="s")
    def Type(self) -> str:
        return "peripheral"

    @dbus_property_async(property_signature="as")
    def ServiceUUIDs(self) -> list[str]:
        return self.uuids

    @dbus_property_async(property_signature="s")
    def LocalName(self) -> str:
        return self.name

    @dbus_property_async(property_signature="q")
    def Appearance(self) -> int:
        return APPEARANCE_CYCLING_POWER_SENSOR

    @dbus_property_async(property_signature="b")
    def Discoverable(self) -> bool:
        return True


class Service(DbusInterfaceCommonAsync, interface_name="org.bluez.GattService1"):
    def __init__(self, uuid: str):
        super().__init__()
        self.uuid = uuid

    @dbus_property_async(property_signature="s")
    def UUID(self) -> str:
        return self.uuid

    @dbus_property_async(property_signature="b")
    def Primary(self) -> bool:
        return True


class Characteristic(DbusInterfaceCommonAsync, interface_name="org.bluez.GattCharacteristic1"):
    """A characteristic with a static or callable value. Notifications are sent by setting
    Value, which emits PropertiesChanged; bluezoo forwards that to the connected client and
    calls Confirm for each value, also for plain notifications."""

    def __init__(self, uuid: str, service_path: str, flags: list[str], name: str,
                 value: bytes = b"",
                 on_write: Optional[Callable[["Characteristic", bytes], None]] = None):
        super().__init__()
        self.uuid = uuid
        self.service_path = service_path
        self.flags = flags
        self.name = name
        self.value = value
        self.on_write = on_write
        self.notifying = False

    @dbus_method_async(input_signature="a{sv}", result_signature="ay")
    async def ReadValue(self, options: dict[str, tuple[str, Any]]) -> bytes:
        log(f"read {self.name} -> {self.value.hex()}")
        return self.value

    @dbus_method_async(input_signature="aya{sv}")
    async def WriteValue(self, value: bytes, options: dict[str, tuple[str, Any]]) -> None:
        log(f"write {self.name} <- {bytes(value).hex()}")
        if self.on_write is not None:
            self.on_write(self, bytes(value))

    @dbus_method_async()
    async def StartNotify(self) -> None:
        log(f"start notify {self.name}")
        self.notifying = True

    @dbus_method_async()
    async def StopNotify(self) -> None:
        log(f"stop notify {self.name}")
        self.notifying = False

    @dbus_method_async()
    async def Confirm(self) -> None:
        pass

    @dbus_property_async(property_signature="s")
    def UUID(self) -> str:
        return self.uuid

    @dbus_property_async(property_signature="o")
    def Service(self) -> str:
        return self.service_path

    @dbus_property_async(property_signature="as")
    def Flags(self) -> list[str]:
        return self.flags

    @dbus_property_async(property_signature="b")
    def Notifying(self) -> bool:
        return self.notifying

    @dbus_property_async(property_signature="ay", flags=sdbus.DbusPropertyEmitsChangeFlag)
    def Value(self) -> bytes:
        return self.value

    @Value.setter_private
    def _value_setter(self, value: bytes) -> None:
        self.value = value

    async def notify(self, value: bytes) -> None:
        await self.Value.set_async(value)


class Trainer:
    def __init__(self, bus: sdbus.SdBus, name: str, power: int, cadence: int):
        self.bus = bus
        self.name = name
        self.power = power
        self.cadence = cadence
        self.manager = DbusObjectManagerInterfaceAsync()
        self.characteristics: list[Characteristic] = []
        self.crank_revolutions = 0
        self.crank_event_time = 0.0
        self.pending_indications: list[tuple[Characteristic, bytes]] = []

    def add_service(self, index: int, uuid: str,
                    characteristics: list[tuple[int, list[str], str, bytes]],
                    on_write: Optional[Callable[[Characteristic, bytes], None]] = None) -> None:
        service_path = f"{APP_PATH}/service{index}"
        self.manager.export_with_manager(service_path, Service(uuid), self.bus)
        for char_index, (short, flags, name, value) in enumerate(characteristics):
            char = Characteristic(uuid16(short), service_path, flags, name, value, on_write)
            self.manager.export_with_manager(f"{service_path}/char{char_index}", char, self.bus)
            self.characteristics.append(char)

    def characteristic(self, name: str) -> Characteristic:
        return next(c for c in self.characteristics if c.name == name)

    def on_control_point(self, char: Characteristic, value: bytes) -> None:
        # Every FTMS control point request (request control, reset, set target power,
        # set simulation parameters, start/resume ...) gets an indication saying it
        # succeeded: response code 0x80, the request op code, result 0x01 (success).
        if not value:
            return
        response = bytes((0x80, value[0], 0x01))
        self.pending_indications.append((char, response))

    def export(self) -> None:
        self.manager.export_to_dbus(APP_PATH, self.bus)
        self.add_service(0, FTMS, [
            # Fitness Machine Features: cadence + power measurement supported; target
            # settings: power target and indoor bike simulation supported.
            (0x2ACC, ["read"], "ftms-feature",
             struct.pack("<II", (1 << 1) | (1 << 14), (1 << 3) | (1 << 13))),
            (0x2AD2, ["notify"], "indoor-bike-data", b""),
            (0x2AD9, ["write", "indicate"], "ftms-control-point", b""),
            (0x2ADA, ["notify"], "ftms-status", b""),
            # Supported Power Range: 0..2000 W in 1 W steps.
            (0x2AD8, ["read"], "supported-power-range", struct.pack("<hhH", 0, 2000, 1)),
        ], on_write=self.on_control_point)
        self.add_service(1, CYCLING_POWER, [
            (0x2A63, ["notify"], "cycling-power-measurement", b""),
            # Cycling Power Feature: crank revolution data supported.
            (0x2A65, ["read"], "cycling-power-feature", struct.pack("<I", 1 << 3)),
            # Sensor Location: rear hub.
            (0x2A5D, ["read"], "sensor-location", bytes((13,))),
        ])
        self.add_service(2, DEVICE_INFORMATION, [
            (0x2A29, ["read"], "manufacturer-name", b"Wahoo Fitness"),
            (0x2A24, ["read"], "model-number", self.name.encode()),
            (0x2A25, ["read"], "serial-number", b"ZWIFTSNAP0001"),
            (0x2A26, ["read"], "firmware-revision", b"4.2.0"),
        ])
        self.advertisement = Advertisement(self.name, [FTMS, CYCLING_POWER])
        self.advertisement.export_to_dbus(ADV_PATH, self.bus)

    async def register(self, adapter: str) -> None:
        gatt = BluezGattManager.new_proxy("org.bluez", adapter, self.bus)
        await gatt.RegisterApplication(APP_PATH, {})
        log(f"GATT application registered on {adapter}")
        advertising = BluezAdvertisingManager.new_proxy("org.bluez", adapter, self.bus)
        await advertising.RegisterAdvertisement(ADV_PATH, {})
        log(f"advertising as {self.name} on {adapter}")

    async def ride(self) -> None:
        bike_data = self.characteristic("indoor-bike-data")
        power_measurement = self.characteristic("cycling-power-measurement")
        start = time.monotonic()
        while True:
            await asyncio.sleep(1)
            while self.pending_indications:
                char, response = self.pending_indications.pop(0)
                log(f"indicate {char.name} -> {response.hex()}")
                await char.notify(response)

            now = time.monotonic() - start
            self.crank_revolutions = (self.crank_revolutions + 1) % 0x10000
            self.crank_event_time = now
            if bike_data.notifying:
                # Indoor Bike Data flags: instantaneous speed (always present, More Data
                # bit clear), cadence (bit 2) and power (bit 6). Speed in 0.01 km/h,
                # cadence in 0.5 rpm.
                await bike_data.notify(struct.pack(
                    "<HHHh", (1 << 2) | (1 << 6), 3000, self.cadence * 2, self.power))
            if power_measurement.notifying:
                # Cycling Power Measurement flags: crank revolution data present (bit 5);
                # last crank event time in 1/1024 s.
                await power_measurement.notify(struct.pack(
                    "<HhHH", 1 << 5, self.power, self.crank_revolutions,
                    int(self.crank_event_time * 1024) % 0x10000))


async def main() -> None:
    parser = argparse.ArgumentParser(description="Mock BLE smart trainer on a bluezoo adapter")
    parser.add_argument("adapter", metavar="ADAPTER",
                        help="object path of the trainer's own adapter, e.g. /org/bluez/hci1")
    parser.add_argument("--name", default="KICKR", help="advertised name; default: %(default)s")
    parser.add_argument("--power", type=int, default=150, help="watts; default: %(default)s")
    parser.add_argument("--cadence", type=int, default=85, help="rpm; default: %(default)s")
    args = parser.parse_args()

    bus = sdbus.sd_bus_open_system()
    sdbus.set_default_bus(bus)
    trainer = Trainer(bus, args.name, args.power, args.cadence)
    trainer.export()
    try:
        await trainer.register(args.adapter)
    except Exception as error:
        print(f"trainer: registration failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
    await trainer.ride()


asyncio.run(main())
