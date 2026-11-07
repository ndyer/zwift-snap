*** Settings ***
Documentation     Visual smoke test of the GTK Rust launcher and real Zwift.
Resource          kvm.resource
Library           yarf_ocr.py

*** Variables ***
${INSTALL_TIMEOUT}      2400
${DIALOG_TIMEOUT}       60

*** Test Cases ***
Launcher shows its progress window
    [Documentation]    Require the GTK launcher, not merely a successful game start.
    Match Text    Show log    ${DIALOG_TIMEOUT}

Zwift installation reaches the mandatory login gate
    [Documentation]    Stop before game launch; no real account or token is used.
    Match Text    Login to Zwift    ${INSTALL_TIMEOUT}
    Match Text    Login is required    ${DIALOG_TIMEOUT}
