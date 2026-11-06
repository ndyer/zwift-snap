#!/usr/bin/env bats

load test_helper

@test "stub profile completes onboarding with valid saved-game chunks and private attributes" {
    run python3 - "${ROOT}/tests/zwift-stub.py" <<'PY'
import importlib.util
import struct
import sys
import zlib

spec = importlib.util.spec_from_file_location("zwift_stub", sys.argv[1])
stub = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stub)


def varint(data, offset):
    value = shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
        shift += 7


def fields(data):
    offset = 0
    while offset < len(data):
        tag, offset = varint(data, offset)
        if tag & 7 == 0:
            value, offset = varint(data, offset)
        else:
            assert tag & 7 == 2, f"unexpected wire type: {tag & 7}"
            size, offset = varint(data, offset)
            value = data[offset:offset + size]
            assert len(value) == size
            offset += size
        yield tag >> 3, value


profile = list(fields(stub.profile()))
saved_game = dict(profile)[33]
assert struct.unpack("<IIIIIQII", saved_game) == (
    0x10000010, 28, 1, 0xE412BB7B, 2, 1, 0x10000005, 8,
)
attributes = {}
for field, value in profile:
    if field == 113:
        attribute = dict(fields(value))
        attributes[attribute[1]] = attribute[5].decode()
assert attributes == {
    zlib.crc32(name.encode()): value
    for name, value in {
        "SPORT_SELECT_TYPE": "CYCLIST",
        "TODAYS_SPORT_SELECT_TYPE": "CYCLIST",
        "FINAL_SPORT_SELECT_TYPE": "CYCLIST",
        "ONBOARD_CUSTOMIZATION_USER_TYPE_ZWIFT_GOAL": "RACING",
    }.items()
}
assert set(dict(fields(stub.login_response()))) == {1, 2, 3, 4}
PY
    assert_success
}

@test "stub OAuth tokens identify the fake Game_Launcher user and can be refreshed" {
    run python3 - "${ROOT}/tests/zwift-stub.py" "${BATS_TEST_TMPDIR}" <<'PY'
import base64
import importlib.util
import io
import json
import sys
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location("zwift_stub", sys.argv[1])
stub = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stub)
stub.settings.token = True
stub.settings.log = str(Path(sys.argv[2]) / "requests.log")

token = stub.fake_token()
for key, kind in (("access_token", "Bearer"), ("refresh_token", "Refresh"), ("id_token", "ID")):
    payload = token[key].split(".")[1]
    claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
    assert claims["azp"] == claims["aud"] == "Game_Launcher"
    assert claims["typ"] == kind
    assert claims["exp"] > time.time()
    assert claims["email"] == "offline@example.com"

body = f"client_id=Game_Launcher&grant_type=refresh_token&refresh_token={token['refresh_token']}".encode()
request = (
    f"POST {stub.TOKEN_PATH} HTTP/1.1\r\n"
    "Content-Type: application/x-www-form-urlencoded\r\n"
    f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n"
).encode() + body
response = io.BytesIO()
stub.serve_requests(io.BytesIO(request), response, "secure.zwift.com")
headers, body = response.getvalue().split(b"\r\n\r\n", 1)
assert headers.startswith(b"HTTP/1.1 200")
assert json.loads(body)["token_type"] == "bearer"
assert json.loads(body)["refresh_token"]
PY
    assert_success
}

@test "stub writes compact command-line token JSON before signalling readiness" {
    run python3 - "${ROOT}/tests/zwift-stub.py" "${BATS_TEST_TMPDIR}" <<'PY'
import json
from pathlib import Path
import subprocess
import sys
import time

directory = Path(sys.argv[2])
ready = directory / "ready"
token = directory / "token.json"
with subprocess.Popen([
    sys.executable, sys.argv[1], "--dir", str(directory), "--log", str(directory / "requests.log"),
    "--port", "0", "--token", "--token-file", str(token), "--ready-file", str(ready),
], stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
    try:
        deadline = time.monotonic() + 15
        while not ready.exists():
            assert process.poll() is None, process.communicate()
            assert time.monotonic() < deadline, "stub did not become ready"
            time.sleep(0.05)
        text = token.read_text()
        assert not any(character.isspace() for character in text)
        assert json.loads(text)["access_token"]
        assert json.loads(text)["refresh_token"]
    finally:
        process.terminate()
        process.communicate(timeout=5)
PY
    assert_success
}
