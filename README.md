# Zwift snap

Unofficial snap packaging of [Zwift](https://zwift.com) for Linux.

[![snapcraft.io](https://snapcraft.io/zwift/badge.svg)](https://snapcraft.io/zwift)

Zwift is proprietary and is **not** included in the snap.

When you run `zwift` it will run a launcher which:
* Installs Wine prerequisites.
* Checks for Zwift updates and downloads from the CDN servers.
* Displays the Zwift sign-in page, or allows you to select cached rider profiles.
* Allows you to configure some game and debugging options.
* Launches the game.

## Installation

Install the snap from the store, then connect its graphics and Bluetooth interfaces:

```shell
sudo snap install --beta zwift
sudo snap connect zwift:bluez
zwift
```

To get faster heart-rate data from a Bluetooth LE heart-rate belt (e.g. a Wahoo TICKR),
optionally connect the privileged Bluetooth control interface. This is strictly opt-in: it
tightens the belt's connection interval from its battery-saving default (~1 s) to ~15 ms,
which uses more power, so it is never connected automatically:

```shell
sudo snap connect zwift:bluetooth-control
```

## Troubleshooting

The snap uses strict confinement. To debug confinement problems, install the local snap
with `--devmode` and watch the logged violations with `snappy-debug`.

Sound goes through the `audio-playback` interface (PulseAudio or PipeWire on the host). To
debug audio run `WINEDEBUG=+pulse,+mmdevapi zwift`.

## Settings

The launcher's user-facing options live in a settings file at
`~/snap/zwift/common/settings.json`. Edit the file directly or
use the **Settings** button in the launcher window.

## Limitations

- Only amd64 is supported.
- Bluetooth smart trainers are experimental. A patched version of Wine is used that addresses
  some integration issues; however, this has only been tested with a limited selection of real
  hardware. If a device does not show up, you may be able to pair it through the Zwift companion app.

## Alternatives

There are several other ways to run Zwift on Linux. The table below is a snapshot and may be out
of date — see [.kb/alternatives-comparison.md](.kb/alternatives-comparison.md) for how it is kept
current.

| | [netbrain/zwift](https://github.com/netbrain/zwift) | [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap) | [linuxundich/tretlager](https://github.com/linuxundich/tretlager) | This snap |
| --- | --- | --- | --- | --- |
| Packaging | Docker/Podman image | Native (AUR, APT, DNF, .deb/.rpm) | Flatpak bundle | Snap (strict) |
| Written in | Shell | Rust (CLI) | Python (PySide6/Kirigami) | Rust (GTK4 GUI) |
| Wine | Bundled in image | System wine-staging | Built inside the Flatpak (WoW64) | Built from source (patched fork) |
| Game files | Pre-installed image | Zwift CDN | Official `ZwiftSetup.exe` | Zwift CDN |
| Windows launcher + .NET/WebView2 | Yes | No | Yes | No |
| Bluetooth sensors | Companion app | BlueZ (pairing screen) | Built-in BLE bridge (DIRCON) or Companion app | Patched Wine BLE |
| ANT+ | No | Yes | No | No |
| Sign-in | In-game | In-game | Web sign-in + keyring | Launcher + per-account sessions |
| Sandbox | Container | None | Flatpak | Strict snap |

The defining choice this snap shares with `zwift-bootstrap` is to fetch the game straight from
Zwift's CDN manifest and never run the Windows launcher (which needs .NET and WebView2). It
differs from the others in two ways: it ships a Wine fork with Bluetooth support baked in, so the
game talks BLE directly rather than through a companion app or a separate bridge process, and it
runs under strict snap confinement on `core26`.

## Development

Development and testing guidance lives in the knowledge base: start at
[AGENTS.md](AGENTS.md) and read [.kb/agents.md](.kb/agents.md).

A [workshop](https://ubuntu.com/workshop) environment is used for building and testing.

## License

The launcher and snap packaging in this repository are licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Zwift itself is proprietary and is not covered by these licenses; it is downloaded
separately at runtime.
