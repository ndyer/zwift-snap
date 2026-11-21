*** Settings ***
Documentation    Exercises the account picker against two remembered accounts.
Resource         kvm.resource
Library          yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library          yarf_frames.py

*** Test Cases ***
Picker Shows Remembered Accounts, Removes One And Launches The Rest
    # Two seeded accounts: the first is the last used one and shows imperial
    # stats, the second is metric. Both cards must be on screen.
    Match Text    Choose a rider    45
    Match Text    Offline Tester    20
    Match Text    Second Rider    20
    # The first account's chips: 2444 m of climbing and 834 minutes ridden.
    Match Text    Elevation 8,018 ft    20
    Match Text    Time 13h    20
    # Remove a card via its top-right remove icon and confirm the dialog. Both
    # cards carry identical remove icons, so the first match is what gets removed.
    Click Target    ${CURDIR}/remove.png
    Match Text    Remove this account?    20
    Click Target    Remove account
    # The grid is rebuilt once the account is gone; let it repaint before the
    # next click is measured against it.
    Sleep    1s
    # The whole card is the launch target now, so click whichever rider survived
    # the removal (they are not identical, so the launch takes the remaining one).
    ${tester}=    Run Keyword And Return Status    Click Target    Offline Tester
    IF    not ${tester}    Click Target    Second Rider

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button