#!/usr/bin/env bash
# Pairing-screen test for the zwift snap: fakes the Zwift login servers so the game reaches its
# device pairing screen without a Zwift account, with the bluezoo Bluetooth mock running.
#
# It is a test aid, never shipped in the snap. tests/zwift-stub.py stands in for Zwift's login
# (secure.zwift.com) and game (us-or-rly101.zwift.com) servers; the game is pointed at it with
# https_proxy and trusts its throwaway CA (appended to the prefix's data/cacert.pem, restored
# afterwards). The game is driven directly (the launcher dance of launcher/zwift-run.sh, without a
# token, so the in-game login form appears), not through the real launcher, so the Zwift patcher
# never runs and cannot overwrite the CA. The yarf suite in tests/pairing/ then types fake
# credentials and clicks through the onboarding screens to the pairing screen.
#
# Usage: SMOKE_DISPLAY=auto|nested|headless tests/pairing.sh [extra yarf arguments]
# The zwift snap must already be installed and Zwift downloaded into the prefix (run the smoke
# test once first); this script never installs, updates or removes anything.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT

readonly WAYLAND_SOCKET="wayland-99"
readonly MIR_EXTENSIONS="zwlr_screencopy_manager_v1:zwlr_virtual_pointer_manager_v1"
readonly MIR_VIRTUAL_OUTPUT="1280x1024"
readonly MIR_START_TIMEOUT=60
readonly STUB_PORT=8443
readonly STUB_PROXY="http://127.0.0.1:${STUB_PORT}"
# yarf is a strict snap and cannot see /project or /tmp, so its suite and output live under $HOME
readonly YARF_SUITE_DIR="${HOME}/zwift-pairing-suite"
readonly OUT_DIR="${HOME}/zwift-pairing-out"
readonly PREFIX="${HOME}/snap/zwift/common/.wine"
readonly CACERT="${PREFIX}/drive_c/Program Files (x86)/Zwift/data/cacert.pem"
readonly ZWIFT_USER_DIR="${PREFIX}/drive_c/users/${USER}/AppData/Local/Zwift"
readonly GAME_LOG="${ZWIFT_USER_DIR}/Logs/Log.txt"
readonly RUN_SCRIPT="${HOME}/snap/zwift/common/pairing-run.sh"
# how long the game is kept running while yarf drives it; must outlast the yarf suite
readonly RUN_SECONDS=360
export BLUEZOO_KEEPALIVE_SECONDS="${RUN_SECONDS}"

mir_pid=""
stub_pid=""
cacert_appended=""

die() {
    echo "pairing: ${*}" >&2
    exit 1
}

# connected PLUG: true when the given plug of the zwift snap is connected
connected() {
    local plug="${1:?}"
    snap connections zwift 2> /dev/null \
        | awk -v plug="zwift:${plug}" '$2 == plug && $3 != "-" { found=1 } END { exit !found }'
}

# shellcheck source=tests/lib/bluezoo.sh
source "${ROOT}/tests/lib/bluezoo.sh"

report_failure() {
    echo "pairing: FAILED, artifacts in ${OUT_DIR} (log.html shows the screenshots)"
    for log in stub/requests.log yarf.log; do
        [[ -f ${OUT_DIR}/${log} ]] && { echo "pairing: tail of ${log}:"; tail -15 "${OUT_DIR}/${log}"; }
    done
    [[ -f ${GAME_LOG} ]] && { echo "pairing: last game states:"; grep -aE "ZSF_|GameFlowState" "${GAME_LOG}" | tail -8; }
}

cleanup() {
    bluezoo_stop
    # stop the game: TERM the keep-alive snap run, then the wineserver of this prefix only
    pkill -TERM -f "pairing-run.sh" > /dev/null 2>&1
    sleep 2
    local pid
    for pid in $(pgrep -x wineserver); do
        if tr '\0' '\n' < "/proc/${pid}/environ" 2> /dev/null | grep -qxF "WINEPREFIX=${PREFIX}"; then
            kill "${pid}" 2> /dev/null
        fi
    done
    [[ -z ${stub_pid} ]] || kill "${stub_pid}" 2> /dev/null
    [[ -z ${mir_pid} ]] || kill "${mir_pid}" 2> /dev/null
    # restore the game's cacert.pem (remove our appended CA)
    [[ -z ${cacert_appended} ]] || restore_cacert
    rm -f "${RUN_SCRIPT}"
    rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/${WAYLAND_SOCKET}" \
        "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/${WAYLAND_SOCKET}.lock"
}
trap cleanup EXIT
trap 'exit 1' TERM INT HUP

ensure_prerequisites() {
    snap list zwift > /dev/null 2>&1 \
        || die "the zwift snap is not installed, run: workshop run zwift-snap build && workshop run zwift-snap install"
    [[ -f "${PREFIX}/drive_c/Program Files (x86)/Zwift/ZwiftApp.exe" ]] \
        || die "Zwift is not installed in the prefix, run the smoke test once to download it"
    command -v openssl > /dev/null 2>&1 || die "openssl is needed to make the stub certificate"
    if ! connected gpu-2604 > /dev/null 2>&1 || ! connected gpu-2604; then
        sudo snap connect zwift:gpu-2604 mesa-2604:gpu-2604
    fi
    bluezoo_ensure_prerequisites
}

# append_cacert: trust the stub CA by appending it to the game's cacert.pem, after a backup
append_cacert() {
    [[ -f ${CACERT} ]] || die "the game cacert.pem is missing: ${CACERT}"
    cp -a "${CACERT}" "${OUT_DIR}/cacert.pem.orig"
    {
        echo
        echo "# zwift-stub test CA (tests/pairing.sh) -- removed when the test ends"
        cat "${OUT_DIR}/stub/ca.pem"
    } >> "${CACERT}"
    cacert_appended=1
}

restore_cacert() {
    [[ -f ${OUT_DIR}/cacert.pem.orig ]] && cp -a "${OUT_DIR}/cacert.pem.orig" "${CACERT}"
    cacert_appended=""
}

clear_stale_socket() {
    local runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
    if [[ -e "${runtime_dir}/${WAYLAND_SOCKET}" ]] && ! pgrep -f mir_demo_server > /dev/null 2>&1; then
        rm -f "${runtime_dir}/${WAYLAND_SOCKET}" "${runtime_dir}/${WAYLAND_SOCKET}.lock"
    fi
}

# start_mir MODE: "nested" (render on the host display) or "headless" (virtual output)
start_mir() {
    local mode="${1:?}"
    local runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
    local mir_args=(--add-wayland-extensions "${MIR_EXTENSIONS}")
    if [[ ${mode} == "nested" && -n ${WAYLAND_DISPLAY:-} ]]; then
        mir_args+=(--wayland-host "${WAYLAND_DISPLAY}")
        echo "pairing: starting the Mir compositor on the host display ${WAYLAND_DISPLAY}"
    elif [[ ${mode} == "headless" ]]; then
        mir_args+=(--platform-display-libs mir:virtual --virtual-output "${MIR_VIRTUAL_OUTPUT}")
        echo "pairing: starting a headless Mir compositor (${MIR_VIRTUAL_OUTPUT})"
    fi
    export WAYLAND_DISPLAY="${WAYLAND_SOCKET}"
    mir-test-tools.demo-server "${mir_args[@]}" > "${OUT_DIR}/mir-${mode}.log" 2>&1 &
    mir_pid=$!
    local waited=0
    while [[ ! -S "${runtime_dir}/${WAYLAND_SOCKET}" ]]; do
        if ! kill -0 "${mir_pid}" 2> /dev/null; then
            if [[ ${mode} == "nested" ]]; then
                echo "pairing: the nested compositor did not start, falling back to headless"
                start_mir headless
                return
            fi
            die "the Mir compositor exited during startup, see ${OUT_DIR}/mir-${mode}.log"
        fi
        ((waited += 1))
        [[ ${waited} -lt ${MIR_START_TIMEOUT} ]] || die "the Mir wayland socket did not appear within ${MIR_START_TIMEOUT}s"
        sleep 1
    done
    export XDG_SESSION_TYPE="wayland"
}

start_stub() {
    mkdir -p "${OUT_DIR}/stub"
    rm -f "${OUT_DIR}/stub/ready" "${OUT_DIR}/stub/ready.pid"
    python3 "${ROOT}/tests/zwift-stub.py" --dir "${OUT_DIR}/stub" --log "${OUT_DIR}/stub/requests.log" \
        --port "${STUB_PORT}" --token --ready-file "${OUT_DIR}/stub/ready" > "${OUT_DIR}/stub/out.log" 2>&1 &
    stub_pid=$!
    local waited=0
    while [[ ! -f "${OUT_DIR}/stub/ready" ]]; do
        kill -0 "${stub_pid}" 2> /dev/null || { cat "${OUT_DIR}/stub/out.log"; die "the stub exited during startup"; }
        ((waited += 1))
        [[ ${waited} -lt 15 ]] || die "the stub did not become ready"
        sleep 1
    done
}

# start_game: drive the game directly (launcher dance without a token), inside the snap
start_game() {
    cat > "${RUN_SCRIPT}" <<EOF
set -uo pipefail
export ZWIFT_NO_GUI=1
export https_proxy="${STUB_PROXY}" HTTPS_PROXY="${STUB_PROXY}"
# shellcheck source=/dev/null
source "\${SNAP}/bin/zwift-common.sh"
link_wayland_socket
unset DISPLAY
zwift_wine_dir="\$(winepath -w "\${ZWIFT_INSTALL_DIR}")"
wine start /d "\${zwift_wine_dir}" ZwiftLauncher.exe SilentLaunch
wait_until_wine_task_started ZwiftLauncher.exe
launcher_pid="\$(wine_task_pid ZwiftLauncher.exe)"
wine start /d "\${zwift_wine_dir}" /unix "\${SNAP}/bin/runfromprocess-rs.exe" "\${launcher_pid}" ZwiftApp.exe
wait_until_wine_task_started ZwiftApp.exe
wine taskkill /f /im ZwiftLauncher.exe > /dev/null 2>&1 || true
for _ in \$(seq 1 ${RUN_SECONDS}); do
    is_wine_task_running ZwiftApp.exe || break
    sleep 1
done
wineserver -k || true
EOF
    (
        # shellcheck disable=SC2016
        snap run --shell zwift.wine -c 'bash "$HOME/../common/pairing-run.sh"' \
            > "${OUT_DIR}/game.log" 2>&1
    ) &
}

main() {
    ensure_prerequisites
    rm -rf "${OUT_DIR}" "${YARF_SUITE_DIR}"
    mkdir -p "${OUT_DIR}" "${YARF_SUITE_DIR}"
    cp -r "${ROOT}/tests/pairing/." "${YARF_SUITE_DIR}/"

    clear_stale_socket
    case "${SMOKE_DISPLAY:-auto}" in
    auto)
        if [[ -n ${WAYLAND_DISPLAY:-} && -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/${WAYLAND_DISPLAY}" ]]; then
            start_mir nested
        else
            start_mir headless
        fi
        ;;
    nested | headless) start_mir "${SMOKE_DISPLAY}" ;;
    *) die "SMOKE_DISPLAY must be auto, nested or headless, got '${SMOKE_DISPLAY}'" ;;
    esac

    bluezoo_start
    bluezoo_scan_start
    start_stub
    append_cacert
    start_game

    # the game needs a moment to come up before yarf starts matching the login form
    echo "pairing: waiting for the Zwift window"
    local waited=0
    until grep -aq "ZSF_UNINITIALIZED -> ZSF_LOGIN\|ZSF_TOKEN_LOGIN" "${GAME_LOG}" 2> /dev/null; do
        ((waited += 1))
        [[ ${waited} -lt 120 ]] || { report_failure; die "the Zwift login screen did not appear"; }
        sleep 1
    done

    # yarf exits 120 with no output when its stdout is a plain file, so pipe it through tee
    if ! yarf --platform Mir "${YARF_SUITE_DIR}" --outdir "${OUT_DIR}" "$@" 2>&1 | tee "${OUT_DIR}/yarf.log"; then
        report_failure
        die "the yarf suite failed"
    fi

    # yarf judges the screen; confirm the game actually reached the pairing state
    if ! grep -aq "ZSF_PAIRING_SCREEN" "${GAME_LOG}" 2> /dev/null; then
        report_failure
        die "the yarf suite passed but the game did not reach ZSF_PAIRING_SCREEN"
    fi

    # keep a screenshot of the pairing screen (its Bluetooth tile shows wine's BLE support state)
    "${ROOT}/tests/tools/yarf.sh" screenshot -s "${WAYLAND_SOCKET}" "${OUT_DIR}/pairing-screen.png" > /dev/null 2>&1 \
        || echo "pairing: note: could not capture the pairing screenshot"

    echo "pairing: PASSED (the game reached the device pairing screen)"
}

main "$@"
