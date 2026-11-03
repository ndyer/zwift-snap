# Zwift snap

Unofficial snap packaging of [Zwift](https://zwift.com) for Linux. It is based on the
docker image from [netbrain/zwift](https://github.com/netbrain/zwift) and runs Zwift with
wine.

Zwift is proprietary and is **not** included in the snap. It is downloaded from Zwift's
servers into a wine prefix under `$SNAP_USER_COMMON` the first time you run `zwift`.

Every `zwift` run first makes sure Zwift is installed and up to date, with a progress window
(yad), and then starts it. The first run installs wine prerequisites and Zwift without any
prompts, which takes several minutes. Later runs check for a newer Zwift version and update
it when needed. If the check or update fails you can still launch the installed version.
The output of all steps is in `$SNAP_USER_COMMON/zwift.log`; the error dialogs have a
`Show log` button.

There is no automatic login. Log in through the Zwift window.

## Build

The snap is built in destructive mode inside a [Workshop](https://ubuntu.com/workshop/docs/)
workshop (Ubuntu 24.04).

```shell
workshop launch zwift-snap
workshop run zwift-snap setup     # installs snapcraft (snap store), shellcheck, yamllint
workshop run zwift-snap build     # sudo snapcraft pack --destructive-mode
workshop run zwift-snap lint      # shellcheck + yamllint
```

Destructive mode installs build packages (including the WineHQ apt repository) into the
workshop itself, so use a throwaway workshop.

## Install and run

```shell
workshop run zwift-snap install   # sudo snap install --dangerous --devmode ./zwift_*.snap
sudo snap install mesa-2404       # graphics userspace, if not installed already
sudo snap connect zwift:gpu-2404 mesa-2404:gpu-2404
zwift                             # installs or updates Zwift when needed, then starts it
```

Other apps: `zwift.wine` and `zwift.winecfg`.

If the installation fails, the error dialog offers `Retry`. Sound goes through
the `audio-playback` interface (PulseAudio or PipeWire on the host). To debug audio run
`WINEDEBUG=+pulse,+mmdevapi zwift`.

## Environment variables

`ZWIFT_OVERRIDE_RESOLUTION` (for example `1920x1080`), `ZWIFT_NO_GAMEMODE` (default `1`),
`WINE_DISABLE_EGL`, `ZWIFT_NO_GUI` (set to `1` for no dialogs, only terminal output), `VERBOSITY`
and `DEBUG`.

## Status and limitations

- The snap is `devmode` and `grade: devel`. Strict confinement has not been attempted.
- Only amd64 is supported.
- The wine part prunes man pages, locales, package docs (copyright files are kept) and the sane
  scanner backends. `usr/lib/*/samba` must stay, `ntlm_auth` needs it.
- Not tested: the build with the latest fixes, wine under `$SNAP`, the first-run installation, the update, the dialogs and gameplay.
- `snap/gui/zwift.png` is a generated placeholder icon.
- The snap plugs the `bluez` interface for Bluetooth devices, but that is untested. The
  `joystick` interface is not plugged, so game controllers are not available. Smart trainers
  and ANT+ devices must be connected through the Zwift companion app.
- WineHQ removes old builds, so the pinned `wine-devel` version in `snap/snapcraft.yaml`
  will eventually need bumping.
