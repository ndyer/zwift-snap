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
readonly TRACE_WAIT_TIMEOUT=60
# How long the wine keep-alive runs; every wait below must finish well within it. The bluezoo
# scan client runs for the same time (BLUEZOO_KEEPALIVE_SECONDS in the helper).
readonly KEEPALIVE_SECONDS=120
export BLUEZOO_KEEPALIVE_SECONDS="${KEEPALIVE_SECONDS}"

wine_pid=""

die() {
    echo "bluetooth: ${*}" >&2
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
    bluezoo_stop
    [[ -z ${wine_pid} ]] || kill "${wine_pid}" 2> /dev/null
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

ensure_prerequisites() {
    snap list zwift > /dev/null 2>&1 \
        || die "the zwift snap is not installed, run: workshop run zwift-snap build && workshop run zwift-snap install"
    bluezoo_ensure_prerequisites
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
    while ! grep -q "${BLUEZOO_PEER_DEVICE_NAME}" "${OUT_DIR}/wine-trace.log" 2> /dev/null; do
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
    grep -q "${BLUEZOO_PEER_DEVICE_NAME}" "${trace}" \
        || { report_failure; die "winebth did not see the discovered peer device ${BLUEZOO_PEER_ADDRESS}"; }
}

main() {
    ensure_prerequisites
    rm -rf "${OUT_DIR}"
    mkdir -p "${OUT_DIR}"

    bluezoo_start
    bluezoo_scan_start
    start_wine
    wait_for_device
    check_trace_failures
    check_trace

    echo "bluetooth: PASSED (winebth saw both mock adapters, its agent and the discovered peer ${BLUEZOO_PEER_ADDRESS})"
}

main "$@"
