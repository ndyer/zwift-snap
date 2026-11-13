*** Settings ***
Documentation     Visual smoke test of the GTK Rust launcher and real Zwift.
Resource          kvm.resource
Library           yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library           yarf_frames.py

*** Variables ***
${INSTALL_TIMEOUT}      2400
${DIALOG_TIMEOUT}       60

*** Test Cases ***
Launcher shows its progress window
    [Documentation]    Require the GTK launcher, not merely a successful game start.
    Match Text    Settings    ${DIALOG_TIMEOUT}

Zwift install and start completes
    [Documentation]    Require Zwift's sign-up screen; do not enter credentials.
    Match Text    SIGN UP    ${INSTALL_TIMEOUT}