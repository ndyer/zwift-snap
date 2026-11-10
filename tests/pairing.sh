#!/usr/bin/env bash
# Pairing-screen test for the zwift snap: fakes the Zwift login servers so the game reaches its
# device pairing screen without a Zwift account, with the bluezoo Bluetooth mock running.
#
# It is a test aid, never shipped in the snap. tests/zwift-stub.py stands in for Zwift's login
# (secure.zwift.com) and game (us-or-rly101.zwift.com) servers; the game is pointed at it with
# https_proxy and trusts its throwaway CA (appended to the prefix's data/cacert.pem, restored
# afterwards). The game is driven directly (the launcher's Wine parent-process sequence, with a
# fake token), not through the real launcher, so the launcher's sync never re-downloads the
# game files over the CA. The stub's pre-onboarded profile skips onboarding; yarf only drives
# device pairing.
#
# Usage: SMOKE_DISPLAY=auto|nested|headless tests/pairing.sh [--explore] [extra yarf arguments]
# With --explore, the game keeps running after reaching the pairing screen for manual
# exploration. Use tests/tools/yarf/yarf.sh screenshot and input commands to interact with it.
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
readonly STOP_SCRIPT="${HOME}/snap/zwift/common/pairing-stop.sh"
readonly TOKEN_FILE="${HOME}/snap/zwift/common/pairing-token.json"
# how long the game is kept running while yarf drives it; must outlast the yarf suite
# in explore mode, extend this to allow manual testing
readonly RUN_SECONDS_DEFAULT=360
readonly RUN_SECONDS_EXPLORE=3600
export BLUEZOO_KEEPALIVE_SECONDS="${RUN_SECONDS_DEFAULT}"

explore_mode=false
yarf_args=()

mir_pid=""
stub_pid=""
game_pid=""
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

# shellcheck source=tests/tools/bluetooth/bluezoo.sh
source "${ROOT}/tests/tools/bluetooth/bluezoo.sh"

report_failure() {
    echo "pairing: FAILED, artifacts in ${OUT_DIR} (log.html shows the screenshots)"
    if [[ -d ${OUT_DIR}/frames ]]; then
        echo "pairing: every captured frame is in ${OUT_DIR}/frames (index.log names the wait), replay them with:"
        echo "pairing:   tests/tools/yarf/yarf.sh ocr ${OUT_DIR}/frames -g KICKR"
    fi
    for log in stub/requests.log yarf.log; do
        [[ -f ${OUT_DIR}/${log} ]] && { echo "pairing: tail of ${log}:"; tail -15 "${OUT_DIR}/${log}"; }
    done
    [[ -f ${GAME_LOG} ]] && { echo "pairing: last game states:"; grep -aE "ZSF_|GameFlowState" "${GAME_LOG}" | tail -8; }
    if [[ -f ${OUT_DIR}/game.log ]]; then
        grep -aE 'wine: Call .*unimplemented function|Unhandled exception' "${OUT_DIR}/game.log" || true
    fi
}

cleanup() {
    bluezoo_stop
    # Killing the snap run wrapper or SIGTERMing wineserver leaves the prefix's Windows
    # processes (services, svchost, winedevice, the Zwift crash handler...) orphaned and
    # running; wineserver -k kills every client of the prefix. It fails harmlessly when the
    # game has already shut the server down.
    if [[ -n ${game_pid} ]]; then
        # shellcheck disable=SC2016
        # snap run fails silently when its stdout is a plain file, so it goes through a pipe
        snap run --shell zwift.wine -c 'WINEPREFIX="$HOME/../common/.wine" wineserver -k' 2>&1 \
            | cat > /dev/null || true
        kill -TERM "${game_pid}" 2> /dev/null || true
        wait "${game_pid}" 2> /dev/null || true
        game_pid=""
    fi
    [[ -z ${stub_pid} ]] || kill "${stub_pid}" 2> /dev/null
    [[ -z ${mir_pid} ]] || kill "${mir_pid}" 2> /dev/null
    # restore the game's cacert.pem (remove our appended CA)
    [[ -z ${cacert_appended} ]] || restore_cacert
    rm -f "${RUN_SCRIPT}" "${STOP_SCRIPT}" "${TOKEN_FILE}"
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
    # a killed run (no EXIT trap) leaves its CA behind: drop it, or it is backed up as original
    if grep -q '^# zwift-stub test CA' "${CACERT}"; then
        local clean
        clean=$(awk '/^# zwift-stub test CA/ { exit } { print }' "${CACERT}")
        printf '%s\n' "${clean}" > "${CACERT}"
    fi
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
        --port "${STUB_PORT}" --token --token-file "${TOKEN_FILE}" \
        --ready-file "${OUT_DIR}/stub/ready" > "${OUT_DIR}/stub/out.log" 2>&1 &
    stub_pid=$!
    local waited=0
    while [[ ! -f "${OUT_DIR}/stub/ready" ]]; do
        kill -0 "${stub_pid}" 2> /dev/null || { cat "${OUT_DIR}/stub/out.log"; die "the stub exited during startup"; }
        ((waited += 1))
        [[ ${waited} -lt 15 ]] || die "the stub did not become ready"
        sleep 1
    done
}

# check_trainer: yarf saw a CONNECTED tile; confirm from the game log that Zwift connected the mock
# trainer in all its roles, and from the trainer's own log that it subscribed to its data
check_trainer() {
    local characteristic devices waited=0
    # Zwift reports the connection asynchronously, so poll instead of sampling the log once
    until grep -aq "Device: \"${BLUEZOO_TRAINER_NAME}\" has new connection status: connected" "${GAME_LOG}" 2> /dev/null; do
        ((waited += 1))
        [[ ${waited} -lt 30 ]] \
            || { report_failure; die "Zwift did not connect the mock trainer ${BLUEZOO_TRAINER_NAME}"; }
        sleep 1
    done
    waited=0
    # BlueZ drops LE connections while a discovery runs, so winebth stops discovery before
    # connecting the trainer and starts it again once the connection is up.
    grep -aq "Pausing LE discovery on" "${OUT_DIR}/game.log" \
        || { report_failure; die "winebth did not pause LE discovery while connecting the trainer"; }
    grep -aq "Resuming LE discovery on" "${OUT_DIR}/game.log" \
        || { report_failure; die "winebth did not resume LE discovery after connecting the trainer"; }
    # The last device list in the log is the current assignment (captured first: grep -q would
    # close the pipe early and pipefail would report tac's SIGPIPE as a failure)
    while :; do
        devices=$(tac "${GAME_LOG}" | sed -n '/^\[[0-9:]*\] Device List:/q;p')
        if [[ ${devices} == *"Power: ${BLUEZOO_TRAINER_NAME} "* ]]; then
            break
        fi
        ((waited += 1))
        [[ ${waited} -lt 30 ]] \
            || { report_failure; die "Zwift did not assign the mock trainer the power role"; }
        sleep 1
    done
    for characteristic in indoor-bike-data cycling-power-measurement ftms-control-point zwift-async zwift-sync-tx; do
        grep -q "start notify ${characteristic}" "${OUT_DIR}/trainer.log" \
            || { report_failure; die "Zwift did not subscribe to the trainer's ${characteristic}"; }
    done
}

# start_game: drive the game directly with the stub's fake OAuth token, inside the snap
start_game() {
    local run_seconds="${1:-${RUN_SECONDS_DEFAULT}}"
    # The discovery-pause assertions above read winebth's trace, so +winebth must be on
    case "${WINEDEBUG:-}" in
    *winebth*) ;;
    "") WINEDEBUG="+winebth" ;;
    *) WINEDEBUG="${WINEDEBUG},+winebth" ;;
    esac
    export WINEDEBUG
    cat > "${RUN_SCRIPT}" <<EOF
set -uo pipefail
export ZWIFT_NO_GUI=1
export https_proxy="${STUB_PROXY}" HTTPS_PROXY="${STUB_PROXY}"
# winebth would otherwise also expose the mock trainer's own adapter as a radio, and Zwift's
# watcher would scan from it; see tests/tools/bluetooth/bluezoo.sh
export ROUVY_BT_ADAPTER=hci0
# The snap ships neither gecko nor mono, and a wine upgrade updates the prefix on the first
# run, where wineboot would otherwise open a modal Wine Mono Installer over the game and
# hang the start. Disable both exactly as the launcher does for its own wineboot (see
# launcher-gui/src/install.rs); the value carries winemenubuilder because wine-env.sh only
# applies its own winemenubuilder.exe=d default when this variable is unset.
export WINEDLLOVERRIDES="mscoree,mshtml=;winemenubuilder.exe=d"
${WINEDEBUG:+export WINEDEBUG="${WINEDEBUG}"}
export WINEPREFIX="\${SNAP_USER_COMMON}/.wine"
export ZWIFT_INSTALL_DIR="\${WINEPREFIX}/drive_c/Program Files (x86)/Zwift"
if [[ -n \${WAYLAND_DISPLAY:-} && \${WAYLAND_DISPLAY} != /* && -n \${XDG_RUNTIME_DIR:-} \
    && ! -e \${XDG_RUNTIME_DIR}/\${WAYLAND_DISPLAY} ]]; then
    ln -sf "/run/user/\$(id -u)/\${WAYLAND_DISPLAY}" "\${XDG_RUNTIME_DIR}/\${WAYLAND_DISPLAY}" || exit 1
fi
unset DISPLAY
wine_task_pid() {
    wine tasklist /fo list /fi "IMAGENAME eq \$1" | grep -m1 -Po '^PID:[\\t ]*\\K[0-9]+'
}
wait_until_wine_task_started() {
    for _ in {1..100}; do
        wine_task_pid "\$1" > /dev/null && return 0
        sleep 0.1
    done
    echo "Timed out waiting for \$1" >&2
    return 1
}
zwift_wine_dir="\$(winepath -w "\${ZWIFT_INSTALL_DIR}")"
# ZwiftApp.exe only starts when its parent image is called ZwiftLauncher.exe. The
# snap's copy of that name is the shim that starts the command line it is given
# (the real launcher would patch the game instead); the launcher puts it there
# too, see launcher-gui/src/launch.rs.
cp "\${SNAP}/bin/ZwiftLauncher.exe" "\${ZWIFT_INSTALL_DIR}/ZwiftLauncher.exe" || exit 1
token="\$(cat "\$HOME/../common/pairing-token.json")" || exit 1
wine start /d "\${zwift_wine_dir}" ZwiftLauncher.exe ZwiftApp.exe "--token=\${token}"
wait_until_wine_task_started ZwiftApp.exe || exit 1
# The shim is the game's parent: it reaps it and forwards its exit code, so it
# stays up for as long as the game runs. The prefix is deliberately left running
# when the game exits, so that stop_game can unload winebth cleanly.
for _ in \$(seq 1 ${run_seconds}); do
    wine_task_pid ZwiftApp.exe > /dev/null || break
    sleep 1
done
EOF
    (
        # shellcheck disable=SC2016
        # snap run loses its output when stdout is a plain file, so it goes through tee
        snap run --shell zwift.wine -c 'bash "$HOME/../common/pairing-run.sh"' 2>&1 \
            | tee "${OUT_DIR}/game.log" > /dev/null
    ) &
    game_pid=$!
}

# stop_game: unload winebth, then shut the prefix down and wait for the game's snap run to
# finish, so the winebth driver-unload trace is flushed before it is checked.
#
# A prefix shutdown never removes drivers: 'wineserver -k' SIGKILLs every process, and even
# wineserver's graceful path only signals an event that nothing consumes, so
# IRP_MN_REMOVE_DEVICE is never sent and winebth's disconnect-on-driver-shutdown cannot run.
# The one path that reaches it is a clean stop of the winedevice service group, whose exit
# from wine_ntoskrnl_main_loop removes the root PnP devices (verified: stopping the group
# logs IRP_MN_SURPRISE_REMOVAL and IRP_MN_REMOVE_DEVICE on winebth's FDO). The group
# services are synthesised per load-order group as Winedevice<N> and numbered in driver start
# order, so stop the ones that exist; at teardown the others are harmless to stop.
stop_game() {
    [[ -n ${game_pid} ]] || return 0
    cat > "${STOP_SCRIPT}" <<'EOF'
set -uo pipefail
# winebth is in the WinePlugPlay load-order group; winebus, winehid, wineusb and the
# System Bus Extender drivers are in the others.
export WINEPREFIX="${SNAP_USER_COMMON}/.wine"
for n in 1 2 3 4 5 6; do
    wine sc stop "Winedevice${n}" > /dev/null 2>&1
done
# give the driver time to disconnect the trainer over D-Bus, which waits for its event loop
sleep 5
wineserver -k || true
EOF
    # shellcheck disable=SC2016
    # snap run fails silently when its stdout is a plain file, so it goes through a pipe
    snap run --shell zwift.wine -c 'bash "$HOME/../common/pairing-stop.sh"' 2>&1 \
        | tee "${OUT_DIR}/stop.log" > /dev/null || true
    wait "${game_pid}" 2> /dev/null || true
    game_pid=""
}

main() {
    # parse arguments to extract --explore flag
    while [[ $# -gt 0 ]]; do
        case "$1" in
        --explore)
            explore_mode=true
            export BLUEZOO_KEEPALIVE_SECONDS="${RUN_SECONDS_EXPLORE}"
            shift
            ;;
        *)
            yarf_args+=("$1")
            shift
            ;;
        esac
    done

    ensure_prerequisites
    rm -rf "${OUT_DIR}" "${YARF_SUITE_DIR}"
    mkdir -p "${OUT_DIR}" "${YARF_SUITE_DIR}"
    cp -r "${ROOT}/tests/pairing/." "${YARF_SUITE_DIR}/"
    cp "${ROOT}/tests/tools/yarf/yarf_ocr.py" "${YARF_SUITE_DIR}/"
    cp "${ROOT}/tests/tools/yarf/yarf_frames.py" "${YARF_SUITE_DIR}/"
    # every frame the suite captures is kept here, so a failed wait can be replayed with
    # 'tests/tools/yarf/yarf.sh ocr <frames> -g KICKR'
    mkdir -p "${OUT_DIR}/frames"
    export YARF_FRAME_DIR="${OUT_DIR}/frames"

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
    bluezoo_trainer_start
    bluezoo_scan_start
    start_stub
    append_cacert
    
    # Determine game run duration based on explore mode
    local game_run_seconds="${RUN_SECONDS_DEFAULT}"
    if [[ ${explore_mode} == true ]]; then
        game_run_seconds="${RUN_SECONDS_EXPLORE}"
    fi
    start_game "${game_run_seconds}"

    # the game needs a moment to come up before yarf starts matching the pairing screen
    echo "pairing: waiting for the Zwift window"
    local waited=0
    until grep -aq "ZSF_UNINITIALIZED -> ZSF_LOGIN\|ZSF_TOKEN_LOGIN" "${GAME_LOG}" 2> /dev/null; do
        ((waited += 1))
        [[ ${waited} -lt 120 ]] || { report_failure; die "Zwift did not begin login"; }
        sleep 1
    done

    # yarf exits 120 with no output when its stdout is a plain file, so pipe it through tee
    if ! yarf --platform Mir "${YARF_SUITE_DIR}" --outdir "${OUT_DIR}" "${yarf_args[@]}" 2>&1 | tee "${OUT_DIR}/yarf.log"; then
        # explore mode is for watching a failing run live, so it carries on to the wait below
        if [[ ${explore_mode} == true ]]; then
            echo "pairing: note: the yarf suite failed; continuing in explore mode"
        else
            report_failure
            die "the yarf suite failed"
        fi
    fi

    if [[ ${explore_mode} == false ]]; then
        # yarf judges the screen; confirm the game actually reached the pairing state
        grep -aq "ZSF_PAIRING_SCREEN" "${GAME_LOG}" 2> /dev/null \
            || { report_failure; die "the yarf suite passed but the game did not reach ZSF_PAIRING_SCREEN"; }

        check_trainer
    fi

    # keep a screenshot of the pairing screen with the connected trainer
    "${ROOT}/tests/tools/yarf/yarf.sh" screenshot -s "${WAYLAND_SOCKET}" "${OUT_DIR}/pairing-screen.png" > /dev/null 2>&1 \
        || echo "pairing: note: could not capture the pairing screenshot"

    if [[ ${explore_mode} == false ]]; then
        # Stopping the prefix now exercises winebth's device-removal path: a device that is still
        # connected must be disconnected, so the mock does not keep advertising it as connected.
        stop_game
        grep -aqE "Disconnecting .* on (device removal|driver shutdown)" "${OUT_DIR}/game.log" \
            || { report_failure; die "winebth did not disconnect the trainer when the driver shut down"; }
    fi

    if [[ ${explore_mode} == true ]]; then
        echo "pairing: explore mode - game running for ${RUN_SECONDS_EXPLORE}s at pairing screen"
        echo "pairing: use 'tests/tools/yarf/yarf.sh screenshot -s ${WAYLAND_SOCKET} [FILE.png]' to view the screen"
        echo "pairing: use 'tests/tools/yarf/yarf.sh input -s ${WAYLAND_SOCKET} <actions>' to interact"
        echo "pairing: tail the logs with: tail -f ~/zwift-pairing-out/bluezoo.log ~/snap/zwift/common/zwift.log"
        echo "pairing: game log at: ${GAME_LOG}"
        echo "pairing: bluezoo log at: ${OUT_DIR}/bluezoo.log, mock trainer log at: ${OUT_DIR}/trainer.log"
        echo "pairing: captured frames at: ${OUT_DIR}/frames (replay: tests/tools/yarf/yarf.sh ocr ${OUT_DIR}/frames -g KICKR)"
        # Wait for the game to finish or user interruption
        local waited=0
        while pgrep -f "C:\\\\windows\\\\system32\\\\winedevice.exe" > /dev/null 2>&1; do
            sleep 10
            ((waited += 1))
            if [[ $((waited % 6)) -eq 0 ]]; then
                local remaining=$((RUN_SECONDS_EXPLORE - waited * 10))
                [[ ${remaining} -gt 0 ]] && echo "pairing: explore mode running (${remaining}s remaining)"
            fi
        done
    fi

    echo "pairing: PASSED (the game reached the pairing screen and connected the mock trainer" \
        "${BLUEZOO_TRAINER_NAME} as its power source)"
}

main "$@"
