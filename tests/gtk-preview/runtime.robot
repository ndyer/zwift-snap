*** Settings ***
Documentation    Drives the real Rust lifecycle against stub Wine commands.
Resource         kvm.resource
Library          yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library          yarf_frames.py

*** Test Cases ***
Real Lifecycle Shows Output And Waits At The Login Gate
    Match Text    Log in to Zwift    45
    Match Text    Complete the login below    20
    Click Target    Settings
    Match Text    Show log    20
    Click Target    Show log
    Match Text    already installed    20
    # Leave the main window focused: this settings dialog is modal and transient,
    # so while it is open it captures the harness's later Alt+F4 and the launcher
    # never closes. Dismiss it here, before the suite ends.
    Match Text    Close    20
    Click Target    Close
    Match Text    Log in to Zwift    20

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button
