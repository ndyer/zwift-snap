#!/usr/bin/env bats

load test_helper

@test "trainer exposes standard FTMS control on a real-trainer service set" {
    run /opt/bluezoo/bin/python - "${ROOT}/tests/tools/bluetooth/bluetooth-trainer.py" <<'PY'
import importlib.util
import struct
import sys
from unittest.mock import Mock, patch

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
for name, uuid, flags in (
    ("ftms-control-point", module.uuid16(0x2AD9), ["write", "indicate"]),
    ("cycling-power-measurement", module.uuid16(0x2A63), ["notify"]),
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
# The Zwift Play custom service is deliberately absent: a real trainer does not
# advertise it, and doing so makes Zwift hang on the Play handshake. See the
# harness header and .kb/pairing-test.md.
assert not any(c.name.startswith("zwift-") for c in trainer.characteristics)

control = trainer.characteristic("ftms-control-point")
for request in (b"\x00", b"\x05\x96\x00", b"\x11" + b"\x00" * 6):
    control.on_write(control, request)
assert trainer.pending_indications == [
    (control, bytes((0x80, opcode, 0x01))) for opcode in (0x00, 0x05, 0x11)
]
PY
    assert_success
}
