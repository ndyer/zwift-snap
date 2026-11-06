#!/usr/bin/env bats

load test_helper

@test "trainer exposes standard FTMS control and the real trainer's Zwift custom write path" {
    run /opt/bluezoo/bin/python - "${ROOT}/tests/tools/bluetooth/bluetooth-trainer.py" <<'PY'
import importlib.util
import struct
import sys
from unittest.mock import Mock, patch

import sdbus

spec = importlib.util.spec_from_file_location("bluetooth_trainer", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

trainer = module.Trainer(Mock(), "KICKR", 150, 85)
trainer.manager = Mock()
with patch.object(module.Advertisement, "export_to_dbus"):
    trainer.export()

exported = {
    call.args[0]: call.args[1]
    for call in trainer.manager.export_with_manager.call_args_list
}
assert exported[f"{module.APP_PATH}/service8"].uuid == module.ZWIFT_SERVICE
for name, uuid, flags in (
    ("zwift-async", module.ZWIFT_ASYNC, ["notify"]),
    ("zwift-sync-rx", module.ZWIFT_SYNC_RX, ["write"]),
    ("zwift-sync-tx", module.ZWIFT_SYNC_TX, ["indicate"]),
    ("ftms-control-point", module.uuid16(0x2AD9), ["write", "indicate"]),
):
    char = trainer.characteristic(name)
    assert char.uuid == uuid
    assert char.flags == flags
    assert char.service_path in exported

assert trainer.characteristic("ftms-feature").value == struct.pack(
    "<II", (1 << 1) | (1 << 14), (1 << 3) | (1 << 13),
)
assert len(trainer.advertisement.uuids) == 8
assert module.FTMS in trainer.advertisement.uuids
assert module.CYCLING_POWER in trainer.advertisement.uuids
assert module.ZWIFT_SERVICE not in trainer.advertisement.uuids

control = trainer.characteristic("ftms-control-point")
for request in (b"\x00", b"\x05\x96\x00", b"\x11" + b"\x00" * 6):
    control.on_write(control, request)
assert trainer.pending_indications == [
    (control, bytes((0x80, opcode, 0x01))) for opcode in (0x00, 0x05, 0x11)
]

sync_rx = trainer.characteristic("zwift-sync-rx")
try:
    sync_rx.on_write(sync_rx, b"RideOn")
except sdbus.DbusNotSupportedError:
    pass
else:
    raise AssertionError("an unimplemented Zwift handshake must not silently succeed")
PY
    assert_success
}
