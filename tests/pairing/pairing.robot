*** Settings ***
Documentation     Pairs the mock trainer on Zwift's device pairing screen, for tests/pairing.sh.
...               The Zwift servers are faked by tests/zwift-stub.py, so no account is used. Every
...               button is found by its on-screen text (OCR), which is resolution independent.
...               The game logs in with a fake token and a pre-onboarded profile. It pairs the mock BLE trainer KICKR
...               (tests/tools/bluetooth/bluetooth-trainer.py) as the power source; tests/pairing.sh confirms the game
...               state and the trainer's GATT traffic from the logs.
Resource          kvm.resource
# caps the threads of the OCR in Match Text, which otherwise runs on every core
Library           yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library           yarf_frames.py

*** Variables ***
# how long to wait for a screen that must appear. Headless (software-composited virtual
# output) takes ~4 minutes just to reach the pairing screen on a slow/busy host, mostly in
# Zwift's POST_LOGIN_INIT, so a 240 s limit times out right at the boundary; nested reaches it
# in seconds and returns as soon as the text matches, so the larger limit costs nothing there.
${SCREEN_TIMEOUT}    600
# how long the pairing dialog may take to list the trainer (bluezoo reports it every second)
${DEVICE_TIMEOUT}    60
# a returning profile reconnects its saved trainer on its own within this time
${RECONNECT_TIMEOUT}    20
# how long to wait for the pair-device dialog (its CLOSE button) once a tile is clicked;
# headless can take many seconds to open it
${DIALOG_TIMEOUT}    30
# how many times to click the POWER SOURCE tile before giving up on the dialog opening
${CLICK_ATTEMPTS}    6
${TRAINER}           KICKR

*** Test Cases ***
Reach The Pairing Screen
    [Documentation]    Token login reaches pairing directly: the stub profile has
    ...    already completed license, sport and goal onboarding.
    Wait For The Pairing Screen
    Pair The Trainer

*** Keywords ***
Click Zwift Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    TRY
        # Zwift can miss a press and release delivered between two game frames.
        Sleep    100ms
    FINALLY
        Release LEFT Button
    END

Wait For The Pairing Screen
    [Documentation]    The pairing screen is titled PAIRED DEVICES, but Zwift can open a "No ANT+
    ...    dongle connected?" tip over the title. Wait for the POWER SOURCE tile instead: it is never
    ...    covered, appears on every pairing layout, and is what Pair The Trainer clicks.
    Match Text    POWER SOURCE    ${SCREEN_TIMEOUT}

Open Device List
    [Documentation]    Click the POWER SOURCE tile until the pair-device dialog opens. A single
    ...    click can be lost (press and release land between two game frames, or the tile is
    ...    still settling), and in headless the dialog can take many seconds to appear, so retry
    ...    until the dialog's CLOSE button shows. Finding the tile label for the click is
    ...    best-effort: once a dialog is opening it covers the label, so OCR may split it (POWE /
    ...    NSOR) or miss it, and that must not abort the wait for CLOSE that follows. The
    ...    dialog's own title is unusable: OCR reads it as PAIRED DEVICES, the screen behind it.
    FOR    ${attempt}    IN RANGE    ${CLICK_ATTEMPTS}
        ${clicked}=    Run Keyword And Return Status    Click Zwift Target    POWER SOURCE
        ${open}=    Run Keyword And Return Status    Match Text    CLOSE    ${DIALOG_TIMEOUT}
        IF    ${open}    RETURN
        Log    the pair-device dialog is not open yet (clicked=${clicked}), clicking POWER SOURCE again
    END
    Fail    the pair-device dialog did not open after clicking POWER SOURCE

Pair The Trainer
    [Documentation]    Pair the mock trainer as the power source. A returning profile reconnects the trainer it
    ...    paired before by itself, and clicking a connected tile would unpair it, so first give
    ...    the reconnect a chance. Clicking the trainer in the dialog toggles it: click it once.
    ...    Open Device List confirms the dialog is up (its CLOSE button) before the trainer is
    ...    clicked, then the trainer is clicked and CLOSE closes the dialog.
    ${reconnected}=    Run Keyword And Return Status    Match Text    ${TRAINER}    ${RECONNECT_TIMEOUT}
    IF    not ${reconnected}
        Open Device List
        Match Text    ${TRAINER}    ${DEVICE_TIMEOUT}
        Click Zwift Target    ${TRAINER}
        Sleep    3s
        Click Zwift Target    CLOSE
    END
    Match Text    CONNECTED    ${SCREEN_TIMEOUT}
