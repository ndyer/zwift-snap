*** Settings ***
Documentation    Drives the real Rust lifecycle against stub Wine commands.
Resource         kvm.resource
Library          yarf_ocr.py

*** Test Cases ***
Real Lifecycle Shows Output And Accepts Launch Anyway
    Match Text    Zwift update failed    30
    Match Text    Show log    20
    Click Target    Show log
    Match Text    updater diagnostic    20
    Click Target    Launch anyway

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button
