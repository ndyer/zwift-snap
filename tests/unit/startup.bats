#!/usr/bin/env bats

load test_helper

@test "startup: progress stays open until the first Zwift window" {
    new_env
    install_fake_zwift 1.125.0
    run_zwift FAKE_LATEST=1.125.0 FAKE_GAME_POLLS=4
    assert_equal "${RC}" 0
    assert_file_contains "${T}/calls.log" "window check"
    assert_file_not_contains "${T}/calls.log" "dialog closed before window"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Zwift window is visible"
    assert_file_contains "${T}/yad.log" "progress-closed"
}

@test "startup: window timeout warns and leaves the game running" {
    new_env
    install_fake_zwift 1.125.0
    run_zwift FAKE_LATEST=1.125.0 FAKE_GAME_POLLS=4 FAKE_WINDOW_RESULT=2
    assert_equal "${RC}" 0
    assert_file_contains "${ZWIFT_LOG_FILE}" "Timed out waiting for the Zwift window"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Zwift closed, exiting"
}

@test "startup: window detection failure reports an error" {
    new_env
    install_fake_zwift 1.125.0
    run_zwift FAKE_LATEST=1.125.0 FAKE_GAME_POLLS=4 FAKE_WINDOW_RESULT=1
    assert_equal "${RC}" 1
    assert_file_contains "${T}/yad.log" "dialog: Zwift failed to start"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Failed to show the Zwift window"
}

@test "startup: game exits before window detection begins" {
    new_env
    install_fake_zwift 1.125.0
    run_zwift FAKE_LATEST=1.125.0 FAKE_GAME_POLLS=2
    assert_equal "${RC}" 1
    assert_file_contains "${ZWIFT_LOG_FILE}" "Zwift exited before its window appeared"
    assert_file_contains "${T}/yad.log" "dialog: Zwift failed to start"
    assert_file_not_contains "${T}/calls.log" "window check"
}

@test "startup: terminal-only launch does not wait for a window" {
    new_env
    install_fake_zwift 1.125.0
    run_zwift FAKE_LATEST=1.125.0 ZWIFT_NO_GUI=1
    assert_equal "${RC}" 0
    assert_file_not_contains "${T}/calls.log" "window check"
}
