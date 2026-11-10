*** Settings ***
Documentation    Wait for the preview's pending installation decision.
Resource         kvm.resource
Library          yarf_ocr.py
# saves every frame the suite captures when YARF_FRAME_DIR is set, for debugging failures
Library          yarf_frames.py

*** Test Cases ***
Preview Waits For An Installation Decision
    Match Text    Zwift installation failed    30
