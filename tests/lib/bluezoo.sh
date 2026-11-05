#!/usr/bin/env bash
# Shared helper to run the bluezoo BlueZ D-Bus mock (https://github.com/Samsung/BlueZoo)
# for the zwift tests. bluezoo takes the place of bluetoothd and owns org.bluez on the
# workshop system bus, so the confined snap reaches it through its bluez plug.
#
# Sourced by tests/bluetooth.sh and tests/pairing.sh. The caller must, before sourcing:
#   - set OUT_DIR (a writable directory for bluezoo.log and scan.log) and ROOT (the repo root),
#   - provide a `die MESSAGE` function and a `connected PLUG` function (true when the plug of the
#     zwift snap is connected).
# It sets bluezoo_pid and scan_pid for the caller's cleanup trap; call bluezoo_stop from there.

# hci0 is the radio winebth attaches to, hci1 acts as the peer device that gets discovered
# (see tests/bluetooth-scan.py).
readonly BLUEZOO_CENTRAL_ADAPTER="/org/bluez/hci0"
readonly BLUEZOO_PEER_ADAPTER="/org/bluez/hci1"
readonly BLUEZOO_PEER_ADDRESS="00:00:00:22:22:22"
# used by callers (bluetooth.sh), not inside this helper
# shellcheck disable=SC2034
readonly BLUEZOO_PEER_DEVICE_NAME="dev_00_00_00_22_22_22"
readonly BLUEZOO_START_TIMEOUT=20
# the scan client stays connected this long; the caller's test must finish well within it
BLUEZOO_KEEPALIVE_SECONDS="${BLUEZOO_KEEPALIVE_SECONDS:-120}"

bluezoo_pid=""
scan_pid=""

# bluezoo_wait_for TIMEOUT DESCRIPTION COMMAND [ARGS...]: run the command every second until it
# succeeds, or die after TIMEOUT seconds
bluezoo_wait_for() {
    local timeout="${1:?}" description="${2:?}"
    shift 2
    local waited=0
    until "$@" > /dev/null 2>&1; do
        ((waited += 1))
        if [[ ${waited} -ge ${timeout} ]]; then
            die "${description} did not happen within ${timeout}s"
        fi
        sleep 1
    done
}

# bluezoo_ensure_prerequisites: the bluez plug connected and bluetoothd out of the way
bluezoo_ensure_prerequisites() {
    command -v bluezoo > /dev/null 2>&1 \
        || die "bluezoo is not installed, it is set up by the SDK setup-base hook (recreate the workshop)"
    command -v bluetoothd > /dev/null 2>&1 \
        || die "the bluez package is not installed, it is set up by the SDK setup-base hook (recreate the workshop)"

    # bluez is manual-connect-only, but connecting it is the point of these tests, same as the
    # gpu-2604 handling in tests/smoke.sh
    if ! connected bluez; then
        sudo snap connect zwift:bluez
    fi

    # bluetoothd must not own org.bluez; the SDK setup-base hook masks it, but this workshop may
    # be older than that hook change. Masking is not enough on its own: the bluez package ships a
    # D-Bus activation file that points at /bin/false and makes clients fail with a confusing
    # systemd error the first time they hit an unowned org.bluez name. Remove it (it belongs to
    # the bluez package, so it comes back on a package update; this is a test container).
    if systemctl is-enabled bluetooth.service > /dev/null 2>&1; then
        sudo systemctl mask --now bluetooth.service
    fi
    if [[ -f /usr/share/dbus-1/system-services/org.bluez.service ]]; then
        sudo rm -f /usr/share/dbus-1/system-services/org.bluez.service
    fi
}

bluezoo_name_owned() {
    busctl list --no-legend 2> /dev/null | grep -qw org.bluez
}

bluezoo_adapters_up() {
    gdbus introspect --system --dest org.bluez --object-path /org/bluez \
        2> /dev/null | grep -q hci0 \
        && gdbus introspect --system --dest org.bluez --object-path /org/bluez \
            2> /dev/null | grep -q hci1
}

bluezoo_start() {
    # A run that died hard leaves bluezoo behind; it must be gone before the new one can own
    # org.bluez.
    sudo pkill -f "bluezoo --auto-enable" > /dev/null 2>&1
    sleep 1

    # bluezoo must run as root: the bluez D-Bus policy only lets root own org.bluez. setsid
    # detaches it from this script's process group so a cancelled caller does not take it down
    # with it. The log is written by the root shell inside the sudo command, because a redirect
    # on the outside would belong to the calling user's sudo wrapper.
    sudo setsid sh -c "exec bluezoo --auto-enable --scan-interval=1 \
        -a 00:00:00:11:11:11 -a ${BLUEZOO_PEER_ADDRESS} \
        > '${OUT_DIR}/bluezoo.log' 2>&1" < /dev/null &
    bluezoo_pid=$!

    bluezoo_wait_for "${BLUEZOO_START_TIMEOUT}" "bluezoo taking over org.bluez" bluezoo_name_owned
    bluezoo_wait_for "${BLUEZOO_START_TIMEOUT}" "the mock adapters hci0 and hci1 coming up" bluezoo_adapters_up
}

bluezoo_scan_start() {
    # The scan client must stay connected or bluezoo stops scanning; it also makes the peer
    # adapter discoverable (see tests/bluetooth-scan.py).
    /opt/bluezoo/bin/python "${ROOT}/tests/bluetooth-scan.py" \
        "${BLUEZOO_CENTRAL_ADAPTER}" "${BLUEZOO_PEER_ADAPTER}" \
        --timeout "${BLUEZOO_KEEPALIVE_SECONDS}" \
        > "${OUT_DIR}/scan.log" 2>&1 &
    scan_pid=$!
    bluezoo_wait_for 10 "the scan client starting the discovery" \
        grep -q "discovery started" "${OUT_DIR}/scan.log"
}

# bluezoo_stop: for the caller's cleanup trap. Stops the scan client and bluezoo.
bluezoo_stop() {
    [[ -z ${scan_pid} ]] || kill "${scan_pid}" 2> /dev/null
    [[ -z ${bluezoo_pid} ]] || {
        sudo kill "${bluezoo_pid}" 2> /dev/null
        sudo pkill -f "bluezoo --auto-enable" > /dev/null 2>&1
    }
}
