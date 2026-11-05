#!/usr/bin/env bash
# Bluetooth plumbing test for the zwift snap, driven by the BlueZ D-Bus mock
# bluezoo (https://github.com/Samsung/BlueZoo).
#
# bluezoo takes the place of bluetoothd and owns org.bluez on the workshop
# system bus (bluetooth.service is masked by the SDK setup-base hook). The
# confined snap reaches it through its bluez plug, exactly as it would reach a
# real BlueZ on the host. With WINEDEBUG=+winebth the test then checks that
# wine's Bluetooth driver
# - sees the two mock adapters (radios),
# - registers its authentication agent with BlueZ, and
# - learns about a peer device that the mock reports as discovered.
#
# This is a plumbing test: Zwift itself cannot see any of it yet, because
# wine 11.21 does not implement the BLE advertisement watcher (see README,
# "Bluetooth does not work in Zwift").
#
# Usage: tests/bluetooth.sh
# The zwift snap must already be installed (workshop run zwift-snap install);
# this script never installs or removes it.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT

readonly OUT_DIR="${HOME}/zwift-bt-out"
# The throwaway wine prefix lives in the snap data directory: the snap has no
# home plug, so that is the only place inside $HOME it may write. Note that
# the snap app remaps $HOME to its private ~/snap/zwift/x1 directory, so the
# wine commands below have to use $HOME/../common explicitly.
readonly BT_PREFIX="${HOME}/snap/zwift/common/bt-test"
# hci0 is the radio winebth attaches to, hci1 acts as the peer device that
# gets discovered (see tests/bluetooth-scan.py).
readonly CENTRAL_ADAPTER="/org/bluez/hci0"
readonly PEER_ADAPTER="/org/bluez/hci1"
readonly PEER_ADDRESS="00:00:00:22:22:22"
readonly PEER_DEVICE_NAME="dev_00_00_00_22_22_22"
readonly BLUEZOO_START_TIMEOUT=20
readonly TRACE_WAIT_TIMEOUT=60
# How long the wine keep-alive and the scan client run; every wait below must
# finish well within this.
readonly KEEPALIVE_SECONDS=120

wine_pid=""
scan_pid=""
bluezoo_pid=""

die() {
    echo "bluetooth: ${*}" >&2
    exit 1
}

report_failure() {
    echo "bluetooth: FAILED, artifacts in ${OUT_DIR}"
    if [[ -f ${OUT_DIR}/wine-trace.log ]]; then
        echo "bluetooth: last 20 winebth lines of the trace:"
        grep winebth "${OUT_DIR}/wine-trace.log" | tail -20
    fi
    for log in bluezoo.log scan.log; do
        if [[ -f ${OUT_DIR}/${log} ]]; then
            echo "bluetooth: last 10 lines of ${log}:"
            tail -10 "${OUT_DIR}/${log}"
        fi
    done
}

cleanup() {
    [[ -z ${scan_pid} ]] || kill "${scan_pid}" 2> /dev/null
    [[ -z ${wine_pid} ]] || kill "${wine_pid}" 2> /dev/null
    [[ -z ${bluezoo_pid} ]] || {
        sudo kill "${bluezoo_pid}" 2> /dev/null
        sudo pkill -f "bluezoo --auto-enable" > /dev/null 2>&1
    }
    # Stop only the wineserver of the test prefix, identified by its
    # WINEPREFIX in /proc, so a real Zwift session is never touched.
    local pid
    for pid in $(pgrep -x wineserver); do
        if tr '\0' '\n' < "/proc/${pid}/environ" 2> /dev/null \
            | grep -qxF "WINEPREFIX=${BT_PREFIX}"; then
            kill "${pid}" 2> /dev/null
        fi
    done
    sleep 1
    # the throwaway prefix is recreated on every run
    rm -rf "${BT_PREFIX}"
}
trap cleanup EXIT

# connected PLUG: true when the given plug of the zwift snap is connected
connected() {
    local plug="${1:?}"
    snap connections zwift 2> /dev/null \
        | awk -v plug="zwift:${plug}" '$2 == plug && $3 != "-" { found=1 } END { exit !found }'
}

ensure_prerequisites() {
    snap list zwift > /dev/null 2>&1 \
        || die "the zwift snap is not installed, run: workshop run zwift-snap build && workshop run zwift-snap install"
    command -v bluezoo > /dev/null 2>&1 \
        || die "bluezoo is not installed, it is set up by the SDK setup-base hook (recreate the workshop)"
    command -v bluetoothd > /dev/null 2>&1 \
        || die "the bluez package is not installed, it is set up by the SDK setup-base hook (recreate the workshop)"

    # bluez is manual-connect-only, but connecting it is the point of this
    # test, same as the gpu-2604 handling in tests/smoke.sh
    if ! connected bluez; then
        sudo snap connect zwift:bluez
    fi

    # bluetoothd must not own org.bluez; the SDK setup-base hook masks it, but
    # this workshop may be older than that hook change. Masking is not enough
    # on its own: the bluez package ships a D-Bus activation file that points
    # at /bin/false and makes clients fail with a confusing systemd error the
    # first time they hit an unowned org.bluez name. Remove it (it belongs to
    # the bluez package, so it comes back on a package update; this is a test
    # container).
    if systemctl is-enabled bluetooth.service > /dev/null 2>&1; then
        sudo systemctl mask --now bluetooth.service
    fi
    if [[ -f /usr/share/dbus-1/system-services/org.bluez.service ]]; then
        sudo rm -f /usr/share/dbus-1/system-services/org.bluez.service
    fi
}

# wait_for TIMEOUT DESCRIPTION COMMAND [ARGS...]: run the command every second
# until it succeeds
wait_for() {
    local timeout="${1:?}" description="${2:?}"
    shift 2
    local waited=0
    until "$@" > /dev/null 2>&1; do
        ((waited += 1))
        if [[ ${waited} -ge ${timeout} ]]; then
            report_failure
            die "${description} did not happen within ${timeout}s"
        fi
        sleep 1
    done
}

bluez_name_owned() {
    busctl list --no-legend 2> /dev/null | grep -qw org.bluez
}

mock_adapters_up() {
    gdbus introspect --system --dest org.bluez --object-path /org/bluez \
        2> /dev/null | grep -q hci0 \
        && gdbus introspect --system --dest org.bluez --object-path /org/bluez \
            2> /dev/null | grep -q hci1
}

start_bluezoo() {
    # A run that died hard leaves bluezoo behind; it must be gone before the
    # new one can own org.bluez.
    sudo pkill -f "bluezoo --auto-enable" > /dev/null 2>&1
    sleep 1

    # bluezoo must run as root: the bluez D-Bus policy only lets root own
    # org.bluez. setsid detaches it from this script's process group so a
    # cancelled caller does not take it down with it. The log is written by
    # the root shell inside the sudo command, because a redirect on the
    # outside would belong to the calling user's sudo wrapper.
    sudo setsid sh -c "exec bluezoo --auto-enable --scan-interval=1 \
        -a 00:00:00:11:11:11 -a ${PEER_ADDRESS} \
        > '${OUT_DIR}/bluezoo.log' 2>&1" < /dev/null &
    bluezoo_pid=$!

    wait_for "${BLUEZOO_START_TIMEOUT}" "bluezoo taking over org.bluez" bluez_name_owned
    wait_for "${BLUEZOO_START_TIMEOUT}" "the mock adapters hci0 and hci1 coming up" mock_adapters_up
}

start_scan() {
    # The scan client must stay connected or bluezoo stops scanning; it also
    # makes the peer adapter discoverable (see tests/bluetooth-scan.py).
    /opt/bluezoo/bin/python "${ROOT}/tests/bluetooth-scan.py" \
        "${CENTRAL_ADAPTER}" "${PEER_ADAPTER}" \
        --timeout "${KEEPALIVE_SECONDS}" \
        > "${OUT_DIR}/scan.log" 2>&1 &
    scan_pid=$!
    wait_for 10 "the scan client starting the discovery" \
        grep -q "discovery started" "${OUT_DIR}/scan.log"
}

start_wine() {
    # wineboot and the keep-alive command run in one shell so that every wine
    # process, including the winedevice that hosts winebth, writes its trace
    # into the same stream. The command goes into a small script file because
    # snap run only sees files inside the snap data directory, and its stdout
    # is written on the host side: inside the snap /tmp is private to each
    # command and a plain file there is gone before it can be read.
    rm -rf "${BT_PREFIX}"
    mkdir -p "${BT_PREFIX}"
    cat > "${BT_PREFIX}/trace-wine.sh" <<EOF
export WINEPREFIX="\$HOME/../common/bt-test"
# gecko and mono installs are disabled at wineboot by the real launcher too;
# winemenubuilder is silenced by wine-env.sh
WINEDEBUG=+winebth WINEDLLOVERRIDES='mshtml,mscoree=' wine wineboot -i
WINEDEBUG=+winebth wine cmd /c 'ping -n ${KEEPALIVE_SECONDS} 127.0.0.1'
EOF
    (
        # snap run fails with an empty output when its stdout is a regular
        # file (the same snapd stdio quirk that makes yarf exit 120 on a
        # plain file), so the trace goes through a tee pipe into the log.
        # The single quotes around the -c argument are deliberate: $HOME has
        # to be expanded inside the snap environment, where it is remapped to
        # the private ~/snap/zwift/x1 directory.
        # shellcheck disable=SC2016
        snap run --shell zwift.wine -c 'bash "$HOME/../common/bt-test/trace-wine.sh"' 2>&1 \
            | tee "${OUT_DIR}/wine-trace.log" > /dev/null
    ) &
    wine_pid=$!
}

check_trace_failures() {
    local trace="${OUT_DIR}/wine-trace.log"
    [[ -f ${trace} ]] || return 0
    if grep -q "Failed to get system dbus connection" "${trace}"; then
        report_failure
        die "winebth could not reach the system D-Bus at all"
    fi
    if grep -q "Failed to register authentication agent" "${trace}"; then
        report_failure
        die "winebth failed to register its authentication agent with BlueZ"
    fi
    if grep -q "Denying method call" "${trace}"; then
        report_failure
        die "the snap confinement denied winebth's D-Bus traffic; is zwift:bluez connected?"
    fi
}

wait_for_device() {
    local waited=0
    while ! grep -q "${PEER_DEVICE_NAME}" "${OUT_DIR}/wine-trace.log" 2> /dev/null; do
        ((waited += 1))
        if [[ ${waited} -ge ${TRACE_WAIT_TIMEOUT} ]]; then
            check_trace_failures
            report_failure
            die "the mock peer device did not reach winebth within ${TRACE_WAIT_TIMEOUT}s"
        fi
        sleep 1
    done
}

check_trace() {
    local trace="${OUT_DIR}/wine-trace.log"
    for adapter in hci0 hci1; do
        if ! grep -q "New BlueZ org.bluez.Adapter1 object \"/org/bluez/${adapter}\"" "${trace}"; then
            report_failure
            die "winebth did not see the mock adapter ${adapter}"
        fi
    done
    grep -q "Adding new bluetooth radio" "${trace}" \
        || { report_failure; die "winebth did not create radio devices for the mock adapters"; }
    grep -q "Registered authentication agent" "${trace}" \
        || { report_failure; die "winebth did not register its authentication agent with BlueZ"; }
    grep -q "${PEER_DEVICE_NAME}" "${trace}" \
        || { report_failure; die "winebth did not see the discovered peer device ${PEER_ADDRESS}"; }
}

main() {
    ensure_prerequisites
    rm -rf "${OUT_DIR}"
    mkdir -p "${OUT_DIR}"

    start_bluezoo
    start_scan
    start_wine
    wait_for_device
    check_trace_failures
    check_trace

    echo "bluetooth: PASSED (winebth saw both mock adapters, its agent and the discovered peer ${PEER_ADDRESS})"
}

main "$@"
