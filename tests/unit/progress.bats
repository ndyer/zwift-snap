#!/usr/bin/env bats
# Tests of the install progress sampler, which shows the size of the wine
# prefix as the progress bar while a fresh install runs. Ported from the
# previous harness (tests/scenarios.sh), see AGENTS.md.

load test_helper

# Collect the sampled percentages of the progress dialog: every numeric line
# the fake yad received, without the final 100 that closes the dialog.
sampled_percentages() {
    grep -oP 'progress: \K[0-9]+(?=$)' "${T}/yad.log" | awk '$1 < 100'
}

@test "install progress: the bar follows the size of drive_c" {
    new_env
    run_zwift FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.124.0 FAKE_WINETRICKS_GROW=1 \
        ZWIFT_EXPECTED_DRIVE_C_BYTES=102400 ZWIFT_PROGRESS_INTERVAL=1
    assert_equal "${RC}" 0

    # during the install the sampler (not the step text) owns the percentage:
    # percent lines rise while the fake winetricks grows drive_c
    local percents first last
    percents="$(sampled_percentages)"
    first="$(head -n1 <<< "${percents}")"
    last="$(tail -n1 <<< "${percents}")"
    assert_not_equal "${percents}" ""
    [ "${last}" -gt "${first}" ]

    # the percentage is monotonic and capped at 99
    assert_equal "$(printf '%s\n' "${percents}" | awk 'NR>1 && $1 < prev {print "down"; exit} {prev=$1}')" ""
    assert_equal "$(printf '%s\n' "${percents}" | awk '$1 > 99')" ""

    # the step texts were still shown, and the milestone went into the log
    assert_file_contains "${T}/yad.log" "progress: #Installing .NET Framework 4.8"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Install progress:"

    # no sampler is left running
    assert_equal "$(pgrep -fc progress_sampler_loop 2> /dev/null || true)" "0"
}

@test "install progress: a failed install stops the sampler too" {
    new_env
    printf '1\n' > "${T}/yad-answers" # Quit
    run_zwift FAKE_WINETRICKS_FAIL=1 FAKE_WINETRICKS_GROW=1 \
        ZWIFT_EXPECTED_DRIVE_C_BYTES=102400 ZWIFT_PROGRESS_INTERVAL=1
    assert_equal "${RC}" 1
    assert_file_contains "${T}/yad.log" "progress-open"
    assert_equal "$(pgrep -fc progress_sampler_loop 2> /dev/null || true)" "0"
}
