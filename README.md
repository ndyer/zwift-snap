# Zwift snap

Unofficial snap packaging of [Zwift](https://zwift.com) for Linux.

Zwift is proprietary and is **not** included in the snap. It is downloaded from Zwift's
servers into a wine prefix under `$SNAP_USER_COMMON` the first time you run `zwift`.

Every `zwift` run first makes sure Zwift is installed and up to date, with a Rust/GTK4 progress window.
The window always shows a web panel along its bottom, which starts on Zwift's launcher page and
switches to the sign-in page once the launcher reaches its login gate.
The game starts only after login succeeds. The first run installs wine prerequisites and Zwift
without installer prompts, which takes several minutes. Later runs check for a newer Zwift
version and update it when needed. If the check or update fails you can still launch the
installed version after logging in. The output of all steps is in `$SNAP_USER_COMMON/zwift.log`;
the error dialogs have a `Show log` button.

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

## Environment variables

- `ZWIFT_DISPLAY_MODE` — shared launcher/game display mode, either `fullscreen` (default) or a resolution like `1920x1080`. A custom resolution implies windowed mode (fullscreen disabled).
- `ZWIFT_NO_GAMEMODE` (default `1`)
- `WINE_DISABLE_EGL`
- `ZWIFT_NO_GUI` (set to `1` for no dialogs, only terminal output)
- `ZWIFT_BLUETOOTH_DEBUG` (set to `1` to enable Wine Bluetooth traces for hardware testing)
- `VERBOSITY`

Display preferences are added when missing, including on a fresh install. If an
existing preferences file cannot be updated, the launcher reports an error instead
of starting with the wrong display mode.

To test a real Bluetooth device, run `ZWIFT_BLUETOOTH_DEBUG=1 zwift`. This enables Wine's
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

Development and testing guidance is in [AGENTS.md](AGENTS.md).
