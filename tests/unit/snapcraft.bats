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
assert {"wine", "zwift-gui", "launcher-runtime", "winetricks", "windows-helpers"} <= dependencies("runtime-prune")
assert "runtime-prune" in dependencies("gpu-2604")
assert "prune-closure.sh" not in parts["wine"].get("override-prime", "")
assert "prune-closure.sh" in parts["runtime-prune"]["override-prime"]
assert snap["apps"]["zwift"]["command"] == "bin/zwift-gui"
assert "rust-launcher" not in snap["apps"]
assert "yad" not in parts["wine"]["stage-packages"]
PY
    assert_success
}

@test "WebKit helper absolute paths are exposed and guarded in the snap" {
    run python3 - "${ROOT}/snap/snapcraft.yaml" <<'PY'
import sys
import yaml

with open(sys.argv[1]) as source:
    snap = yaml.safe_load(source)
assert "glib-networking" in snap["parts"]["zwift-gui"]["stage-packages"]
assert "usr/lib/x86_64-linux-gnu/gio/modules/libgiognutls.so" in snap["parts"]["runtime-prune"]["override-prime"]
directory = "/usr/lib/x86_64-linux-gnu/webkitgtk-6.0"
assert snap["layout"][directory] == {"bind": "$SNAP" + directory}
guards = snap["parts"]["runtime-prune"]["override-prime"]
for helper in ("WebKitNetworkProcess", "WebKitWebProcess", "WebKitGPUProcess",
               "injected-bundle/libwebkitgtkinjectedbundle.so"):
    assert directory.lstrip("/") + "/" + helper in guards
PY
    assert_success
}

@test "pruning preserves libraries required only by WebKit private processes" {
    local prime="${BATS_TEST_TMPDIR}/prime"
    local lib="${prime}/usr/lib/x86_64-linux-gnu"
    mkdir -p "${lib}/webkitgtk-6.0" "${prime}/usr/libexec" "${prime}/opt/wine-devel"
    printf 'int helper(void) { return 0; }\n' > "${BATS_TEST_TMPDIR}/helper.c"
    printf 'extern int helper(void); int main(void) { return helper(); }\n' > "${BATS_TEST_TMPDIR}/main.c"
    cc -shared -fPIC -Wl,-soname,libwebkit-test.so -o "${lib}/libwebkit-test.so" "${BATS_TEST_TMPDIR}/helper.c"
    cc -o "${lib}/webkitgtk-6.0/WebKitNetworkProcess" "${BATS_TEST_TMPDIR}/main.c" -L"${lib}" -lwebkit-test
    cc -shared -fPIC -o "${lib}/libunused-test.so" "${BATS_TEST_TMPDIR}/helper.c"
    run "${ROOT}/snap/prune-closure.sh" "${prime}"
    assert_success
    assert_file_exists "${lib}/libwebkit-test.so"
    assert_file_not_exists "${lib}/libunused-test.so"
}
