#!/usr/bin/env bash
# Visual smoke test of the zwift snap, driven with yarf
# (https://documentation.ubuntu.com/yarf). Starts a Mir compositor, runs the zwift
# launcher against it and lets the yarf suite in tests/smoke check on screen that
# Zwift gets installed and starts.
#
# The compositor renders on the host display through the workshop desktop interface
# when one is available, and on a headless virtual output otherwise (the same recipe
# as the yarf CI: --platform-display-libs mir:virtual).
#
# Usage: SMOKE_DISPLAY=auto|nested|headless tests/smoke.sh [--fresh] [extra yarf arguments]
# The first run downloads Zwift into the wine prefix and can take half an hour;
# later runs reuse the prefix and are quick.
#
# The downloads (the Zwift game tree) are also kept in a cache under
# $HOME/zwift-cache, which the run restores before starting and updates after
# passing: see tests/tools/dl-cache/dl-cache.sh. Set SMOKE_CACHE=0 or pass --fresh
# to ignore it and wipe what it staged, for a real cold install.
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

# the download cache helper, sourced for its DL_CACHE_* variables and functions
# shellcheck source=tests/tools/dl-cache/dl-cache.sh
source "${ROOT}/tests/tools/dl-cache/dl-cache.sh"

mir_pid=""
zwift_pid=""

die() {
    echo "smoke: ${*}" >&2
    exit 1
}

cleanup() {
    if [[ -n ${zwift_pid} ]]; then
        kill -TERM "${zwift_pid}" 2> /dev/null || true
        for ((i = 0; i < 150; i++)); do
            kill -0 "${zwift_pid}" 2> /dev/null || break
            sleep 0.2
        done
        if kill -0 "${zwift_pid}" 2> /dev/null; then
            echo "smoke: launcher did not stop after TERM; stopping its Wine prefix" >&2
            # shellcheck disable=SC2016
            timeout 15 snap run --shell zwift.wine -c \
                'WINEPREFIX="$SNAP_USER_COMMON/.wine" wineserver -k' 2>&1 | tee -a "${YARF_OUT_DIR}/cleanup.log"
            kill -KILL "${zwift_pid}" 2> /dev/null || true
        fi
        wait "${zwift_pid}" 2> /dev/null || true
    fi
    if [[ -n ${mir_pid} ]]; then
        kill "${mir_pid}" 2> /dev/null || true
        wait "${mir_pid}" 2> /dev/null || true
    fi
}
trap cleanup EXIT

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
        || die "the zwift snap is not installed, run: workshop run zwift-snap build && workshop run zwift-snap install"

    # the gpu-2604-wrapper in the command chain refuses to run without a provider;
    # mesa-2604 is installed by the SDK setup-base hook, just connect it if needed
    if ! connected gpu-2604 mesa-2604; then
        sudo snap connect zwift:gpu-2604 mesa-2604:gpu-2604
    fi
}

# clear_stale_socket: a previous run that died hard leaves the socket behind, which
# would make mir fail to bind and every later check pass without a compositor
clear_stale_socket() {
    if [[ -e "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}" ]] && ! pgrep -f mir_demo_server > /dev/null 2>&1; then
        echo "smoke: removing the stale ${WAYLAND_SOCKET} socket of an earlier run"
        rm -f "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}" "${XDG_RUNTIME_DIR}/${WAYLAND_SOCKET}.lock"
    fi
}

# clear_prefs: reset cached Zwift login details (prefs.xml) from previous runs
clear_prefs() {
    local prefs_file="${SNAP_USER_COMMON:-$HOME/snap/zwift}/.wine/drive_c/users/workshop/AppData/Local/Zwift/prefs.xml"
    rm -f "${prefs_file}" 2>/dev/null || true
    echo "smoke: cleared cached prefs from previous run"
}

# prepare_install FRESH: put the cached game tree into the wine prefix so the
# installer only has to fetch what changed, or wipe it for a cold install when
# FRESH is non-empty.
prepare_install() {
    local install_dir="${DL_CACHE_PREFIX}/${DL_CACHE_GAME_SUBDIR}"
    if [[ -n ${1} ]]; then
        echo "smoke: --fresh, removing ${install_dir}"
        rm -rf "${install_dir}"
        return 0
    fi
    if ! dl_cache_present; then
        echo "smoke: no Zwift download cache yet, this run downloads everything"
        return 0
    fi
    dl_cache_stage
}

# start_mir MODE: MODE is "nested" (render on the host display through the workshop
# desktop interface) or "headless" (virtual output, no display needed).
start_mir() {
    local mode="${1:?}"
    local mir_args=(--add-wayland-extensions "${MIR_EXTENSIONS}")

    if [[ ${mode} == "nested" ]]; then
        # mir names its own socket after WAYLAND_DISPLAY, so the host socket must be
        # passed with --wayland-host before the export below replaces its value
        if [[ -n ${WAYLAND_DISPLAY:-} ]]; then
            mir_args+=(--wayland-host "${WAYLAND_DISPLAY}")
            echo "smoke: starting the Mir compositor on the host wayland display ${WAYLAND_DISPLAY}"
        else
            echo "smoke: starting the Mir compositor on the host display"
        fi
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
    mkdir -p "${YARF_SUITE_DIR}" "${YARF_OUT_DIR}" "${YARF_OUT_DIR}/frames"
    cp -r "${ROOT}/tests/smoke/." "${YARF_SUITE_DIR}/"
    cp "${ROOT}/tests/tools/yarf/yarf_ocr.py" "${YARF_SUITE_DIR}/"
    cp "${ROOT}/tests/tools/yarf/yarf_frames.py" "${YARF_SUITE_DIR}/"
    # every frame the suite captures is kept here, so a failed wait can be replayed
    # with 'tests/tools/yarf/yarf.sh ocr <frames> -g <text>'
    export YARF_FRAME_DIR="${YARF_OUT_DIR}/frames"
}

report_failure() {
    echo "smoke: FAILED, artifacts in ${YARF_OUT_DIR} (log.html shows the screenshots)"
    if [[ -d ${YARF_OUT_DIR}/frames ]]; then
        echo "smoke: every captured frame is in ${YARF_OUT_DIR}/frames (index.log names the wait), replay them with:"
        echo "smoke:   tests/tools/yarf/yarf.sh ocr ${YARF_OUT_DIR}/frames -g <text>"
    fi
    if [[ -f ${ZWIFT_LOG} ]]; then
        echo "smoke: last 30 lines of the zwift launcher log:"
        tail -30 "${ZWIFT_LOG}"
    fi
}

main() {
    # --fresh and SMOKE_CACHE=0 belong to this harness, not to yarf, so keep them
    # out of the arguments forwarded to the suite
    local fresh="" arg
    local -a yarf_args=()
    for arg in "$@"; do
        case ${arg} in
        --fresh) fresh=1 ;;
        *) yarf_args+=("${arg}") ;;
        esac
    done
    if [[ ${SMOKE_CACHE:-1} == 0 ]]; then
        fresh=1
    fi

    ensure_snaps
    clear_stale_socket
    clear_prefs
    prepare_install "${fresh}"
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
    # Skip login to go straight to the game (enables robot tests)
    snap run zwift --skip-login > >(tee "${YARF_OUT_DIR}/zwift.out") 2>&1 &
    zwift_pid=$!

    # yarf exits 120 with no output when its stdout is a plain file (snap stdio
    # quirk), so let it write through a pipe into the log file instead.
    if ! yarf --platform Mir "${YARF_SUITE_DIR}" --outdir "${YARF_OUT_DIR}" "${yarf_args[@]}" \
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

    # keep this run's downloads for the next one; a no-op when nothing changed
    if [[ -d ${DL_CACHE_PREFIX}/${DL_CACHE_GAME_SUBDIR} ]]; then
        dl_cache_capture || echo "smoke: could not update the download cache" >&2
    fi

    echo "smoke: PASSED"
}

main "$@"
