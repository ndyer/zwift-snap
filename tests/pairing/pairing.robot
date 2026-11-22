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
# how long to wait for a screen that must appear
${SCREEN_TIMEOUT}    240
# how long the pairing dialog may take to list the trainer (bluezoo reports it every second)
${DEVICE_TIMEOUT}    60
# a returning profile reconnects its saved trainer on its own within this time
${RECONNECT_TIMEOUT}    20
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
    ...    click can be lost (the press and release land between two game frames, or the tile is
    ...    still settling), so retry and confirm the dialog opened by its CLOSE button, which the
    ...    pairing screen behind it does not have. The dialog's own title is not usable: OCR reads
    ...    it as PAIRED DEVICES, the screen behind it.
    FOR    ${attempt}    IN RANGE    ${CLICK_ATTEMPTS}
        Sleep    1s
        Click Zwift Target    POWER SOURCE
        ${open}=    Run Keyword And Return Status    Match Text    CLOSE    5
        IF    ${open}    RETURN
        Log    the pair-device dialog did not open yet, clicking POWER SOURCE again
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
