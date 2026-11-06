#!/usr/bin/env bats

load test_helper

@test "runtime pruning follows all payloads without coupling Wine to the Rust launcher" {
    run python3 - "${ROOT}/snap/snapcraft.yaml" <<'PY'
import sys
import yaml

with open(sys.argv[1]) as source:
    snap = yaml.safe_load(source)
parts = snap["parts"]

def dependencies(name):
    result = set()
    for parent in parts[name].get("after", []):
        result.add(parent)
        result.update(dependencies(parent))
    return result

assert "zwift-gui" not in dependencies("wine")
assert {"wine", "zwift-gui", "launcher-runtime", "winetricks", "runfromprocess"} <= dependencies("runtime-prune")
assert "runtime-prune" in dependencies("gpu-2604")
assert "prune-closure.sh" not in parts["wine"].get("override-prime", "")
assert "prune-closure.sh" in parts["runtime-prune"]["override-prime"]
assert snap["apps"]["zwift"]["command"] == "bin/zwift-gui"
assert "rust-launcher" not in snap["apps"]
assert "yad" not in parts["wine"]["stage-packages"]
PY
    assert_success
}
