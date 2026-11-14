*** Settings ***
Documentation    Drives the real Rust lifecycle against stub Wine commands.
Resource         kvm.resource
Library          yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library          yarf_frames.py

*** Test Cases ***
Real Lifecycle Shows Output And Waits At The Login Gate
    # The fixture's install is already current, so the update transfers nothing
    # and the launcher settles at its login gate. The launcher shows no heading of
    # its own there, so the gate is proved by its WebView: the embed page is only
    # a marketing slider, and its login form ("Remember me") appears solely on the
    # sign-in page the gate navigates to.
    Match Text    Settings    45
    Match Text    Remember me    60
    Click Target    Settings
    Match Text    Show log    20
    Click Target    Show log
    Match Text    already installed    20
    # Leave the main window focused: this settings dialog is modal and transient,
    # so while it is open it captures the harness's later Alt+F4 and the launcher
    # never closes. Dismiss it here, before the suite ends.
    Match Text    Close    20
    Click Target    Close
    Match Text    Remember me    20

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button
