#!/usr/bin/env bats
# Tests for the version helpers of the launcher. Sourced from
# tests/scenarios.sh of the previous harness; see AGENTS.md for the layout.

load test_helper

@test "version_ge accepts newer, equal, older, none" {
    # shellcheck source=launcher/zwift-setup.sh
    source "${ROOT}/launcher/zwift-setup.sh" 2> /dev/null

    run version_ge 1.125.0 1.124.0 # newer
    assert_success

    run version_ge 1.124.0 1.124.0 # equal
    assert_success

    run version_ge 1.123.9 1.124.0 # older
    assert_failure

    run version_ge 1.9.0 1.10.0 # numeric order, not alphabetical
    assert_failure

    run version_ge none 1.124.0 # not installed
    assert_failure

    run version_ge "" 1.124.0 # empty
    assert_failure

    run version_ge 1.124.0 none
    assert_success
}

@test "get_current_version gives none for the 0.0.0 placeholder" {
    ZWIFT_INSTALL_DIR="$(mktemp -d)"
    echo '<Zwift version="0.0.0" GAME_URL="x" manifest="Zwift_0.0.0_manifest.xml"/>' \
        > "${ZWIFT_INSTALL_DIR}/Zwift_ver_cur.xml"

    # shellcheck source=launcher/zwift-setup.sh
    source "${ROOT}/launcher/zwift-setup.sh" 2> /dev/null
    run get_current_version
    assert_output "none"
}
