#!/usr/bin/env bash
# Tests for the launcher scripts. They run zwift-run.sh with a fake yad and stubbed wine tools,
# so no display, wine or network is needed. Usage: tests/run-tests.sh
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT
failures=0
checks=0

check() {
    local description="${1:?}"
    shift
    checks=$((checks + 1))
    if "$@"; then
        echo "  ok: ${description}"
    else
        echo "  FAILED: ${description}"
        failures=$((failures + 1))
    fi
}

has_line() { grep -q -- "$1" "$2" 2> /dev/null; }
lacks_line() { ! grep -q -- "$1" "$2" 2> /dev/null; }
count_is() { [[ "$(grep -c -- "$1" "$2" 2> /dev/null)" -eq "$3" ]]; }

# new_env: create a fresh fake snap in $T. It sets T, FAKE_SNAP and BIN.
new_env() {
    T="$(mktemp -d)"
    FAKE_SNAP="${T}/snap"
    BIN="${T}/bin"
    mkdir -p "${FAKE_SNAP}/bin" "${BIN}" "${T}/data"
    cp "${ROOT}"/launcher/zwift-*.sh "${FAKE_SNAP}/bin/"
    echo "fake" > "${FAKE_SNAP}/bin/runfromprocess-rs.exe"
    local n
    for n in wine wineboot winetricks winepath wineserver wget; do
        cp "${ROOT}/tests/stubs/wine-tools" "${BIN}/${n}"
    done
    cp "${ROOT}/tests/stubs/yad" "${BIN}/yad"
    chmod +x "${BIN}"/*
    : > "${T}/calls.log"
    : > "${T}/yad.log"
}

# run_zwift [VAR=value...]: run zwift-run.sh, the exit code is in RC and the output in $T/out.log
run_zwift() {
    env -i PATH="${BIN}:/usr/bin:/bin" HOME="${T}" TEST_DIR="${T}" SNAP="${FAKE_SNAP}" \
        SNAP_USER_COMMON="${T}/data" ZWIFT_POLL_INTERVAL=0 DISPLAY=:99 XDG_SESSION_TYPE=x11 \
        YAD="${BIN}/yad" "$@" bash "${FAKE_SNAP}/bin/zwift-run.sh" > "${T}/out.log" 2>&1
    RC=$?
    ZWIFT_LOG_FILE="${T}/data/zwift.log"
}

install_fake_zwift() {
    local version="${1:?}"
    local dir="${T}/data/.wine/drive_c/Program Files (x86)/Zwift"
    mkdir -p "${dir}" "${T}/data/.wine/drive_c/users/tester"
    touch "${dir}/ZwiftApp.exe" "${dir}/ZwiftLauncher.exe"
    echo "<Zwift sversion=\"${version} (1)\"/>" > "${dir}/Zwift_ver_cur.xml"
}

installed_version() {
    grep -oP 'sversion="\K[^ ]+' "${T}/data/.wine/drive_c/Program Files (x86)/Zwift/Zwift_ver_cur.xml"
}

# shellcheck source=tests/scenarios.sh
source "${ROOT}/tests/scenarios.sh"

echo
echo "${checks} checks, ${failures} failed"
[[ ${failures} -eq 0 ]]
