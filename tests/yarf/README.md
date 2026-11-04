# Yarf smoke test

`tests/smoke.sh` runs the visual smoke test in the workshop: it starts a Mir
compositor, runs the zwift launcher against it and drives the `yarf` suite in this
directory (`smoke.robot`). The suite checks on screen that the launcher progress
dialog appears and that Zwift gets installed and started.

The first run installs the wine prerequisites and downloads Zwift into the wine
prefix, which takes tens of minutes. Later runs reuse the prefix and are quick.

## Templates

`zwift-login.png` is the template of the Zwift window that the suite waits for. It
is captured once per machine (window decorations and theme of the compositor differ
between machines) and must be committed.

To capture it:

1. Run `workshop run zwift-snap-core26 smoke` and wait until it fails or passes
   without the template (the suite logs a warning when the template is missing).
2. While the Zwift window is on screen, run `yarf --platform Mir` in the workshop
   against the staged suite (`~/zwift-smoke-suite`) to open the interactive
   console, then `Grab Templates zwift-login` and draw a box around a distinctive,
   static part of the window (the login panel, not an animation).
3. Copy the saved template into this directory and commit it.

Alternatively grab a full screenshot with the `Grab Screenshot` keyword and crop it
with any image editor. Keep the template small: large templates make the matching
slow and brittle.
