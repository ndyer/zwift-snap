#!/usr/bin/env bash
# Bluetooth plumbing test for the zwift snap, driven by the BlueZ D-Bus mock
# bluezoo (https://github.com/Samsung/BlueZoo).
#
# bluezoo takes the place of bluetoothd and owns org.bluez on the workshop
# system bus (bluetooth.service is masked by the SDK setup-base hook). The
# confined snap reaches it through its bluez plug, exactly as it would reach a
# real BlueZ on the host. bluezoo runs two adapters: hci0 is the PC's radio, hci1 is a mock
# BLE smart trainer (tests/bluetooth-trainer.py) that hci0 discovers over LE. With
# WINEDEBUG=+winebth,+dbus the test then checks that wine's Bluetooth driver
# - uses hci0 as its only radio and ignores the trainer's adapter (ROUVY_BT_ADAPTER),
# - registers its authentication agent with BlueZ, and
# - learns about the trainer from its LE advertisement (the two service UUIDs, FTMS and
#   Cycling Power) and creates a Windows device for it.
#
# This is a plumbing test without a Windows program: nothing connects to the trainer or
# reads its GATT services here, tests/pairing.sh covers that through Zwift.
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
    # wineserver -k kills every process of the test prefix only, so a real Zwift session is
    # never touched; SIGTERMing the wineserver instead leaves its clients orphaned and running.
    # shellcheck disable=SC2016
    snap run --shell zwift.wine -c 'WINEPREFIX="$HOME/../common/bt-test" wineserver -k' \
        > /dev/null 2>&1 || true
    [[ -z ${wine_pid} ]] || kill "${wine_pid}" 2> /dev/null
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
# winebth would otherwise also expose the trainer's adapter as a radio, see bluezoo.sh
export ROUVY_BT_ADAPTER=hci0
# gecko and mono installs are disabled at wineboot by the real launcher too;
# winemenubuilder is silenced by wine-env.sh
WINEDEBUG=+winebth,+dbus WINEDLLOVERRIDES='mshtml,mscoree=' wine wineboot -i
WINEDEBUG=+winebth,+dbus wine cmd /c 'ping -n ${KEEPALIVE_SECONDS} 127.0.0.1'
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

# the trainer is fully plumbed once winebth has started the Windows device it created for it
trainer_device_started() {
    grep -q "remote_device_pdo_pnp .*IRP_MN_START_DEVICE" "${OUT_DIR}/wine-trace.log" 2> /dev/null
}

wait_for_device() {
    local waited=0
    while ! trainer_device_started; do
        ((waited += 1))
        if [[ ${waited} -ge ${TRACE_WAIT_TIMEOUT} ]]; then
            check_trace_failures
            report_failure
            die "the mock trainer did not reach winebth within ${TRACE_WAIT_TIMEOUT}s"
        fi
        sleep 1
    done
}

# trainer_uuid_count: the number of service UUIDs in the properties winebth parsed for the
# trainer (the +dbus channel shows the property types and array sizes, not the values)
trainer_uuid_count() {
    local device="${BLUEZOO_CENTRAL_ADAPTER}/${BLUEZOO_TRAINER_DEVICE_NAME}"
    awk -v start="bluez_handle_new_object (\"${device}\"" '
        index($0, start) { inside = 1; next }
        inside && /bluez_device_prop_from_dict_entry \("UUIDs"/ {
            match($0, /count=[0-9]+/); print substr($0, RSTART + 6, RLENGTH - 6); exit
        }
        inside && /New org.bluez.Device1 object/ { exit }
    ' "${OUT_DIR}/wine-trace.log"
}

check_trace() {
    local trace="${OUT_DIR}/wine-trace.log" radios uuids
    grep -q "New BlueZ org.bluez.Adapter1 object \"${BLUEZOO_CENTRAL_ADAPTER}\"" "${trace}" \
        || { report_failure; die "winebth did not see the mock adapter hci0"; }
    grep -q "winebth: ignoring Bluetooth adapter hci1" "${trace}" \
        || { report_failure; die "winebth did not ignore the trainer's adapter hci1 (ROUVY_BT_ADAPTER)"; }
    radios=$(grep -c "Adding new bluetooth radio" "${trace}")
    [[ ${radios} -eq 1 ]] \
        || { report_failure; die "winebth created ${radios} radios, expected only the one for hci0"; }
    grep -q "Registered authentication agent" "${trace}" \
        || { report_failure; die "winebth did not register its authentication agent with BlueZ"; }
    grep -q "New org.bluez.Device1 object \"${BLUEZOO_CENTRAL_ADAPTER}/${BLUEZOO_TRAINER_DEVICE_NAME}\"" "${trace}" \
        || { report_failure; die "winebth did not see the trainer ${BLUEZOO_TRAINER_ADDRESS} on hci0"; }
    uuids=$(trainer_uuid_count)
    [[ ${uuids} == 2 ]] \
        || { report_failure; die "winebth saw ${uuids:-no} service UUIDs for the trainer, expected 2 (FTMS, Cycling Power)"; }
}

main() {
    ensure_prerequisites
    rm -rf "${OUT_DIR}"
    mkdir -p "${OUT_DIR}"

    bluezoo_start
    bluezoo_trainer_start
    bluezoo_scan_start
    start_wine
    wait_for_device
    check_trace_failures
    check_trace

    echo "bluetooth: PASSED (winebth used hci0 only, registered its agent and created a device for the" \
        "LE trainer ${BLUEZOO_TRAINER_NAME} ${BLUEZOO_TRAINER_ADDRESS} with its 2 service UUIDs)"
}

main "$@"
