#!/usr/bin/env bats

load test_helper

setup() {
    SNAP="${BATS_TEST_TMPDIR}/snap"
    SNAP_USER_COMMON="${BATS_TEST_TMPDIR}/common"
    export SNAP SNAP_USER_COMMON
    mkdir -p "${SNAP}/usr/share/mime" "${SNAP_USER_COMMON}"
    touch "${SNAP}/usr/share/mime/mime.cache"
}

@test "wine bootstrap exposes staged GIO TLS modules to child processes" {
    run env -u GIO_EXTRA_MODULES bash "${ROOT}/zwsnap-runtime/wine-env.sh" printenv GIO_EXTRA_MODULES
    assert_success
    assert_output "${SNAP}/usr/lib/x86_64-linux-gnu/gio/modules"
}

@test "wine bootstrap preserves extra GIO module paths after staged modules" {
    run env GIO_EXTRA_MODULES=/existing/modules:/other/modules \
        bash "${ROOT}/zwsnap-runtime/wine-env.sh" printenv GIO_EXTRA_MODULES
    assert_success
    assert_output "${SNAP}/usr/lib/x86_64-linux-gnu/gio/modules:/existing/modules:/other/modules"
}
