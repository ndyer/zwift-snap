# Yarf smoke test

`tests/smoke.sh` runs the visual smoke test in the workshop: it starts a Mir
compositor, runs the zwift launcher against it and drives the `yarf` suite in this
directory (`smoke.robot`). The suite checks on screen that the launcher progress
dialog appears and that Zwift gets installed and started by requiring the `SIGN UP`
text on screen. A returning profile that opens on `LOG IN` will not pass this check.

The first run installs the wine prerequisites and downloads Zwift into the wine
prefix, which takes tens of minutes. Later runs reuse the prefix and are quick.

## Templates

`zwift-dialog-title.png` matches the progress dialog's static title bar. The game
screen is checked with OCR (`Match Text`), not an image template.
