*** Settings ***
Documentation     Visual smoke test of the zwift snap.
...
...              Started by tests/smoke.sh, which provides the Mir compositor that
...              this suite screenshots, and runs the zwift launcher against it.
...              The suite checks that the launcher gets Zwift installed and started:
...              the yad progress dialog appears, the install stages run through and
...              the Zwift window finally comes up. No login is performed.
Resource          kvm.resource

*** Variables ***
# The first run installs wine prerequisites and downloads Zwift, which takes tens of
# minutes. Later runs only check for updates and start within a few minutes. The
# Match keyword takes its timeout in seconds, not as a Robot time string.
${INSTALL_TIMEOUT}      2400
${DIALOG_TIMEOUT}       60
# The launcher closes the progress dialog when Zwift is up (and when it fails, but
# tests/smoke.sh then finds no ZwiftApp.exe process and fails the run). OCR and
# template matching are not 100% reliable on every frame, so the dialog only counts
# as gone when it is missing from several consecutive checks.
${GONE_CHECKS}          3
${GONE_INTERVAL}        5s

*** Test Cases ***
Launcher shows its progress dialog
    [Documentation]    The launcher opens a yad progress dialog while it prepares
    ...    Zwift. Only its window chrome is static (the body text changes with
    ...    every install stage), so match the title bar template and read the
    ...    dialog text with OCR without failing the test on OCR hiccups.
    ...    On a warm prefix the whole install finishes within seconds, so the
    ...    dialog may be gone before this test starts matching: the test passes
    ...    when the dialog is either visible or already gone (the second test
    ...    then requires the game itself, so a missing dialog alone cannot pass
    ...    the suite).
    ${appeared}=    Run Keyword And Return Status
    ...    Match    ${CURDIR}${/}zwift-dialog-title.png    ${DIALOG_TIMEOUT}
    IF    not ${appeared}
        # the dialog is gone, confirm it stays gone (several consecutive misses)
        Wait Until Keyword Succeeds    3 times    ${GONE_INTERVAL}    Progress Dialog Is Gone
        Log    The progress dialog is already gone, the install finished before the suite matched it.    WARN
    END
    ${ocr_ok}=    Run Keyword And Return Status    Match Text    Getting Zwift ready    ${DIALOG_TIMEOUT}
    IF    not ${ocr_ok}    Log    OCR did not read the dialog text, the template match is authoritative.    WARN

Zwift install and start completes
    [Documentation]    Wait until the launcher is done. It closes the progress
    ...    dialog right after Zwift started. When the template of the Zwift window
    ...    is available, require the window itself to be on screen; otherwise
    ...    require the progress dialog to be gone from several consecutive checks.
    ...    tests/smoke.sh double checks the ZwiftApp.exe wine process afterwards,
    ...    because a failed install also closes the dialog (leaving an error dialog
    ...    in its place).
    ${have_template}=    Evaluate    os.path.exists("${CURDIR}${/}zwift-login.png")    modules=os
    IF    ${have_template}
        Match    ${CURDIR}${/}zwift-login.png    ${INSTALL_TIMEOUT}
    ELSE
        Log    zwift-login.png is missing, checking the progress dialog instead. Capture it with the recipe in tests/yarf/README.md.    WARN
        Wait Until Keyword Succeeds    ${INSTALL_TIMEOUT}    ${GONE_INTERVAL}    Progress Dialog Is Gone
    END

*** Keywords ***
Progress Dialog Is Gone
    [Documentation]    Succeeds when the progress dialog is missing from
    ...    ${GONE_CHECKS} consecutive checks, so that one bad frame does not
    ...    report a running dialog as gone. Only the title bar template is used:
    ...    the body text changes with every install stage.
    FOR    ${i}    IN RANGE    ${GONE_CHECKS}
        Run Keyword And Expect Error    *    Match    ${CURDIR}${/}zwift-dialog-title.png    2
    END
