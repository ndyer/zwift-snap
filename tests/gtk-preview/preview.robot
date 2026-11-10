*** Settings ***
Documentation    Exercises the standalone GTK launcher preview, not Wine or the snap.
Resource         kvm.resource
Library          yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library          yarf_frames.py

*** Test Cases ***
Preview Displays Failure And Log And Retries
    Match Text    Zwift installation failed    30
    Match Text    Example installation error    20
    Match Text    Show log    20
    Click Target    Show log
    Match Text    Example installer output    20
    Match Text    Hide log    20
    Click Target    Hide log
    Match Text    Show log    20
    Click Target    Retry
    Match Text    Preview complete    20

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button
