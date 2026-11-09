*** Settings ***
Documentation    Drives the real Rust lifecycle against stub Wine commands.
Resource         kvm.resource
Library          yarf_ocr.py

*** Test Cases ***
Real Lifecycle Shows Output And Waits At The Login Gate
    Match Text    Log in to Zwift    45
    Match Text    Complete the login below    20
    Click Target    Show log
    Match Text    already installed    20

*** Keywords ***
Click Target
    [Arguments]    ${target}
    Move Pointer To ${target}
    Sleep    100ms
    Press LEFT Button
    Sleep    100ms
    Release LEFT Button
