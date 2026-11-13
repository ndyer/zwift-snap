# Zwift snap

Unofficial snap packaging of [Zwift](https://zwift.com) for Linux.

[![snapcraft.io](https://snapcraft.io/zwift/badge.svg)](https://snapcraft.io/zwift)

Zwift is proprietary and is **not** included in the snap. It is downloaded from Zwift's
servers into a wine prefix under `$SNAP_USER_COMMON` the first time you run `zwift`.

Every `zwift` run first makes sure Zwift is installed and up to date, with a Rust/GTK4 progress window.
The window always shows a web panel along its bottom, which starts on Zwift's launcher page and
switches to the sign-in page once the launcher reaches its login gate. On the very first run the
panel shows Zwift's registration page instead. Links in the panel open in your normal browser,
not inside the panel.
The game starts only after login succeeds. The first run installs wine prerequisites and Zwift
without installer prompts, which takes several minutes. Later runs check for a newer Zwift
version and update it when needed. If the check or update fails you can still launch the
installed version after logging in. The output of all steps is in `$SNAP_USER_COMMON/zwift.log`;
the window's `Settings` dialog has a `Show log` button that shows the same output in-app.

Login uses OAuth with PKCE and validates the returned state. The token is saved with owner-only
permissions in `~/snap/zwift/common/pairing-token.json` and passed to the game at launch time.
Treat this file as a credential; do not share it in logs or bug reports. Each graphical launch
still requires signing in; an existing token file does not bypass that gate.

https://snapcraft.io/zwift

## Install and run

Install a local build of the snap, then connect its graphics and Bluetooth interfaces:

```shell
sudo snap install --beta zwift
sudo snap connect zwift:bluez
zwift                             # installs or updates Zwift when needed, then starts it
```

To get faster heart-rate data from a Bluetooth LE heart-rate belt (e.g. a Wahoo TICKR),
optionally connect the privileged Bluetooth control interface. This is strictly opt-in: it
tightens the belt's connection interval from its battery-saving default (~1 s) to ~15 ms,
which uses more power, so it is never connected automatically:

```shell
sudo snap connect zwift:bluetooth-control
```

The progress dialog stays open until Zwift shows its first window (including splash
screens), rather than closing as soon as the process starts. If no window appears
within two minutes, the launcher logs a warning and leaves Zwift running.

To debug confinement problems, install the local snap with `--devmode` and watch the
logged violations with `snappy-debug`.

The snap has no `home` plug. The folders of the wine user (Desktop, Documents, Downloads, Music,
Pictures, Videos) are plain directories in the wine prefix, so the Zwift files are in
`~/snap/zwift/common/.wine/drive_c/users/<user>/`. The launcher replaces the links that wine
creates to the real home directory, at every start.

If the installation fails, the error dialog offers `Retry`.

Sound goes through the `audio-playback` interface (PulseAudio or PipeWire on the host). To
debug audio run `WINEDEBUG=+pulse,+mmdevapi zwift`.

## Settings

The launcher's user-facing options live in a settings file at
`~/snap/zwift/common/settings.json`. On first run the launcher copies the
read-only default from the snap (`$SNAP/etc/zwift/settings.json`) into place; edit
the file directly or use the **Settings** button in the launcher window. The
available keys are:

- `display_mode` — shared launcher/game display mode, either `fullscreen` (default) or a resolution like `1920x1080`. A custom resolution implies windowed mode (fullscreen disabled).
- `graphics_profile` — Zwift graphics profile to use: `auto` (default), `basic`, `medium`, `high`, `ultra` or `default`.
- `disable_egl` — set to `true` to disable EGL in Wine.
- `bluetooth_debug` — set to `true` to enable Wine Bluetooth traces for hardware testing.
- `verbosity` — logging verbosity, `0` (quiet) to `3` (debug); default `1`.

The Settings dialog also reports the status of the Zwift account token and the
`bluez` plug connection, can run the graphics probe, and offers separate actions to
reset the Wine prefix and to delete the downloaded game data.

Display preferences are added when missing, including on a fresh install. If an
existing preferences file cannot be updated, the launcher reports an error instead
of starting with the wrong display mode.

## Environment variables

These remain available for debugging and for callers that cannot edit the settings
file; prefer the settings file for normal use.

- `ZWIFT_NO_GUI` (set to `1` for no dialogs, only terminal output)
- `ZWIFT_BT_INTERVAL` (the BLE connection interval to request in 1.25 ms units, default `12` = 15 ms; only applies with `bluetooth-control` connected)
- `ZWIFT_BT_TIMEOUT` (the BLE supervision timeout in 10 ms units, default `200` = 2 s)
- `ZWIFT_BT_RESEND_MS` (how long the interval override waits before re-applying, in ms, default `4000`)

## Graphics profile

Zwift picks its graphics profile from an internal GPU table that rates many cards below
what they can render. The launcher can override that. With `graphics_profile` set to
`auto` (the default) it measures the GPU at launch and writes the matching profile; a
fixed value (`basic`, `medium`, `high`, `ultra`) skips the measurement, and `default`
leaves Zwift's own files alone so its GPU table decides again.


The profiles are the game's own `data/configs/*.txt` files. Before overwriting them the
launcher saves a copy of each next to the prefix in `~/snap/zwift/common/graphics/` and
writes the chosen profile's content into *all four* files, so it does not matter which one
the game picks. A patcher update that replaces a file is recognised and its saved copy
refreshed. The choice is logged as `Graphics profile: ...` in `zwift.log`; if the GPU test
cannot run (no GPU access, for example) the launcher logs a warning and leaves the files to
the game.

To test a real Bluetooth device, set `bluetooth_debug` to `true` in the settings
file (or use the Settings dialog) and launch Zwift. This enables Wine's
`+bluetooth` and `+winebth` channels and writes the runtime trace to
`~/snap/zwift/common/zwift.log` (as well as the terminal). Bluetooth diagnostics are off by
default; the log is rotated to `zwift.log.1` at the start of the next run.

Rust captures child-process output and sends it directly to the log file, terminal,
and in-app log view; the GUI does not tail a file. Closing the launcher cancels its
work and stops its Wine prefix. Without a display (or with `ZWIFT_NO_GUI=1`), it
runs in terminal mode: installation errors quit and update errors permit launching
an installed copy. Terminal mode has no embedded-browser login gate and retains the game's
own login flow (or accepts explicitly supplied game arguments).

## Status and limitations

- The snap uses strict confinement. If something is blocked, install with `--devmode`
  and report the denials from `snappy-debug`.
- Only amd64 is supported.
- Bluetooth smart trainers are experimental: the snap's Wine implements the BLE scanning
  and GATT support Zwift uses, and Zwift discovers, pairs and reads power from a mocked
  BLE trainer in testing, but real trainers are not yet verified. Only Bluetooth LE
  devices are found (no classic Bluetooth). Game controllers are not available. If a
  device does not show up, pair it through the Zwift companion app.

Development and testing guidance lives in the knowledge base: start at
[AGENTS.md](AGENTS.md) and read [.kb/agents.md](.kb/agents.md).

## License

The launcher and snap packaging in this repository are licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Zwift itself is proprietary and is not covered by these licenses; it is downloaded
separately at runtime.

