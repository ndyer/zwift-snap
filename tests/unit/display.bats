#!/usr/bin/env bats

load test_helper

setup() {
    new_env
    install_fake_zwift 1.125.0
    PREFS="${T}/data/.wine/drive_c/users/tester/AppData/Local/Zwift/prefs.xml"
    mkdir -p "$(dirname "${PREFS}")"
}

assert_display_prefs() {
    python3 - "${PREFS}" "$@" <<'PY'
import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
config = root.find("CONFIG")
assert config is not None
assert len(config.findall("FULLSCREEN")) == 1
assert config.findtext("FULLSCREEN") == sys.argv[2]
if len(sys.argv) > 3:
    assert len(config.findall("USER_RESOLUTION_PREF")) == 1
    assert config.findtext("USER_RESOLUTION_PREF") == sys.argv[3]
PY
}

@test "display: fullscreen expands a fresh install self-closing CONFIG" {
    printf '<ZWIFT>\r\n  <CONFIG/>\r\n</ZWIFT>\r\n' > "${PREFS}"
    run_zwift FAKE_LATEST=1.125.0
    assert_equal "${RC}" 0
    assert_display_prefs 1
    assert_file_contains "${ZWIFT_LOG_FILE}" "Set Zwift display mode to fullscreen"
}

@test "display: fullscreen creates a missing CONFIG" {
    printf '<ZWIFT><WORKOUTS><USE_ERG>1</USE_ERG></WORKOUTS></ZWIFT>\n' > "${PREFS}"
    run_zwift FAKE_LATEST=1.125.0
    assert_equal "${RC}" 0
    assert_display_prefs 1
    assert_file_contains "${PREFS}" "<USE_ERG>1</USE_ERG>"
}

@test "display: fullscreen preserves existing settings and resolution on repeated runs" {
    printf '<ZWIFT><CONFIG><FULLSCREEN>0</FULLSCREEN><USER_RESOLUTION_PREF>1920x1080</USER_RESOLUTION_PREF><LANGUAGE>en</LANGUAGE></CONFIG></ZWIFT>\n' > "${PREFS}"
    run_zwift FAKE_LATEST=1.125.0
    assert_equal "${RC}" 0
    assert_display_prefs 1 1920x1080
    run_zwift FAKE_LATEST=1.125.0 FAKE_GAME_POLLS=20
    assert_equal "${RC}" 0
    assert_display_prefs 1 1920x1080
    assert_file_contains "${PREFS}" "<LANGUAGE>en</LANGUAGE>"
}

@test "display: custom resolution adds missing fullscreen and resolution preferences" {
    printf '<ZWIFT><CONFIG /></ZWIFT>\n' > "${PREFS}"
    run_zwift FAKE_LATEST=1.125.0 ZWIFT_DISPLAY_MODE=1920x1080
    assert_equal "${RC}" 0
    assert_display_prefs 0 1920x1080
}

@test "display: custom resolution updates existing preferences" {
    printf '<ZWIFT><CONFIG><FULLSCREEN>1</FULLSCREEN><USER_RESOLUTION_PREF>1280x720</USER_RESOLUTION_PREF></CONFIG></ZWIFT>\n' > "${PREFS}"
    run_zwift FAKE_LATEST=1.125.0 ZWIFT_DISPLAY_MODE=1920x1080
    assert_equal "${RC}" 0
    assert_display_prefs 0 1920x1080
}

@test "display: invalid preferences fail without replacing the file or starting Zwift" {
    printf '<ZWIFT><CONFIG>\n' > "${PREFS}"
    cp "${PREFS}" "${T}/original"
    run_zwift FAKE_LATEST=1.125.0
    assert_equal "${RC}" 1
    cmp "${PREFS}" "${T}/original"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Unable to update display mode"
    assert_file_contains "${T}/yad.log" "dialog: Zwift preferences"
    assert_file_not_contains "${T}/calls.log" "runfromprocess-rs.exe"
}

@test "display: invalid resolution is reported without changing preferences" {
    printf '<ZWIFT><CONFIG/></ZWIFT>\n' > "${PREFS}"
    cp "${PREFS}" "${T}/original"
    run_zwift FAKE_LATEST=1.125.0 ZWIFT_DISPLAY_MODE=garbage
    assert_equal "${RC}" 1
    cmp "${PREFS}" "${T}/original"
    assert_file_contains "${ZWIFT_LOG_FILE}" "Invalid display mode"
}
