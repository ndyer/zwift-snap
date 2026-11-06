#!/usr/bin/env bats
# Tests of the fresh installation flow: winetricks prerequisites, the silent
# Zwift installer, the launcher download and the dialogs around failures.
# Ported from the previous harness (tests/scenarios.sh), see AGENTS.md.

load test_helper

@test "fresh install: install, the launcher downloads Zwift, then launch" {
    new_env
    run_zwift FAKE_GAME_POLLS=3 FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.124.0
    assert_equal "${RC}" 0
    assert_file_contains "${T}/calls.log" "wineboot -u"
    assert_file_contains "${T}/calls.log" "winetricks -q dotnet48"
    assert_file_contains "${T}/calls.log" \
        "ZwiftSetup.exe /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL"
    assert_file_contains "${T}/calls.log" "Zwift_ver_cur.xml"
    assert_file_contains "${T}/calls.log" "ZwiftLauncher.exe SilentLaunch"
    assert_file_contains "${T}/yad.log" "progress: #Downloading Zwift 1.124.0"
    assert_equal "$(installed_version)" "1.124.0"
    assert_file_contains "${T}/calls.log" "runfromprocess-rs.exe 4141 ZwiftApp.exe"
    assert_file_contains "${T}/yad.log" "progress-closed"
    assert_file_not_contains "${T}/yad.log" "dialog:"
    assert_file_contains "${T}/yad.log" "progress: #Installing .NET Framework 4.8"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Zwift started using wine"
    # the installer was asked for a log, the log went to the launcher log and terminal
    assert_file_contains "${T}/calls.log" "ZwiftSetup.exe .* /LOG=Z:.*ZwiftSetup.log"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Fake Inno Setup log"
    assert_file_contains "${T}/out.log" "Fake Inno Setup log"
    # the wine Documents folder is private, not a link, and the folder behind the
    # old link was not touched
    assert [ -d "${T}/data/.wine/drive_c/users/tester/Documents" ]
    assert [ ! -L "${T}/data/.wine/drive_c/users/tester/Documents" ]
    assert_file_contains "${T}/hostdocs/keep.txt" "keep"
}

@test "Bluetooth diagnostics are disabled by default" {
    new_env
    install_fake_zwift 1.124.0
    run_zwift FAKE_LATEST=1.124.0
    assert_equal "${RC}" 0
    assert_file_contains "${T}/wine-debug.log" "WINEDEBUG=<unset>"
    assert_file_not_contains "${T}/wine-debug.log" "fixme-all"
    assert_file_not_contains "${T}/out.log" "fake Bluetooth runtime trace"
    assert_file_not_contains "${ZWIFT_LOG_FILE}" "fake Bluetooth runtime trace"
}

@test "Bluetooth diagnostics are opt-in and saved in the launcher log" {
    new_env
    install_fake_zwift 1.124.0
    run_zwift ZWIFT_BLUETOOTH_DEBUG=1 WINEDEBUG=warn,+timestamp FAKE_LATEST=1.124.0
    assert_equal "${RC}" 0
    assert_file_contains "${T}/wine-debug.log" "WINEDEBUG=warn,+timestamp,+bluetooth,+winebth"
    assert_file_contains "${T}/out.log" \
        "winebth: fake Bluetooth trace during prefix setup (warn,+timestamp,+bluetooth,+winebth)"
    assert_file_contains "${ZWIFT_LOG_FILE}" \
        "winebth: fake Bluetooth trace during prefix setup (warn,+timestamp,+bluetooth,+winebth)"
    assert_file_contains "${T}/out.log" \
        "winebth: fake Bluetooth runtime trace (warn,+timestamp,+bluetooth,+winebth)"
    assert_file_contains "${ZWIFT_LOG_FILE}" \
        "winebth: fake Bluetooth runtime trace (warn,+timestamp,+bluetooth,+winebth)"
}

@test "fresh install: a prefix that links Documents to the home directory is privatized" {
    new_env
    install_fake_zwift 1.124.0
    mkdir -p "${T}/hostdocs"
    echo "keep" > "${T}/hostdocs/keep.txt"
    ln -s "${T}/hostdocs" "${T}/data/.wine/drive_c/users/tester/Documents"
    run_zwift FAKE_LATEST=1.124.0
    assert_equal "${RC}" 0
    assert [ -d "${T}/data/.wine/drive_c/users/tester/Documents" ]
    assert [ ! -L "${T}/data/.wine/drive_c/users/tester/Documents" ]
    assert_file_contains "${T}/hostdocs/keep.txt" "keep"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Keeping Documents private"
    assert_file_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "fresh install: the installer works but the launcher does not download Zwift" {
    new_env
    printf '1\n' > "${T}/yad-answers" # Quit
    run_zwift FAKE_LATEST=1.124.0 ZWIFT_DOWNLOAD_ITERATIONS=3
    assert_equal "${RC}" 1
    assert_file_contains "${T}/calls.log" "ZwiftSetup.exe"
    assert_file_contains "${T}/out.log" "Update timed out"
    assert_file_contains "${T}/yad.log" "dialog: Zwift installation failed"
    assert_file_not_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "fresh install: the latest version cannot be looked up" {
    new_env
    printf '1\n' > "${T}/yad-answers" # Quit
    run_zwift FAKE_UPDATE_RESULT=1.124.0
    assert_equal "${RC}" 1
    assert_file_contains "${T}/out.log" "Zwift cannot be downloaded"
    assert_file_not_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "install fails, user quits" {
    new_env
    printf '1\n' > "${T}/yad-answers"
    run_zwift FAKE_WINETRICKS_FAIL=1
    assert_equal "${RC}" 1
    assert_file_contains "${T}/yad.log" "dialog: Zwift installation failed"
    assert_file_not_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "install fails, user retries, it fails again and the user quits" {
    new_env
    printf '10\n1\n' > "${T}/yad-answers" # Retry, then Quit
    run_zwift FAKE_SETUP_FAIL=1
    assert_equal "${RC}" 1
    assert_equal "$(count_lines "${T}/calls.log" 'wineboot -u')" 2
    assert_equal "$(count_lines "${T}/yad.log" 'progress-open')" 2
}

@test "show log button" {
    new_env
    printf '20\n1\n' > "${T}/yad-answers" # Show log, then Quit
    run_zwift FAKE_WINETRICKS_FAIL=1
    assert_file_contains "${T}/yad.log" "text-info: Zwift log"
    assert_equal "${RC}" 1
}

@test "no GUI available" {
    new_env
    run_zwift ZWIFT_NO_GUI=1 FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.124.0
    assert_equal "${RC}" 0
    assert [ ! -s "${T}/yad.log" ]
    assert_file_contains "${T}/calls.log" "ZwiftSetup.exe"
}
