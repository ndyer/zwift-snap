*** Settings ***
Documentation    Wait for the preview's pending installation decision.
Resource         kvm.resource
Library          yarf_ocr.py

*** Test Cases ***
Preview Waits For An Installation Decision
    Match Text    Zwift installation failed    30
