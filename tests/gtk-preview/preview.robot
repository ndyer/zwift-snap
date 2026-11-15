*** Settings ***
Documentation    Exercises the standalone GTK launcher preview, not Wine or the snap.
Resource         kvm.resource
Library          yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library          yarf_frames.py

*** Test Cases ***
Preview Displays Failure And Log And Retries
    Match Text    Zwift installation failed    30
    # The header controls are icon-only now, so match the log toggle template.
    Match    ${CURDIR}/log.png    20
    Click Target    ${CURDIR}/log.png
    # The failure's full reason now lives in the log, not under the heading.
    Match Text    Example installation error    20
    Match Text    Example installer output    20
    Click Target    ${CURDIR}/retry.png
    Match Text    Preview complete    20

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button
