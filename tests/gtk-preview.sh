#!/usr/bin/env bash
# Exercise the Rust GTK preview with Mir and yarf, without running Wine.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runtime="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"
socket="wayland-$$"
stage="$(mktemp -d "${HOME}/zwift-gtk-preview.XXXXXX")"
mir_pid=""
gui_pid=""
socket_linked=false

cleanup() {
    if [[ -n ${gui_pid} ]]; then kill "${gui_pid}" 2>/dev/null || true; wait "${gui_pid}" 2>/dev/null || true; fi
    if [[ -n ${mir_pid} ]]; then kill "${mir_pid}" 2>/dev/null || true; wait "${mir_pid}" 2>/dev/null || true; fi
    if [[ ${socket_linked} == true ]]; then rm -f "${runtime}/snap.yarf/${socket}"; fi
    echo "GTK preview artifacts: ${stage}"
}
trap cleanup EXIT

cargo build --manifest-path "${root}/launcher-gui/Cargo.toml" --features gtk-ui
mkdir -p "${stage}/suite" "${stage}/close-suite" "${stage}/runtime-suite" "${stage}/out" "${runtime}/snap.yarf"
cp "${root}/tests/gtk-preview/preview.robot" "${root}/tests/tools/yarf/yarf_ocr.py" "${stage}/suite/"
cp "${root}/tests/gtk-preview/close.robot" "${root}/tests/tools/yarf/yarf_ocr.py" "${stage}/close-suite/"
cp "${root}/tests/gtk-preview/runtime.robot" "${root}/tests/tools/yarf/yarf_ocr.py" "${stage}/runtime-suite/"
cp "${root}/tests/tools/yarf/input.py" "${stage}/"

export WAYLAND_DISPLAY="${socket}" XDG_SESSION_TYPE=wayland
if [[ -e ${runtime}/${socket} || -L ${runtime}/${socket} || -e ${runtime}/${socket}.lock \
    || -e ${runtime}/snap.yarf/${socket} || -L ${runtime}/snap.yarf/${socket} ]]; then
    echo "Wayland socket name already in use: ${socket}" >&2
    exit 1
fi
mir-test-tools.demo-server \
    --add-wayland-extensions zwlr_screencopy_manager_v1:zwlr_virtual_pointer_manager_v1 \
    --platform-display-libs mir:virtual --virtual-output 1280x1024 \
    > >(tee "${stage}/mir.log") 2>&1 &
mir_pid=$!
for ((i = 0; i < 60; i++)); do
    [[ -S ${runtime}/${socket} ]] && break
    kill -0 "${mir_pid}" 2>/dev/null || { echo "Mir exited during startup" >&2; exit 1; }
    sleep 1
done
[[ -S ${runtime}/${socket} ]] || { echo "Mir socket did not appear" >&2; exit 1; }
ln -s "${runtime}/${socket}" "${runtime}/snap.yarf/${socket}"
socket_linked=true

echo "Testing cancellation of a pending decision"
mkdir -p "${stage}/out/close"
ZWIFT_DISPLAY_MODE=800x600 "${root}/launcher-gui/target/debug/zwift-gui" --preview \
    > >(tee "${stage}/gui-close.log") 2>&1 &
gui_pid=$!
yarf --platform Mir "${stage}/close-suite" --outdir "${stage}/out/close" \
    2>&1 | tee "${stage}/yarf-close.log"
# shellcheck disable=SC2016
snap run --shell yarf -c \
    'python3 "$1/input.py" "$2" key Alt_L F4' _ "${stage}" "${socket}" 2>&1 | cat
for ((i = 0; i < 50; i++)); do
    kill -0 "${gui_pid}" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "${gui_pid}" 2>/dev/null; then
    echo "GTK preview did not exit after closing the window" >&2
    exit 1
fi
wait "${gui_pid}"
gui_pid=""
grep -q "Launcher cancelled" "${stage}/gui-close.log" \
    || { echo "GTK preview did not report cancellation" >&2; exit 1; }
for mode in fullscreen 800x600; do
    echo "Testing GTK preview in ${mode} mode"
    mkdir -p "${stage}/out/${mode}"
    ZWIFT_DISPLAY_MODE="${mode}" "${root}/launcher-gui/target/debug/zwift-gui" --preview \
        > >(tee "${stage}/gui-${mode}.log") 2>&1 &
    gui_pid=$!
    yarf --platform Mir "${stage}/suite" --outdir "${stage}/out/${mode}" \
        2>&1 | tee "${stage}/yarf-${mode}.log"
    kill -0 "${gui_pid}" 2>/dev/null || { echo "GTK preview exited unexpectedly" >&2; exit 1; }
    kill "${gui_pid}"
    wait "${gui_pid}" 2>/dev/null || true
    gui_pid=""
done

echo "Testing the real GTK lifecycle against stub Wine"
fixture="${stage}/runtime"
game="${fixture}/common/.wine/drive_c/Program Files (x86)/Zwift"
data="${fixture}/common/.wine/drive_c/users/tester/AppData/Local/Zwift"
mkdir -p "${fixture}/bin" "${game}" "${data}" "${stage}/out/runtime"
touch "${game}/ZwiftApp.exe" "${game}/ZwiftLauncher.exe"
printf '<Zwift sversion="1.0.0 x"/>' > "${game}/Zwift_ver_cur.xml"
printf '<ZWIFT><CONFIG/></ZWIFT>' > "${data}/prefs.xml"
cat > "${fixture}/bin/wget" <<'STUB'
#!/bin/sh
printf '<Zwift sversion="2.0.0 x"/>'
STUB
cat > "${fixture}/bin/winepath" <<'STUB'
#!/bin/sh
printf 'C:\\Zwift\n'
STUB
cat > "${fixture}/bin/wineserver" <<'STUB'
#!/bin/sh
echo "$1" >> "$TEST_ROOT/cleanup"
STUB
cat > "${fixture}/bin/wine" <<'STUB'
#!/bin/sh
set -eu
case "$1" in
start)
    if [ ! -f "$TEST_ROOT/update-failed" ]; then
        touch "$TEST_ROOT/update-failed"
        echo 'updater diagnostic' >&2
        exit 7
    fi
    echo 'stub launch output';;
tasklist)
    case "$5" in
    *ZwiftLauncher.exe) printf 'PID: 41\n';;
    *ZwiftApp.exe)
        if [ ! -f "$TEST_ROOT/release" ]; then
            touch "$TEST_ROOT/game-running"
            printf 'PID: 42\n'
        fi;;
    esac;;
*/wait-for-window.exe) touch "$TEST_ROOT/window-ready";;
taskkill) :;;
*) exit 1;;
esac
STUB
chmod +x "${fixture}/bin/"*
PATH="${fixture}/bin:${PATH}" SNAP="${fixture}" SNAP_USER_COMMON="${fixture}/common" \
    WINEPREFIX="${fixture}/common/.wine" ZWIFT_LOG="${fixture}/common/zwift.log" \
    TEST_ROOT="${fixture}" ZWIFT_NO_GUI=0 ZWIFT_DISPLAY_MODE=800x600 ZWIFT_POLL_INTERVAL=1 \
    "${root}/launcher-gui/target/debug/zwift-gui" \
    > >(tee "${stage}/gui-runtime.log") 2>&1 &
gui_pid=$!
yarf --platform Mir "${stage}/runtime-suite" --outdir "${stage}/out/runtime" \
    2>&1 | tee "${stage}/yarf-runtime.log"
for ((i = 0; i < 100; i++)); do
    [[ -f ${fixture}/window-ready ]] && break
    kill -0 "${gui_pid}" 2>/dev/null || { echo "Real GTK worker exited before window handoff" >&2; exit 1; }
    sleep 0.1
done
[[ -f ${fixture}/window-ready ]] || { echo "Real GTK worker did not wait for the game window" >&2; exit 1; }
touch "${fixture}/release"
for ((i = 0; i < 100; i++)); do
    kill -0 "${gui_pid}" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "${gui_pid}" 2>/dev/null; then
    echo "Real GTK worker did not exit after the game closed" >&2
    exit 1
fi
wait "${gui_pid}"
gui_pid=""
grep -q 'Zwift closed, exiting' "${fixture}/common/zwift.log"
grep -q '<USER_RESOLUTION_PREF>800x600</USER_RESOLUTION_PREF>' "${data}/prefs.xml"
[[ $(cat "${fixture}/cleanup") == $'-k\n-w' ]] \
    || { echo "Real GTK worker did not clean up Wine" >&2; exit 1; }
