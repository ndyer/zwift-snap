# Zwift snap

Unofficial snap packaging of [Zwift](https://zwift.com) for Linux.

Zwift is proprietary and is **not** included in the snap. It is downloaded from Zwift's
servers into a wine prefix under `$SNAP_USER_COMMON` the first time you run `zwift`.

Every `zwift` run first makes sure Zwift is installed and up to date, with a yad progress window,
and then starts it. The first run installs wine prerequisites and Zwift without any
prompts, which takes several minutes. Later runs check for a newer Zwift version and update
it when needed. If the check or update fails you can still launch the installed version.
The output of all steps is in `$SNAP_USER_COMMON/zwift.log`; the error dialogs have a
`Show log` button.

There is no automatic login. Log in through the Zwift window.

## Install and run

Install a local build of the snap, then connect its graphics and Bluetooth interfaces:

```shell
sudo snap install --dangerous ./zwift_*_amd64.snap
sudo snap install mesa-2604       # graphics userspace, if not installed already
sudo snap connect zwift:gpu-2604 mesa-2604:gpu-2604
sudo snap connect zwift:bluez
zwift                             # installs or updates Zwift when needed, then starts it
```

`bluez` is not connected automatically. Other apps: `zwift.wine` and `zwift.winecfg`.

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

- `ZWIFT_DISPLAY_MODE` — display mode, either `fullscreen` (default) or a resolution like `1920x1080`. A custom resolution implies windowed mode (fullscreen disabled).
- `ZWIFT_NO_GAMEMODE` (default `1`)
- `WINE_DISABLE_EGL`
- `ZWIFT_NO_GUI` (set to `1` for no dialogs, only terminal output)
- `ZWIFT_BLUETOOTH_DEBUG` (set to `1` to enable Wine Bluetooth traces for hardware testing)
- `VERBOSITY`
- `DEBUG`

To test a real Bluetooth device, run `ZWIFT_BLUETOOTH_DEBUG=1 zwift`. This enables Wine's
`+bluetooth` and `+winebth` channels and writes the runtime trace to
`~/snap/zwift/common/zwift.log` (as well as the terminal). Bluetooth diagnostics are off by
default; the log is rotated to `zwift.log.1` at the start of the next run.

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
