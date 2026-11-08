#!/usr/bin/env bats
# Unit tests for the Zwift download cache helper the smoke test uses
# (tests/tools/dl-cache/dl-cache.sh).

load test_helper

# the helper is a sourced script, so it expects the caller to provide die(), as
# tests/smoke.sh and tests/dl-cache.sh do
die() {
    echo "dl-cache: ${*}" >&2
    exit 1
}

setup() {
    # the helper derives every path from HOME, so give it a throwaway one
    export HOME="${BATS_TEST_TMPDIR}/home"
    mkdir -p "${HOME}"
    # shellcheck source=tests/tools/dl-cache/dl-cache.sh
    source "${ROOT}/tests/tools/dl-cache/dl-cache.sh"
}

# write_install DIR VERSION: a minimal Zwift installation of VERSION in DIR, using
# the Zwift_ver_cur_filename.txt indirection the real installation has
write_install() {
    local dir="${1:?}" version="${2:?}"
    mkdir -p "${dir}/assets"
    printf 'Zwift_ver_cur.7.xml\0' > "${dir}/Zwift_ver_cur_filename.txt"
    printf '<Zwift version="1.0.7" sversion="%s (7)"/>\n' "${version}" > "${dir}/Zwift_ver_cur.7.xml"
    printf 'launcher\n' > "${dir}/ZwiftApp.exe"
    printf 'game data\n' > "${dir}/assets/data.bin"
}

# live_install_dir: the installation directory inside the fake wine prefix
live_install_dir() {
    printf '%s' "${DL_CACHE_PREFIX}/${DL_CACHE_GAME_SUBDIR}"
}

@test "an empty cache is reported as absent" {
    run dl_cache_present
    assert_failure
}

@test "the version follows the Zwift_ver_cur_filename.txt indirection" {
    local game="${BATS_TEST_TMPDIR}/game"
    write_install "${game}" "1.125.0"
    run dl_cache_version "${game}"
    assert_success
    assert_output "1.125.0"
}

@test "the version is empty when no version file is present" {
    mkdir -p "${BATS_TEST_TMPDIR}/empty"
    run dl_cache_version "${BATS_TEST_TMPDIR}/empty"
    assert_success
    assert_output ""
}

@test "capture records the installed version and its game tree" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    run dl_cache_present
    assert_success
    run dl_cache_version "${DL_CACHE_GAME}"
    assert_output "1.5.0"
    assert [ -f "${DL_CACHE_GAME}/assets/data.bin" ]
    assert [ -f "${DL_CACHE_META}" ]
}

@test "capture is skipped when the cache already holds the version" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    # a marker that must survive the skipped second capture
    rm -f "${DL_CACHE_GAME}/ZwiftApp.exe"
    run dl_cache_capture
    assert_success
    assert_output --partial "not capturing"
    assert [ ! -f "${DL_CACHE_GAME}/ZwiftApp.exe" ]
}

@test "capture --force replaces a cache holding the same version" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    rm -f "${DL_CACHE_GAME}/ZwiftApp.exe"
    run dl_cache_capture --force
    assert_success
    assert [ -f "${DL_CACHE_GAME}/ZwiftApp.exe" ]
}

@test "stage restores the game tree into an empty prefix and keeps modification times" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    touch -t 202001020304.05 "${DL_CACHE_GAME}/assets/data.bin"
    local expected
    expected="$(stat -c %Y "${DL_CACHE_GAME}/assets/data.bin")"
    # wipe the prefix the way `snap remove --purge zwift` would
    rm -rf "${DL_CACHE_PREFIX}"
    run dl_cache_stage
    assert_success
    assert_output --partial "restoring Zwift 1.5.0"
    assert [ -f "$(live_install_dir)/ZwiftApp.exe" ]
    assert [ -f "$(live_install_dir)/assets/data.bin" ]
    run stat -c %Y "$(live_install_dir)/assets/data.bin"
    assert_output "${expected}"
}

@test "stage leaves a prefix that already holds the cached version alone" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    # a locally changed file that a skipped restore must not overwrite
    printf 'local change\n' > "$(live_install_dir)/ZwiftApp.exe"
    run dl_cache_stage
    assert_success
    assert_output --partial "already holds Zwift 1.5.0"
    run cat "$(live_install_dir)/ZwiftApp.exe"
    assert_output "local change"
}

@test "stage --force restores over an existing prefix" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    printf 'local change\n' > "$(live_install_dir)/ZwiftApp.exe"
    run dl_cache_stage --force
    assert_success
    run cat "$(live_install_dir)/ZwiftApp.exe"
    assert_output "launcher"
}

@test "stage fails when the cache is empty" {
    run dl_cache_stage
    assert_failure
    assert_output --partial "the Zwift download cache is empty"
}

@test "capture fails when nothing is installed" {
    run dl_cache_capture
    assert_failure
    assert_output --partial "no installed Zwift to capture"
}

@test "stage and capture carry the winetricks downloads" {
    write_install "$(live_install_dir)" "1.5.0"
    mkdir -p "${DL_CACHE_WINETRICKS_LIVE}"
    printf 'installer\n' > "${DL_CACHE_WINETRICKS_LIVE}/dotnet48.exe"
    run dl_cache_capture
    assert_success
    assert [ -f "${DL_CACHE_WINETRICKS}/dotnet48.exe" ]
    rm -rf "${DL_CACHE_WINETRICKS_LIVE}"
    run dl_cache_stage
    assert_success
    assert [ -f "${DL_CACHE_WINETRICKS_LIVE}/dotnet48.exe" ]
}

@test "clear removes the cache" {
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    run dl_cache_clear
    assert_success
    assert [ ! -d "${DL_CACHE_DIR}" ]
}

@test "status describes an empty cache and a populated one" {
    run dl_cache_status
    assert_success
    assert_output --partial "empty"
    write_install "$(live_install_dir)" "1.5.0"
    run dl_cache_capture
    assert_success
    run dl_cache_status
    assert_success
    assert_output --partial "Zwift version 1.5.0"
}
