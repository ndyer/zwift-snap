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

## Build

The snap is built in destructive mode inside a [Workshop](https://ubuntu.com/workshop/docs/)
workshop (Ubuntu 26.04). The snap uses `core26`.

```shell
workshop launch zwift-snap
workshop run zwift-snap build     # sudo snapcraft pack --destructive-mode
workshop run zwift-snap test      # unit tests of the launcher scripts (bats)
workshop run zwift-snap smoke     # visual smoke test with yarf, see below
workshop run zwift-snap lint      # shellcheck + yamllint
workshop run zwift-snap size      # where the bytes are, for pruning work
workshop run zwift-snap stats     # pkgproxy cache usage
```

## Visual smoke test

The `smoke` action runs the launcher for real and verifies on screen that Zwift gets
installed and started, with [yarf](https://documentation.ubuntu.com/yarf) driving a
headless (or host-display) Mir compositor and checking the pixels with OCR and
template matching. The suite is in `tests/smoke`, the harness in `tests/smoke.sh`.
Extra arguments after `smoke` are passed to `yarf`, for example
`workshop run zwift-snap smoke --variant light`.

The harness installs the `yarf` and `mir-test-tools` snaps and connects
`zwift:gpu-2604` to `mesa-2604` when needed. With a display available through the
workshop desktop interface (`workshop connect zwift-snap-sdk:desktop`) the
Mir compositor renders in a window on the host screen, otherwise it runs on a
headless virtual output (set `SMOKE_DISPLAY=nested|headless` to force either).

The first run installs wine prerequisites and downloads Zwift into the wine prefix,
which takes tens of minutes; later runs reuse the prefix and take a couple of
minutes. The suite checks that the launcher progress dialog appears and that the
Zwift window finally comes up (no login is performed). On failure the yarf report
(`log.html` in `~/zwift-smoke-out`) shows the screenshots it compared, and the
harness prints the last lines of the launcher log.

The `zwift-login.png` template in `tests/smoke` matches the Zwift window. Window
decorations differ between compositor setups, so recapture it when the test reports
it missing — the recipe is in `tests/smoke/README.md`.

Setup also installs [pkgproxy](https://snapcraft.io/pkgproxy), a caching proxy, and points
apt and snapd at it. Builds in the same workshop then download each package and snap only
once; repeat downloads come from the local cache (`workshop run zwift-snap stats`
shows the savings). Not cached: the base snap that snapcraft downloads itself,
anything over `https://`, and the git clone of the wine source.

Destructive mode installs build packages into the workshop itself, so use a
throwaway workshop.

## Install and run

```shell
workshop run zwift-snap install   # sudo snap install --dangerous ./zwift_*.snap
sudo snap install mesa-2604       # graphics userspace, if not installed already
sudo snap connect zwift:gpu-2604 mesa-2604:gpu-2604
sudo snap connect zwift:bluez
zwift                             # installs or updates Zwift when needed, then starts it
```

`bluez` is not connected automatically. Other apps: `zwift.wine` and `zwift.winecfg`.

To debug confinement problems, install with `workshop run zwift-snap install --devmode` (the
violations are only logged) and watch them with `snappy-debug`.

The snap has no `home` plug. The folders of the wine user (Desktop, Documents, Downloads, Music,
Pictures, Videos) are plain directories in the wine prefix, so the Zwift files are in
`~/snap/zwift/common/.wine/drive_c/users/<user>/`. The launcher replaces the links that wine
creates to the real home directory, at every start.

If the installation fails, the error dialog offers `Retry`.

Sound goes through the `audio-playback` interface (PulseAudio or PipeWire on the host). To
debug audio run `WINEDEBUG=+pulse,+mmdevapi zwift`.

## Environment variables

`ZWIFT_OVERRIDE_RESOLUTION` (for example `1920x1080`), `ZWIFT_NO_GAMEMODE` (default `1`),
`WINE_DISABLE_EGL`, `ZWIFT_NO_GUI` (set to `1` for no dialogs, only terminal output), `VERBOSITY`
and `DEBUG`.

## Status and limitations

- The snap is `strict` and `grade: devel`. Wine, the first-run installation and audio work under
  strict confinement without the `home`, `hardware-observe` and `process-control` interfaces,
  although `userfaultfd` and `sched_setaffinity` show up as seccomp log entries in `devmode`.
  If something is blocked, install with `--devmode` and report the denials from `snappy-debug`.
- Only amd64 is supported.
- The wine part prunes man pages, locales, package docs (copyright files are kept) and the sane
  scanner backends. `usr/lib/*/samba` must stay, `ntlm_auth` needs it.
- To keep the download small, the build also prunes everything that is not reachable at
  runtime: wine's Windows compiler toolchain (`widl`, `winegcc`, ...) and the ~776 static
  import libraries of `lib/wine` (~100 MB), and — via the DT_NEEDED closure walk in
  `snap/prune-closure.sh` — every library that only the removed build tools, the samba
  utilities beyond `ntlm_auth`, gnupg, perl or gstreamer plugins for neural networks
  (onnx/tensorflow/dnnl), speech synthesis (flite) and similar non-media codecs need
  (~320 MB more). Libraries that are dlopened by name are kept explicitly: gstreamer
  plugins, gdk-pixbuf/gio/gtk modules, gconv and nss modules, the samba tree, and the
  glycin image loaders. A guard in the wine part's `override-prime` fails the build when
  a runtime file is missing (over-pruning) or a build-time file is still primed (stale
  prime directory, needs `workshop run zwift-snap clean`).
- The wine runtime is not split into a separate content snap (like
  [wine-platform-runtime](https://github.com/mmtrt/wine-platform-runtime-core24-snap)):
  that snap ships only the support libraries of old wine builds for core24, not wine
  itself, and its noble libraries do not match this snap's resolute-built wine. Splitting
  would not reduce what users download in total and would add a second snap to publish.
- Not tested: the update, the dialogs after the first run and gameplay (a ride).
- `snap/gui/zwift.png` is a generated placeholder icon.
- Bluetooth does not work in Zwift. With the `bluez` interface connected, wine's Bluetooth
  driver (`winebth`) reaches BlueZ and finds the adapter and the nearby devices
  (`WINEDEBUG=+winebth zwift`), but wine 11.21 does not implement the BLE advertisement watcher
  (`BluetoothLEAdvertisementWatcher`, `Start`, `put_ScanningMode` and `add_Received` are stubs
  that return `E_NOTIMPL`) that Zwift uses to scan for devices. Zwift then reports that the
  Bluetooth receiver is turned off. This is a wine limitation, not a confinement problem. The
  `bluez` plug is kept so Bluetooth can be tried again with a newer wine: check whether
  `adv_watcher_Start` in `dlls/windows.devices.bluetooth/advertisement.c` of wine is still a
  stub. The `joystick` interface is not plugged, so game controllers are not available. Smart
  trainers and ANT+ devices must be connected through the Zwift companion app.
- Wine is built from source: the `wine` part of `snap/snapcraft.yaml` builds
  [ndyer/wine](https://github.com/ndyer/wine) (a fork of the official
  wine-mirror/wine GitHub mirror) at the tag given by `source-tag`. Bump the tag
  to update wine; for snap-specific patches put them on a branch in the fork and
  point the part at it with a pinned commit.
- The build is a new style WoW64 build: there is no `wine64` binary and no 32-bit
  unix libraries, 32-bit Windows programs run through the PE files in
  `lib/wine/i386-windows`. The snap has no i386 packages. The winetricks steps
  (`dotnet48`, `d3dcompiler_47`, `webview2`) have to be checked on a desktop,
  winetricks prints a warning that new style WoW64 is experimental.
