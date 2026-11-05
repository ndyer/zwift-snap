#!/usr/bin/env bats
# Tests of the update flow: the version check against the Zwift CDN and what
# happens when the Zwift launcher does not complete an update. Ported from the
# previous harness (tests/scenarios.sh), see AGENTS.md.

load test_helper

@test "installed and up to date: no update, launch" {
    new_env
    install_fake_zwift 1.124.0
    run_zwift FAKE_LATEST=1.124.0
    assert_equal "${RC}" 0
    assert_file_not_contains "${T}/calls.log" "wineboot"
    assert_file_contains "${T}/calls.log" "Zwift_ver_cur.xml"
    assert_file_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "installed version newer than advertised (the 1.125.0 vs 1.124.0 case)" {
    new_env
    install_fake_zwift 1.125.0
    run_zwift FAKE_LATEST=1.124.0
    assert_equal "${RC}" 0
    assert_file_not_contains "${T}/yad.log" "dialog:"
    assert_file_not_contains "${T}/out.log" "unexpected version"
}

@test "update needed and successful" {
    new_env
    install_fake_zwift 1.100.0
    run_zwift FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.125.0
    assert_equal "${RC}" 0
    assert_equal "$(installed_version)" "1.125.0"
    assert_file_not_contains "${T}/yad.log" "dialog:"
    assert_file_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "update needed but the launcher does not update, user launches anyway" {
    new_env
    install_fake_zwift 1.100.0
    printf '10\n' > "${T}/yad-answers" # Launch anyway
    run_zwift FAKE_LATEST=1.124.0 FAKE_LAUNCHER_STAYS=1
    assert_equal "${RC}" 0
    assert_file_contains "${T}/yad.log" "dialog: Zwift update failed"
    assert_file_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "update fails and the user quits" {
    new_env
    install_fake_zwift 1.100.0
    printf '1\n' > "${T}/yad-answers" # Quit
    run_zwift FAKE_LATEST=1.124.0 FAKE_LAUNCHER_STAYS=1
    assert_equal "${RC}" 1
    assert_file_not_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "update check without network" {
    new_env
    install_fake_zwift 1.100.0
    run_zwift
    assert_equal "${RC}" 0
    assert_file_not_contains "${T}/yad.log" "dialog:"
    assert_file_contains "${T}/out.log" "Unable to check for Zwift updates"
    assert_file_contains "${T}/calls.log" "runfromprocess-rs.exe"
}
