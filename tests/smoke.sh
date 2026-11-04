#!/usr/bin/env bash
# Visual smoke test of the zwift snap, driven with yarf
# (https://documentation.ubuntu.com/yarf). Starts a Mir compositor, runs the zwift
# launcher against it and lets the yarf suite in tests/yarf check on screen that
# Zwift gets installed and starts.
#
# The compositor renders on the host display through the workshop desktop interface
# when one is available, and on a headless virtual output otherwise (the same recipe
# as the yarf CI: --platform-display-libs mir:virtual).
#
# Usage: SMOKE_DISPLAY=auto|nested|headless tests/smoke.sh [extra yarf arguments]
# The first run downloads Zwift into the wine prefix and can take half an hour;
# later runs reuse the prefix and are quick.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT

readonly WAYLAND_SOCKET="wayland-99"
readonly MIR_EXTENSIONS="zwlr_screencopy_manager_v1:zwlr_virtual_pointer_manager_v1"
readonly MIR_VIRTUAL_OUTPUT="1280x1024"
readonly MIR_START_TIMEOUT=60
# yarf is a strict snap: it cannot see /tmp or /project, and its own XDG
# directories are remapped into $HOME/snap/yarf, so a suite under ~/.cache is
# invisible to it too. The suite and its output therefore live directly under
# $HOME and the suite is staged there.
readonly YARF_SUITE_DIR="${HOME}/zwift-smoke-suite"
readonly YARF_OUT_DIR="${HOME}/zwift-smoke-out"
readonly ZWIFT_LOG="${HOME}/snap/zwift/common/zwift.log"

mir_pid=""
zwift_pid=""

die() {
    echo "smoke: ${*}" >&2
    exit 1
}

cleanup() {
    # TERM makes zwift-run.sh run its own cleanup trap, which stops wine cleanly.
    [[ -z ${zwift_pid} ]] || kill -TERM "${zwift_pid}" 2> /dev/null
    pkill -TERM -f "zwift-run.sh" > /dev/null 2>&1
    # wine may keep running when the launcher died before its cleanup trap; the
    # dialog may be up too. Kill what is left, then stop the compositor.
    sleep 2
    pkill -TERM -f "ZwiftApp.exe|ZwiftLauncher.exe|wineserver" > /dev/null 2>&1
    pkill -TERM -x yad > /dev/null 2>&1
    sleep 1
    pkill -KILL -f "ZwiftApp.exe|ZwiftLauncher.exe|wineserver" > /dev/null 2>&1
    [[ -z ${mir_pid} ]] || kill "${mir_pid}" 2> /dev/null
    sleep 1
    if pgrep -f "ZwiftApp.exe|ZwiftLauncher.exe|wineserver" > /dev/null 2>&1; then
        echo "smoke: warning: wine processes survived the cleanup" >&2
    fi
}
trap cleanup EXIT

require_snap() {
    local name="${1:?}"
    shift
    if ! snap list "${name}" > /dev/null 2>&1; then
        echo "smoke: installing snap ${name}"
        sudo snap install "$@" "${name}" || die "unable to install the ${name} snap"
    fi
}

# connected PLUG [SLOT_PREFIX]: true when the given plug of the zwift snap is
# connected, optionally only to a slot whose name starts with SLOT_PREFIX
connected() {
    local plug="${1:?}"
    local slot_prefix="${2:-}"
    snap connections zwift 2> /dev/null | awk -v plug="zwift:${plug}" -v prefix="${slot_prefix}" \
        '$2 == plug && $3 != "-" && (prefix == "" || index($3, prefix) == 1) { found=1 } END { exit !found }'
}

ensure_snaps() {
    snap list zwift > /dev/null 2>&1 \
        || die "the zwift snap is not installed, run: workshop run zwift-snap-core26 build && workshop run zwift-snap-core26 install"

    # the gpu-2604-wrapper in the command chain refuses to run without a provider
    if ! connected gpu-2604 mesa-2604; then
        require_snap mesa-2604
        sudo snap connect zwift:gpu-2604 mesa-2604:gpu-2604
    fi

    require_snap yarf
    # optional: speeds up the OCR of yarf
    snap connections yarf 2> /dev/null | awk '$2 == "yarf:process-control" && $3 == "-" { found=1 } END { exit !found }' \
        && sudo snap connect yarf:process-control
    require_snap mir-test-tools
}

# clear_stale_socket: a previous run that died hard leaves the socket behind, which
# would make mir fail to bind and every later check pass without a compositor
clear_stale_socket() {
    if [[ -e "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}" ]] && ! pgrep -f mir_demo_server > /dev/null 2>&1; then
        echo "smoke: removing the stale ${WAYLAND_SOCKET} socket of an earlier run"
        rm -f "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}" "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}.lock"
    fi
}

# start_mir MODE: MODE is "nested" (render on the host display through the workshop
# desktop interface) or "headless" (virtual output, no display needed).
start_mir() {
    local mode="${1:?}"
    local mir_args=(--add-wayland-extensions "${MIR_EXTENSIONS}")

    if [[ ${mode} == "nested" ]]; then
        # The tutorial recipe: mir names its own socket after WAYLAND_DISPLAY and
        # picks the way it renders on the host display itself (through the sockets
        # the desktop interface provides).
        echo "smoke: starting the Mir compositor on the host display"
    else
        mir_args+=(--platform-display-libs mir:virtual --virtual-output "${MIR_VIRTUAL_OUTPUT}")
        echo "smoke: no display available, starting a headless Mir compositor (${MIR_VIRTUAL_OUTPUT})"
    fi

    export WAYLAND_DISPLAY="${WAYLAND_SOCKET}"
    mir-test-tools.demo-server "${mir_args[@]}" > "${YARF_OUT_DIR}/mir-${mode}.log" 2>&1 &
    mir_pid=$!

    # wait for the wayland socket of the compositor
    local waited=0
    while [[ ! -S "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}" ]]; do
        ((waited += 1))
        if ! kill -0 "${mir_pid}" 2> /dev/null; then
            if [[ ${mode} == "nested" ]]; then
                echo "smoke: the compositor for the host display did not start, falling back to a headless one"
                start_mir headless
                return
            fi
            die "the Mir compositor exited during startup, see ${YARF_OUT_DIR}/mir.log"
        fi
        [[ ${waited} -lt ${MIR_START_TIMEOUT} ]] \
            || die "the Mir wayland socket did not appear within ${MIR_START_TIMEOUT}s, see ${YARF_OUT_DIR}/mir.log"
        sleep 1
    done
    export XDG_SESSION_TYPE="wayland"
}

# stage_suite: yarf is a strict snap and cannot read /project, copy the suite home
stage_suite() {
    rm -rf "${YARF_SUITE_DIR}" "${YARF_OUT_DIR}"
    mkdir -p "${YARF_SUITE_DIR}" "${YARF_OUT_DIR}"
    cp -r "${ROOT}/tests/yarf/." "${YARF_SUITE_DIR}/"
}

report_failure() {
    echo "smoke: FAILED, artifacts in ${YARF_OUT_DIR} (log.html shows the screenshots)"
    if [[ -f ${ZWIFT_LOG} ]]; then
        echo "smoke: last 30 lines of the zwift launcher log:"
        tail -30 "${ZWIFT_LOG}"
    fi
}

main() {
    ensure_snaps
    clear_stale_socket
    mkdir -p "${YARF_OUT_DIR}"
    stage_suite

    case "${SMOKE_DISPLAY:-auto}" in
    auto)
        # nested needs a real host display: the desktop interface of the workshop
        # sets WAYLAND_DISPLAY/DISPLAY and provides their sockets. A bare env var
        # with no socket behind it means there is nothing to nest onto.
        if [[ -n ${WAYLAND_DISPLAY:-} && -S "${XDG_RUNTIME_DIR}/${WAYLAND_DISPLAY}" ]] \
            || [[ -n ${DISPLAY:-} && -S /tmp/.X11-unix/"${DISPLAY#:}" ]]; then
            start_mir nested
        else
            start_mir headless
        fi
        ;;
    nested | headless)
        start_mir "${SMOKE_DISPLAY}"
        ;;
    *)
        die "SMOKE_DISPLAY must be auto, nested or headless, got '${SMOKE_DISPLAY}'"
        ;;
    esac

    # the launcher links the compositor socket into the runtime dir of the snap and
    # runs everything (dialogs and wine) on native wayland
    snap run zwift > "${YARF_OUT_DIR}/zwift.out" 2>&1 &
    zwift_pid=$!

    # yarf exits 120 with no output when its stdout is a plain file (snap stdio
    # quirk), so let it write through a pipe into the log file instead.
    if ! yarf --platform Mir "${YARF_SUITE_DIR}" --outdir "${YARF_OUT_DIR}" "$@" \
        2>&1 | tee "${YARF_OUT_DIR}/yarf.log"; then
        echo "smoke: the yarf suite failed, see ${YARF_OUT_DIR}/yarf.log"
        report_failure
        exit 1
    fi

    # The suite can only judge the screen, and a failed install also closes the
    # progress dialog (with an error dialog in its place), so confirm the game is
    # really running.
    if ! pgrep -f ZwiftApp.exe > /dev/null 2>&1; then
        report_failure
        die "the yarf suite passed but the ZwiftApp.exe wine process is not running"
    fi

    echo "smoke: PASSED"
}

main "$@"
