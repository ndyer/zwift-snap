*** Settings ***
Documentation     Drives Zwift from the login form to the device pairing screen, for tests/pairing.sh.
...               The Zwift servers are faked by tests/zwift-stub.py, so no account is used. Every
...               button is found by its on-screen text (OCR), which is resolution independent. The
...               login form is the exception: its Username and Password fields are low-contrast
...               placeholders OCR cannot read, so they are clicked at a fraction of the span between
...               two text anchors ("LOG IN" above and "create new user" below), which also scales
...               with the screen size. tests/pairing.sh confirms the game reached the pairing state.
Resource          kvm.resource

*** Variables ***
${EMAIL}              offline@example.com
${PASSWORD}          offline-test-password
# how long to wait for a screen that must appear
${SCREEN_TIMEOUT}    90
# how long to wait for an optional onboarding screen (skipped on a returning profile)
${OPTIONAL_TIMEOUT}    20
# the login form is reached two ways; poll for each in short steps so neither waits the full
# screen timeout (LOGIN_POLL_STEP seconds per check, up to LOGIN_POLL_ATTEMPTS rounds)
${LOGIN_POLL_STEP}    ${3}
${LOGIN_POLL_ATTEMPTS}    ${15}
# vertical position of each login field as a fraction of the "LOG IN" -> "create new user" span
${USERNAME_FRACTION}    ${0.33}
${PASSWORD_FRACTION}    ${0.53}
${GO_FRACTION}       ${0.85}

*** Test Cases ***
Reach The Pairing Screen
    [Documentation]    Log in through the form, click through the onboarding screens that a new
    ...    profile shows (each is optional, a returning profile skips them) and stop at the pairing
    ...    screen.
    Log In With The Form
    Accept The License Agreement
    Choose The Sport
    Choose The Reason
    Wait For The Pairing Screen

*** Keywords ***
Point Between ${top} And ${bottom} At ${fraction}
    [Documentation]    A point on the vertical line through ${top}, ${fraction} of the way down to
    ...    ${bottom}. ${top} and ${bottom} are (x, y) tuples; returns an (x, y) tuple.
    ${point}=    Evaluate    (${top}[0], int(${top}[1] + ${fraction} * (${bottom}[1] - ${top}[1])))
    RETURN    ${point}

Wait For The Login Form
    [Documentation]    The login form is reached two ways: a first run opens on the SIGN UP screen
    ...    (with a "log in with existing account" link to the form), a returning run opens on the
    ...    LOG IN form directly. Poll for a title unique to each so neither case waits the full
    ...    screen timeout. "SIGN UP" is checked first, because the SIGN UP screen's login link also
    ...    fuzzy-matches "LOG IN". Returns ${True} on the SIGN UP screen, ${False} on the LOG IN form.
    FOR    ${attempt}    IN RANGE    ${LOGIN_POLL_ATTEMPTS}
        ${on_signup}=    Run Keyword And Return Status    Match Text    SIGN UP    ${LOGIN_POLL_STEP}
        IF    ${on_signup}    RETURN    ${True}
        ${on_login}=    Run Keyword And Return Status    Match Text    LOG IN    ${LOGIN_POLL_STEP}
        IF    ${on_login}    RETURN    ${False}
    END
    Fail    neither the SIGN UP nor the LOG IN screen appeared

Log In With The Form
    [Documentation]    Two cases converge on the LOG IN form:
    ...    - a first run opens on the SIGN UP screen, with a link to the login form, and an empty form;
    ...    - a returning run opens on the LOG IN form directly, with the email already filled in.
    ...    So the email is typed only on a first run; the password is always typed (it is never kept).
    ${on_signup}=    Wait For The Login Form
    IF    ${on_signup}
        Click LEFT Button On log in with existing account
        Match Text    LOG IN    ${SCREEN_TIMEOUT}
    END
    ${top}=    Get Position Of LOG IN
    ${bottom}=    Get Position Of create new user
    IF    ${on_signup}
        ${username_field}=    Point Between ${top} And ${bottom} At ${USERNAME_FRACTION}
        Click LEFT Button On ${username_field}
        Type String    ${EMAIL}
    END
    ${password_field}=    Point Between ${top} And ${bottom} At ${PASSWORD_FRACTION}
    Click LEFT Button On ${password_field}
    Type String    ${PASSWORD}
    ${go_button}=    Point Between ${top} And ${bottom} At ${GO_FRACTION}
    Click LEFT Button On ${go_button}

Accept The License Agreement
    ${shown}=    Run Keyword And Return Status    Match Text    LICENSE AGREEMENT    ${OPTIONAL_TIMEOUT}
    IF    ${shown}    Click LEFT Button On Accept

Choose The Sport
    ${shown}=    Run Keyword And Return Status    Match Text    TELL US ABOUT YOURSELF    ${OPTIONAL_TIMEOUT}
    IF    ${shown}
        Click LEFT Button On CYCLIST
        # the OK button only becomes active once a sport is selected
        Sleep    1s
        Click LEFT Button On OK
    END

Choose The Reason
    ${shown}=    Run Keyword And Return Status    Match Text    WHAT BRINGS YOU    ${OPTIONAL_TIMEOUT}
    IF    ${shown}
        Click LEFT Button On RACING
        Sleep    1s
        Click LEFT Button On OK
    END

Wait For The Pairing Screen
    [Documentation]    The pairing screen is titled PAIRED DEVICES.
    Match Text    PAIRED DEVICES    ${SCREEN_TIMEOUT}
