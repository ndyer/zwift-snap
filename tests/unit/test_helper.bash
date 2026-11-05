# Shared setup for the bats tests of the launcher scripts. Loaded by every
# tests/unit/*.bats file with `load helpers`. The tests run zwift-run.sh with
# fake wine tools and a fake yad (tests/unit/stubs), so no display, wine or network
# is needed and every test is isolated in its own temporary directory.

# shellcheck disable=SC1091
load '/usr/lib/bats/bats-support/load'
# shellcheck disable=SC1091
load '/usr/lib/bats/bats-assert/load'
# shellcheck disable=SC1091
load '/usr/lib/bats/bats-file/load'
# shellcheck disable=SC2093

# The project root, derived from this helper's location.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export ROOT

# new_env: create a fresh fake snap and fake tool bin in BATS_TEST_TMPDIR.
# It sets T, FAKE_SNAP and BIN for the current test. bats removes
# BATS_TEST_TMPDIR after the test, so nothing is left behind.
new_env() {
    T="${BATS_TEST_TMPDIR}"
    FAKE_SNAP="${T}/snap"
    BIN="${T}/bin"
    mkdir -p "${FAKE_SNAP}/bin" "${BIN}" "${T}/data"
    cp "${ROOT}"/launcher/zwift-*.sh "${FAKE_SNAP}/bin/"
    echo "fake" > "${FAKE_SNAP}/bin/runfromprocess-rs.exe"
    local n
    for n in wine wineboot winetricks winepath wineserver wget; do
        cp "${ROOT}/tests/unit/stubs/wine-tools" "${BIN}/${n}"
    done
    cp "${ROOT}/tests/unit/stubs/yad" "${BIN}/yad"
    chmod +x "${BIN}"/*
    : > "${T}/calls.log"
    : > "${T}/yad.log"
}

# run_zwift [VAR=value...]: run zwift-run.sh in the fake environment. The exit
# code is in RC, the output in ${T}/out.log, the launcher log path in
# ZWIFT_LOG_FILE. Deliberately not inside a subshell: the tests read RC.
# shellcheck disable=SC2034  # RC and ZWIFT_LOG_FILE are read by the tests
run_zwift() {
    RC=0
    env -i PATH="${BIN}:/usr/bin:/bin" HOME="${T}" TEST_DIR="${T}" SNAP="${FAKE_SNAP}" \
        SNAP_USER_COMMON="${T}/data" ZWIFT_POLL_INTERVAL=0 DISPLAY=:99 XDG_SESSION_TYPE=x11 \
        YAD="${BIN}/yad" "$@" bash "${FAKE_SNAP}/bin/zwift-run.sh" > "${T}/out.log" 2>&1 \
        || RC=$?
    ZWIFT_LOG_FILE="${T}/data/zwift.log"
}

# count_lines FILE PATTERN: how often PATTERN occurs in FILE.
count_lines() {
    grep -c -- "$2" "$1" 2> /dev/null || true
}

# install_fake_zwift VERSION: put a fake Zwift installation of VERSION into the
# wine prefix of the current test environment.
install_fake_zwift() {
    local version="${1:?}"
    local dir="${T}/data/.wine/drive_c/Program Files (x86)/Zwift"
    mkdir -p "${dir}" "${T}/data/.wine/drive_c/users/tester"
    touch "${dir}/ZwiftApp.exe" "${dir}/ZwiftLauncher.exe"
    echo "<Zwift sversion=\"${version} (1)\"/>" > "${dir}/Zwift_ver_cur.xml"
}

# installed_version: the sversion of the fake installation.
installed_version() {
    grep -oP 'sversion="\K[^ ]+' \
        "${T}/data/.wine/drive_c/Program Files (x86)/Zwift/Zwift_ver_cur.xml"
}
