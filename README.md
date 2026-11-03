# Zwift snap

Unofficial snap packaging of [Zwift](https://zwift.com) for Linux. It is based on the
docker image from [netbrain/zwift](https://github.com/netbrain/zwift) and runs Zwift with
wine.

Zwift is proprietary and is **not** included in the snap. It is downloaded from Zwift's
servers into a wine prefix under `$SNAP_USER_COMMON` the first time you run `zwift.install`.

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
zwift.install                     # first run only: wine prefix, winetricks, Zwift
zwift                             # start Zwift
zwift.update                      # update Zwift
```

Other apps: `zwift.wine` and `zwift.winecfg`.

## Environment variables

`ZWIFT_OVERRIDE_RESOLUTION` (for example `1920x1080`), `ZWIFT_NO_GAMEMODE` (default `1`),
`WINE_DISABLE_EGL`, `ZWIFT_PERMISSIVE_UPDATE`, `VERBOSITY` and `DEBUG`.

## Status and limitations

- The snap is `devmode` and `grade: devel`. Strict confinement has not been attempted.
- Only amd64 is supported.
- Not tested: the build with the latest fixes, wine under `$SNAP`, `zwift.install` and gameplay.
- `snap/gui/zwift.png` is a generated placeholder icon.
- Smart trainers and other Bluetooth or ANT+ devices must be connected through the Zwift
  companion app.
- WineHQ removes old builds, so the pinned `wine-devel` version in `snap/snapcraft.yaml`
  will eventually need bumping.
