# shellcheck shell=bash
# Test scenarios, sourced by run-tests.sh which provides new_env, run_zwift and check.

echo "version comparison"
(
    # shellcheck source=launcher/zwift-setup.sh
    source "${ROOT}/launcher/zwift-setup.sh" 2> /dev/null
    r=0
    version_ge 1.125.0 1.124.0 || r=1 # newer
    version_ge 1.124.0 1.124.0 || r=1 # equal
    version_ge 1.123.9 1.124.0 && r=1 # older
    version_ge 1.9.0 1.10.0 && r=1    # numeric order, not alphabetical
    version_ge none 1.124.0 && r=1    # not installed
    version_ge "" 1.124.0 && r=1
    version_ge 1.124.0 none || r=1
    exit "${r}"
)
check "version_ge handles older, equal, newer, none" test $? -eq 0

echo "fresh install: install, the launcher downloads Zwift, then launch"
new_env
run_zwift FAKE_GAME_POLLS=2 FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.124.0
check "exit code is 0" test "${RC}" -eq 0
check "wine was initialized" has_line "wineboot -u" "${T}/calls.log"
check "winetricks installed .NET" has_line "winetricks -q dotnet48" "${T}/calls.log"
check "the installer ran silently" has_line "ZwiftSetup.exe /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL" "${T}/calls.log"
check "the latest version was looked up after the install" has_line "Zwift_ver_cur.xml" "${T}/calls.log"
check "the launcher was started" has_line "ZwiftLauncher.exe SilentLaunch" "${T}/calls.log"
check "the download was announced" has_line "progress: #Downloading Zwift 1.124.0" "${T}/yad.log"
check "the installed version is 1.124.0" test "$(installed_version)" = "1.124.0"
check "zwift was started" has_line "runfromprocess-rs.exe 4141 ZwiftApp.exe" "${T}/calls.log"
check "the progress dialog was closed" has_line "progress-closed" "${T}/yad.log"
check "no error dialog" lacks_line "dialog:" "${T}/yad.log"
check "progress text names the steps" has_line "progress: #Installing .NET Framework 4.8" "${T}/yad.log"
check "the log file has the messages" has_line "Zwift started using wine" "${ZWIFT_LOG_FILE}"
check "the installer was asked for a log" has_line "ZwiftSetup.exe .* /LOG=Z:.*ZwiftSetup.log" "${T}/calls.log"
check "the installer log is in the log file" has_line "Fake Inno Setup log" "${ZWIFT_LOG_FILE}"
check "the installer log is on the terminal" has_line "Fake Inno Setup log" "${T}/out.log"
check "the wine Documents folder is private, not a link" test -d "${T}/data/.wine/drive_c/users/tester/Documents" -a ! -L "${T}/data/.wine/drive_c/users/tester/Documents"
check "the folder behind the old link was not touched" has_line "keep" "${T}/hostdocs/keep.txt"

echo "installed with a prefix that links Documents to the home directory"
new_env
install_fake_zwift 1.124.0
mkdir -p "${T}/hostdocs"
echo "keep" > "${T}/hostdocs/keep.txt"
ln -s "${T}/hostdocs" "${T}/data/.wine/drive_c/users/tester/Documents"
run_zwift FAKE_LATEST=1.124.0
check "exit code is 0" test "${RC}" -eq 0
check "the wine Documents folder is private, not a link" test -d "${T}/data/.wine/drive_c/users/tester/Documents" -a ! -L "${T}/data/.wine/drive_c/users/tester/Documents"
check "the folder behind the old link was not touched" has_line "keep" "${T}/hostdocs/keep.txt"
check "the change was logged" has_line "Keeping Documents private" "${ZWIFT_LOG_FILE}"
check "zwift was started" has_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "fresh install: the installer works but the launcher does not download Zwift"
new_env
printf '1\n' > "${T}/yad-answers" # Quit
run_zwift FAKE_LATEST=1.124.0 ZWIFT_DOWNLOAD_ITERATIONS=3
check "exit code is 1" test "${RC}" -eq 1
check "the installer ran" has_line "ZwiftSetup.exe" "${T}/calls.log"
check "the timeout was reported" has_line "Update timed out" "${T}/out.log"
check "the install failed dialog was shown" has_line "dialog: Zwift installation failed" "${T}/yad.log"
check "zwift was not started" lacks_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "fresh install: the latest version cannot be looked up"
new_env
printf '1\n' > "${T}/yad-answers" # Quit
run_zwift FAKE_UPDATE_RESULT=1.124.0
check "exit code is 1" test "${RC}" -eq 1
check "the error was reported" has_line "Zwift cannot be downloaded" "${T}/out.log"
check "zwift was not started" lacks_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "version of a freshly installed launcher (placeholder 0.0.0) is none"
(
    export ZWIFT_INSTALL_DIR
    ZWIFT_INSTALL_DIR="$(mktemp -d)"
    echo '<Zwift version="0.0.0" GAME_URL="x" manifest="Zwift_0.0.0_manifest.xml"/>' > "${ZWIFT_INSTALL_DIR}/Zwift_ver_cur.xml"
    # shellcheck source=launcher/zwift-setup.sh
    source "${ROOT}/launcher/zwift-setup.sh" 2> /dev/null
    [[ "$(get_current_version)" == "none" ]]
)
check "get_current_version gives none for 0.0.0" test $? -eq 0

echo "installed and up to date: no update, launch"
new_env
install_fake_zwift 1.124.0
run_zwift FAKE_LATEST=1.124.0
check "exit code is 0" test "${RC}" -eq 0
check "no install" lacks_line "wineboot" "${T}/calls.log"
check "the version was checked" has_line "Zwift_ver_cur.xml" "${T}/calls.log"
check "zwift was started" has_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "installed version newer than advertised (the 1.125.0 vs 1.124.0 case)"
new_env
install_fake_zwift 1.125.0
run_zwift FAKE_LATEST=1.124.0
check "exit code is 0" test "${RC}" -eq 0
check "no error dialog" lacks_line "dialog:" "${T}/yad.log"
check "no unexpected version error" lacks_line "unexpected version" "${T}/out.log"

echo "update needed and successful"
new_env
install_fake_zwift 1.100.0
run_zwift FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.125.0
check "exit code is 0" test "${RC}" -eq 0
check "the installed version is now 1.125.0" test "$(installed_version)" = "1.125.0"
check "no error dialog" lacks_line "dialog:" "${T}/yad.log"
check "zwift was started" has_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "update needed but the launcher does not update, user launches anyway"
new_env
install_fake_zwift 1.100.0
printf '10\n' > "${T}/yad-answers" # Launch anyway
run_zwift FAKE_LATEST=1.124.0 FAKE_LAUNCHER_STAYS=1
check "exit code is 0" test "${RC}" -eq 0
check "the update failed dialog was shown" has_line "dialog: Zwift update failed" "${T}/yad.log"
check "zwift was started anyway" has_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "update fails and the user quits"
new_env
install_fake_zwift 1.100.0
printf '1\n' > "${T}/yad-answers" # Quit
run_zwift FAKE_LATEST=1.124.0 FAKE_LAUNCHER_STAYS=1
check "exit code is 1" test "${RC}" -eq 1
check "zwift was not started" lacks_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "update check without network"
new_env
install_fake_zwift 1.100.0
run_zwift
check "exit code is 0" test "${RC}" -eq 0
check "no error dialog" lacks_line "dialog:" "${T}/yad.log"
check "a warning was written" has_line "Unable to check for Zwift updates" "${T}/out.log"
check "zwift was started" has_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "install fails, user quits"
new_env
printf '1\n' > "${T}/yad-answers"
run_zwift FAKE_WINETRICKS_FAIL=1
check "exit code is 1" test "${RC}" -eq 1
check "the install failed dialog was shown" has_line "dialog: Zwift installation failed" "${T}/yad.log"
check "zwift was not started" lacks_line "runfromprocess-rs.exe" "${T}/calls.log"

echo "install fails, user retries, it fails again and the user quits"
new_env
printf '10\n1\n' > "${T}/yad-answers" # Retry, then Quit
run_zwift FAKE_SETUP_FAIL=1
check "exit code is 1" test "${RC}" -eq 1
check "wine was initialized twice" count_is "wineboot -u" "${T}/calls.log" 2
check "the progress dialog was reopened" count_is "progress-open" "${T}/yad.log" 2

echo "show log button"
new_env
printf '20\n1\n' > "${T}/yad-answers" # Show log, then Quit
run_zwift FAKE_WINETRICKS_FAIL=1
check "the log was shown" has_line "text-info: Zwift log" "${T}/yad.log"
check "exit code is 1" test "${RC}" -eq 1

echo "no GUI available"
new_env
run_zwift ZWIFT_NO_GUI=1 FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.124.0
check "exit code is 0" test "${RC}" -eq 0
check "yad was not used" test ! -s "${T}/yad.log"
check "install steps still ran" has_line "ZwiftSetup.exe" "${T}/calls.log"

echo "install progress: the bar follows the size of drive_c"
new_env
run_zwift FAKE_LATEST=1.124.0 FAKE_UPDATE_RESULT=1.124.0 FAKE_WINETRICKS_GROW=1 \
    ZWIFT_EXPECTED_DRIVE_C_BYTES=102400 ZWIFT_PROGRESS_INTERVAL=1
check "exit code is 0" test "${RC}" -eq 0
# during the install the sampler (not the step text) owns the percentage: percent
# lines rise while the fake winetricks grows drive_c. The final 100 that closes
# the dialog is not a sample and is excluded from the checks.
(
    percents="$(grep -oP 'progress: \K[0-9]+(?=$)' "${T}/yad.log")"
    percents="$(printf '%s\n' "${percents}" | awk '$1 < 100')"
    first="$(printf '%s\n' "${percents}" | head -1)"
    last="$(printf '%s\n' "${percents}" | tail -1)"
    [[ -n ${first} && ${last} -gt ${first} ]]
)
check "the sampled percentage rises" test $? -eq 0
(
    percents="$(grep -oP 'progress: \K[0-9]+(?=$)' "${T}/yad.log")"
    percents="$(printf '%s\n' "${percents}" | awk '$1 < 100')"
    [[ -z "$(printf '%s\n' "${percents}" | awk 'NR>1 && $1 < prev {print "down"; exit} {prev=$1}')" ]]
)
check "the sampled percentage is monotonic" test $? -eq 0
check "the step texts were still shown" has_line "progress: #Installing .NET Framework 4.8" "${T}/yad.log"
check "percent milestones were logged" has_line "Install progress:" "${ZWIFT_LOG_FILE}"
check "no sampler is left running" test -z "$(pgrep -f progress_sampler_loop 2> /dev/null)"

echo "install progress: a failed install stops the sampler too"
new_env
printf '1\n' > "${T}/yad-answers" # Quit
run_zwift FAKE_WINETRICKS_FAIL=1 FAKE_WINETRICKS_GROW=1 \
    ZWIFT_EXPECTED_DRIVE_C_BYTES=102400 ZWIFT_PROGRESS_INTERVAL=1
check "exit code is 1" test "${RC}" -eq 1
check "the progress dialog was opened before the failure" has_line "progress-open" "${T}/yad.log"
check "the sampler did not outlive the install" test -z "$(pgrep -f progress_sampler_loop 2> /dev/null)"
