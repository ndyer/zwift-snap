# AGENTS.md

Guidance for AI coding agents working in this repository.

## What this project is

An unofficial snap packaging of [Zwift](https://zwift.com) for Linux. Zwift itself is
proprietary and NOT part of the snap — the launcher scripts download and update it
into a wine prefix at runtime. Wine is built from source by the snap's `wine` part:
[ndyer/wine](https://github.com/ndyer/wine), a fork of the official wine-mirror/wine
GitHub mirror, at the `source-branch` in `snap/snapcraft.yaml` (currently
`zwift-radios-rebased`: wine-11.21 plus BLE support — advertisement watcher, GATT,
Windows.Devices.Radios; new-style WoW64, no i386 host libs). Strict confinement, amd64 only, base `core26`.
The old `core24` variant of the snap is gone from the tree; it only lives in git history (pre-`6e31f55` commits
or the `origin/core26` / `v0.11` refs).

## Layout

```
launcher/          source of the launcher scripts (zwift-run/gui/setup/common.sh, wine-env.sh)
snap/snapcraft.yaml     the snap definition (parts, prune filters, plugs, layouts)
snap/prune-closure.sh   DT_NEEDED closure walk that prunes unreachable libraries
tests/             unit tests (bats in tests/unit, stubs in tests/unit/stubs), the visual smoke
                   test (tests/smoke.sh + tests/smoke/), the bluetooth plumbing test
                   (tests/bluetooth.sh), the pairing-screen test (tests/pairing.sh +
                   tests/pairing/ + tests/zwift-stub.py)
  tools/bluetooth/   bluezoo mock tooling shared by both bluetooth tests: the helper bluezoo.sh,
                   the RSSI wrapper bluezoo-run.py, the scan client bluetooth-scan.py and the
                   BLE trainer mock bluetooth-trainer.py
  tools/yarf/        yarf tooling: the screenshot/input debugging tool yarf.sh (+ screenshot.py,
                   input.py) and the OCR thread cap yarf_ocr.py used by the robot suites
  .workshop/         workshop definition (build environment) + in-project SDK zwift-snap-sdk
overlay/, prime/, stage/, parts/   build trees, gitignored, never edit by hand
```

The snap has three apps: `zwift` (the launcher), `wine`, `winecfg`; the last two let
users run arbitrary wine commands. All share the command-chain
`gpu-2604-wrapper -> wine-env.sh`.

## Workshop

All building and testing happens in a [Workshop](https://ubuntu.com/workshop/docs/)
environment. Use `workshop run <action> [arguments...]`; `workshop --help` lists
actions. The local runner's header at `.workshop/zwift-snap-sdk/workshop` documents
its behavior and limitations.

There is no `setup` action: snapcraft, shellcheck, yamllint, bats, bluez, bluezoo and
the pkgproxy cache are installed by the `setup-base` hook of the `zwift-snap-sdk` SDK
at launch and refresh (see `.workshop/zwift-snap-sdk/hooks/setup-base`).

The canonical agent documentation for operating Workshop is the vendored skill under
`.github/skills/` (`use-workshop`, `onboard-workshop`, `design-sdk`, imported from
github.com/canonical/use-workshop-skill). Route Workshop CLI questions there — in
particular `.github/skills/use-workshop/references/in-project-sdk.md` for the SDK
layout and hook rules used by this project.

The workshop definition is `.workshop/zwift-snap.yaml`; actions are plain bash
with `errexit`/`pipefail` set. In-project SDKs are listed with a `project-` prefix in
`sdks:` (`project-zwift-snap-sdk` for `.workshop/zwift-snap-sdk/sdk.yaml`), but plug and
slot references elsewhere (e.g. `connections:`) use the bare name (`zwift-snap-sdk`),
because the `project-` prefix only selects the source and is not part of the name.
The workshop desktop and gpu interfaces are NOT auto-connected, the `connections:`
entries are required.

## Build

Use the `build` action to build and the `install` action to install the result.

When running the `build` action (or any long-running action) from an agent,
pipe the output through `tee` instead of swallowing it with `tail`: a
piped `tail` hides the build's live progress (and makes it look hung), while
`tee` shows the output as it streams and keeps it in a file for later reading,
for example by appending `| tee build.log` to the appropriate command. Keep
`pipefail` in mind when chaining commands afterwards.

## Verification

Before declaring a task done run `lint` and `test`. Run `smoke` when the change
affects anything the launcher does at runtime (plugs, command-chain, wine
environment, dialogs).

- **Stale snap data**: reinstalling the snap (`snap install --dangerous --amend`)
  does not remove the old wine prefix in `~/snap/zwift/common`. Before smoke
  testing a fresh build, purge first (`sudo snap remove --purge zwift`), install
  the new snap and reconnect the `gpu-2604` interface — otherwise the test runs
  against the stale installation and the new wine is never exercised. `tests/smoke.sh`
  expects the snap to be installed beforehand and never installs or removes it.

## Visual smoke test (yarf)

`tests/smoke.sh` starts a `mir-test-tools` Mir compositor, runs the real launcher
against it and drives the Robot Framework suite in `tests/smoke/` with
[yarf](https://documentation.ubuntu.com/yarf): test 1 requires the yad progress
dialog (template match on its static title bar), test 2 requires the Zwift window
(`zwift-login.png`). The first run on a fresh wine prefix installs wine prerequisites
and downloads Zwift (tens of minutes), later runs are quick. No login is performed.
Extra arguments to the `smoke` action are passed to yarf (for example,
`--variant light`). The harness installs `yarf` and `mir-test-tools` and connects
`zwift:gpu-2604` to `mesa-2604` when needed.

On failure check `~/zwift-smoke-out/`: `yarf.log`, `log.html` (screenshots yarf
compared), `zwift.out`, the per-mode `mir-<mode>.log`, and the tail of
`~/snap/zwift/common/zwift.log` that the harness prints.

Two logfiles show live progress while the suite runs:

- `~/snap/zwift/common/zwift.log` — the launcher log: winetricks steps
  (`Executing` lines), the Zwift download, the launcher's own state.
- `~/snap/zwift/common/.wine/drive_c/users/<user>/AppData/Local/Zwift/Logs/Log.txt`
  — Zwift's own log, which only exists once the game has started.

Window decorations and themes differ between machines: when the suite reports a
missing template, recapture it with the recipe in `tests/smoke/README.md` and commit
it. `SMOKE_DISPLAY=nested|headless` overrides the auto detection; nested renders on
the host display through the workshop desktop interface.

Yarf gotchas that cost time to discover (all verified live):

- The yarf snap is strict: it sees only `$HOME` — not `/project`, not `/tmp`, and not
  `~/.cache`. The harness stages the suite to `~/zwift-smoke-suite` for that reason.
- yarf exits 120 with EMPTY output when its stdout is a regular file; piped output is
  fine. The harness pipes it through `tee` (with `pipefail`). This is not a yarf quirk:
  `snap run` (and `snap run --shell`) behaves the same — with stdout redirected to a
  plain file it fails silently with rc=1 and no stderr, while a terminal or pipe works.
  `tests/bluetooth.sh` hits the same wall and pipes its snap-run output through `tee`.
- The suite argument must be a directory containing `*.robot` files, not a file.
- `Match`/`Match Text` timeouts are integer seconds, not Robot time strings.
- OCR (`Match Text`) misreads the small grey yad text ("Gettir" for "Getting");
  template matching on static window chrome is authoritative, OCR is informational.
- A single missed frame happens even when the dialog is visible: "gone" checks must
  require several consecutive misses.
- `mir-test-tools.demo-server` names its own wayland socket after `WAYLAND_DISPLAY`
  (export `wayland-99` before starting it) and refuses to start when that socket
  already exists — clean stale sockets first. To nest on the host compositor it
  also needs `--wayland-host` set to the host socket name, captured before that
  export, plus GPU device access from the workshop gpu interface — without either
  it hangs with an empty log and the harness falls back to headless.
- Nested Mir can also abort on startup with `Failed to read Wayland events: Broken pipe`
  when a required host/workshop interface is disconnected. Check the workshop desktop
  and GPU connections before blaming stale processes or sockets; reconnecting the missing
  plug fixed this during a pairing run. The harness falls back to headless even with
  `SMOKE_DISPLAY=nested`, so check its startup output to confirm the requested mode.
  To expose a blank compositor log, reproduce its command with output piped through
  `tee` rather than redirected to a regular file.

## Screenshots and input for the running game

To see what Zwift shows (login form, error messages, pairing screen) while debugging, or to
click/type into it, start a compositor and the game as `tests/smoke.sh` does, then use
`tests/tools/yarf/yarf.sh` (the `yarf` workshop action):

```
tests/tools/yarf/yarf.sh screenshot [-s wayland-99] [OUTPUT.png]   # save one frame, view the PNG
tests/tools/yarf/yarf.sh input [-s wayland-99] click 0.5 0.8 then type foo then key Return
```

The default screenshot output is `~/zwift-yarf/shot-<time>.png`. The `input` commands are
`click X Y` / `move X Y` (X, Y are 0.0-1.0 proportional), `type TEXT`, `key NAME`, `sleep SECONDS`.

- **Do not use `grim`.** It is installed in the workshop but fails against Mir with
  `wl_shm@N.create_pool: invalid arguments` / `failed to screenshoot all outputs`, whatever
  the options (`-s`, `-t`) and timing (reproduced against an idle, freshly started Mir).
  Connecting `mir-test-tools:wayland` does not change that: the `demo-server` app does not
  even declare that plug. Not root-caused; do not spend more time on it.
- The script runs `tests/tools/yarf/screenshot.py` / `input.py` inside the yarf snap, using yarf's own
  `zwlr_screencopy_unstable_v1` and virtual pointer/keyboard clients (`yarf.lib.wayland.*`), the
  same capture/input path as the smoke and pairing tests. Mir must have been started with
  `zwlr_screencopy_manager_v1` in `--add-wayland-extensions` (smoke.sh does).
- yarf's `python3` only exists inside its snap (`/snap/yarf/*/bin/python3` is a dangling
  symlink on the host), so go through `snap run --shell yarf -c ...`.
- Inside the yarf snap `$HOME` is remapped to `~/snap/yarf/<rev>`; the real home is
  `$SNAP_REAL_HOME`. yarf can read and write non-hidden directories under the real home
  (hence `~/zwift-yarf`), but not `/tmp` or `/project`.
- yarf also has its own `XDG_RUNTIME_DIR` (`/run/user/<uid>/snap.yarf`): the compositor
  socket must be symlinked into it, which the script does.
- A black frame means the compositor works but no window is up yet; take it later.

## Driving the game by hand (no launcher script)

`zwift.wine` defaults to `WINEPREFIX=~/snap/zwift/x1/.wine`, not the real prefix, so set
`WINEPREFIX=$HOME/snap/zwift/common/.wine` (inside the snap `$SNAP_USER_COMMON/.wine`).
Starting `ZwiftApp.exe` directly only spawns the .NET launcher; to start the game itself
repeat the launcher dance of `launcher/zwift-run.sh`: `wine start ZwiftLauncher.exe
SilentLaunch`, read its pid, then `wine start /unix $SNAP/bin/runfromprocess-rs.exe <pid>
ZwiftApp.exe <args>`. Run it from a script file in `~/snap/zwift/common` through
`snap run --shell zwift.wine -c 'bash "$HOME/../common/script.sh"'` and source
`$SNAP/bin/zwift-common.sh` for the helpers. Use `ZwiftApp.exe --token=<json>` to skip the
login form; the game log is `AppData/Local/Zwift/Logs/Log.txt` in the prefix user dir.
A hand-made token is accepted by the client, but the game then refreshes it at
`secure.zwift.com`, whose real 400 `invalid_grant` shows as "Oops. Wrong email/password".

## Bluetooth plumbing test (bluezoo)

`tests/bluetooth.sh` checks the Bluetooth plumbing of the snap, not the game: wine's
Bluetooth driver (`winebth`, hosted by winedevice.exe) must reach BlueZ over the snap's
`bluez` plug, see the adapters, register its authentication agent and learn about a
discovered BLE smart trainer. It asserts only on the winebth trace (no GATT traffic).

The workshop has no Bluetooth hardware, and bluetoothd's management socket needs
privileges the container does not have (`bluetoothctl scan` hangs on `mgmt_socket`),
so the test uses [bluezoo](https://github.com/Samsung/BlueZoo), a user-space BlueZ
D-Bus mock, in place of bluetoothd. The `setup-base` SDK hook installs `bluez`
(bluetoothctl, the D-Bus policy), a `/opt/bluezoo` venv (PyPI, not cached by pkgproxy)
and masks `bluetooth.service`; the test starts bluezoo itself, runs `zwift.wine` in a
throwaway prefix (`~/snap/zwift/common/bt-test`) with `WINEDEBUG=+winebth` and asserts
on the trace. Artifacts on failure: `~/zwift-bt-out/`.

Gotchas that cost time to discover (all verified live):

- bluezoo must run as root (only root may own `org.bluez` per the bluez D-Bus policy),
  detached with `setsid` — a cancelled caller takes its process group down otherwise.
  The bluez package's D-Bus activation stub (`Exec=/bin/false`) must be removed, or the
  first call to the unowned `org.bluez` name fails with a confusing systemd error.
- bluezoo has no device emulation of its own: a device is another mock adapter. bluezoo
  runs two adapters, hci0 (the PC, `00:00:00:11:11:11`) and hci1 (the trainer,
  `00:00:00:22:22:22`). `tests/tools/bluetooth/bluetooth-trainer.py` makes hci1 a BLE trainer named
  "KICKR": an LE advertisement (FTMS 0x1826 + Cycling Power 0x1818, discoverable) and a
  GATT application with FTMS, Cycling Power and Device Information; it streams 150 W /
  85 rpm while notifications are on and logs every read/write/notify to `trainer.log`.
  bluezoo links the GATT application to the central's Device1 on Connect.
- bluezoo only scans while the client that started the discovery stays connected (and any
  client's StopDiscovery stops it for all). `bluetoothctl` cannot do that here (its
  management socket hangs), so `tests/tools/bluetooth/bluetooth-scan.py` keeps an LE-only discovery open on
  hci0 over plain D-Bus.
- bluezoo reports RSSI 0 and never changes it, but winebth re-reports an advertisement to
  the BLE watcher only when an LE property changes with an RSSI present — so Zwift saw
  KICKR once and dropped it. `tests/tools/bluetooth/bluezoo-run.py` wraps bluezoo, sets RSSI to -55 and
  jitters it on every scan round (one PropertiesChanged per second).
- winebth would add the trainer's own adapter as a second radio, and wine's watcher uses
  the first radio. `ROUVY_BT_ADAPTER=hci0` (a hci name or address, read in winebth's
  `dbus.c`) makes winebth use only hci0 and log "ignoring Bluetooth adapter hci1"; both
  tests export it.
- The snap app remaps `$HOME` to its private `~/snap/zwift/x1`, so a prefix in
  `~/snap/zwift/common` is `$HOME/../common/...` from inside the snap.
- Everything under `/tmp` inside the snap is private to each command and discarded —
  a log file written there is gone before it can be read.

The bluezoo setup (starting the mock as root, the trainer, the scan client, the prerequisites) lives in
`tests/tools/bluetooth/bluezoo.sh`, shared by `tests/bluetooth.sh` and the pairing test below.

## Pairing-screen test (fake login servers)

`tests/pairing.sh` (the `pairing` workshop action) reaches Zwift's device pairing screen
**without a Zwift account**, with the bluezoo mock running. It is a test aid, never shipped.
Full detail is in `tests/pairing/README.md`; the essentials:

- `tests/zwift-stub.py` fakes the two servers login needs (`secure.zwift.com`,
  `us-or-rly101.zwift.com`). It is an HTTPS proxy: the game is pointed at it with `https_proxy`
  (which `snap run` passes through), and it terminates TLS for those two hosts with a throwaway
  CA. Every other host is tunnelled through unchanged, sentry is refused.
- With only `--token` the game still refreshes at `secure.zwift.com` and the real 400
  `invalid_grant` shows as "Oops. Wrong email/password" (see "Driving the game by hand"), so the
  token endpoint must be faked. The stub answers it with an unsigned token and the handful of
  game API calls the game blocks on before pairing (`/api/auth`, `/api/users/login`,
  `/api/profiles/me`, the world list, some empty 200s). The protobuf response shapes come from
  [zwift-offline](https://github.com/fatsbrown/zwift-offline).
- The stub CA is appended to the prefix's `data/cacert.pem` for the test (backed up, restored
  after). The game is driven directly (the launcher dance of `zwift-run.sh`, with fake
  `Game_Launcher` OAuth JSON passed via `--token`) rather than through the real launcher, so the Zwift patcher never runs and
  cannot re-download and overwrite the CA.
- `tests/pairing/pairing.robot` drives the UI with yarf and clicks buttons by their OCR text
  (resolution independent). The stub's `--token-file` writes a temporary fake token file under
  `~/snap/zwift/common`, removed on exit; token refresh goes through the stub. No login-form
  clicks or credentials are needed. The stub profile pre-completes EULA / sport / reason:
  `saved_game` contains the accepted-license record and its mandatory end chunk, while private
  attributes select CYCLIST and RACING. The suite does not click or wait for those screens.
- The pairing screen is detected by its POWER SOURCE tile, not the PAIRED DEVICES title: Zwift
  can open a "No ANT+ dongle connected?" tip over the title. The suite then pairs the mock
  trainer: POWER SOURCE → PAIR DEVICE dialog → click KICKR → CLOSE. Clicking a device in the
  dialog toggles it, and clicking a *connected* tile unpairs it, so a returning profile is first
  given time to reconnect by itself.
- yarf cannot read the snap's data dir, so `tests/pairing.sh` asserts in bash on the game
  `Log.txt`: `ZSF_PAIRING_SCREEN`, `Device: "KICKR" has new connection status: connected`, the
  `Power: KICKR` role in the last `Device List:`, and the indoor-bike-data / cycling-power
  notify subscriptions in `trainer.log`. A fresh pairing assigns only the Power role; Cadence
  and Controllable Trainer show up on some layouts or after re-selecting, so they are not
  required. Artifacts: `~/zwift-pairing-out/`.
- On the pairing screen Zwift starts a native BLE search (BleWin10Lib); wine's advertisement
  watcher turns it into a BlueZ `SetDiscoveryFilter(Transport="le")` + `StartDiscovery` on the
  default radio. Classic BR/EDR devices are therefore never reported to Zwift.
  `tests/pairing.sh --explore` keeps the game on the pairing screen for an hour to watch and
  click live (game log, `bluezoo.log`, `trainer.log`, `tests/tools/yarf/yarf.sh`); run it detached
  so an interrupted caller does not close the game. Set `WINEDEBUG` (e.g.
  `+bluetooth,+winebth`) to trace wine's side.
- A killed run skips its EXIT trap and leaves the stub CA in `cacert.pem`; the next run strips it
  before taking its backup.
- yarf's OCR (RapidOCR on onnxruntime) uses every core by default: one pass over a 1280×1024
  frame cost ~32 s of CPU. `tests/tools/yarf/yarf_ocr.py`, imported as a Robot `Library` by both
  suites (and staged next to them), caps it at 4 intra-op threads (~6 s CPU, faster wall time).
  yarf's own match loops already sleep at least 0.1 s per iteration.
- `tests/tools/yarf/yarf.sh` (the `yarf` action) sends pointer/keyboard input and takes screenshots
  of the running compositor (yarf's own wayland clients inside the yarf snap), for exploring the
  screens by hand when updating the suite for a new Zwift version.

## Snap specifics that are easy to break

- **Pruning**: the wine part aggressively prunes (man pages, locales, wine's build
  toolchain, ~776 static import libraries, plus everything unreachable via the
  `snap/prune-closure.sh` closure walk). Guards in the part's `override-prime` fail
  the build on over-pruning (runtime file missing) or a stale prime directory (needs
  the `clean` action). Bump the `v<N>` comment there when editing
  `prune-closure.sh`, otherwise craft-parts will not re-run the prime step.
- **Do not prune** `usr/lib/*/samba` (ntlm_auth needs it) and the nss libraries
  listed in the `lint.ignore` block of `snap/snapcraft.yaml` (dlopened by name).
- **Wine source**: the `wine` part builds [ndyer/wine](https://github.com/ndyer/wine)
  (fork of wine-mirror/wine) at the `source-branch` in `snap/snapcraft.yaml`
  (`zwift-radios-rebased`, not pinned to a `source-commit`). craft-parts does not
  re-pull a git source for new commits on an unchanged branch, so run the `clean`
  action (or pin/bump `source-commit`) to pick up new commits on the branch.
- **Wine runtime**: new-style WoW64 has no `wine64` binary or i386 Unix packages;
  32-bit Windows programs use `lib/wine/i386-windows`. Check the winetricks steps
  (`dotnet48`, `d3dcompiler_47`, `webview2`) on a desktop. The runtime is not a
  separate content snap: existing core24 Wine runtime libraries do not match this
  build, and splitting it adds a snap without reducing the total download.
- **Bluetooth**: the `zwift-radios-rebased` wine branch implements the BLE
  advertisement watcher (`adv_watcher_Start` in
  `dlls/windows.devices.bluetooth/advertisement.c`, LE-only discovery) and GATT on top
  of winebth. Upstream wine 11.21 stubs the watcher with `E_NOTIMPL`, so check this
  when moving the part to another branch or tag.
- **pkgproxy** caches apt and snap downloads in the workshop; the base snap,
  https sources and the wine git clone are not cached. The `stats` action shows
  the savings.
- **No `home` plug**: wine user folders live inside the wine prefix
  (`~/snap/zwift/common/.wine/drive_c/users/<user>/`); the launcher replaces links
  to the real home with private directories at every start.
- **No SVG pixbuf loader / no gecko/mono**: dialogs use a png icon, mshtml/mscoree
  are disabled at wineboot (`WINEDLLOVERRIDES`).
- **glycin layouts**: `layout:` in snapcraft.yaml bind-mounts the glycin loader
  directories because yad loads images through absolute paths from its config.
- **gtk data at runtime**: `wine-env.sh` generates the gdk-pixbuf loaders cache and
  the mime cache per snap revision, and compiles glib schemas at build time
  (`glib-compile-schemas` in override-prime). Without them yad aborts.

## Conventions

- Launcher scripts: bash, `set -uo pipefail`, shellcheck-clean (see the `lint`
  action for the exact command and scope). GUI is optional: every dialog function
  degrades to terminal output when there is no display or `ZWIFT_NO_GUI=1`.
- User-visible launcher messages go through `msgbox`/`gui_progress` and are written
  to `~/snap/zwift/common/zwift.log`; keep that logging intact when editing — the
  smoke test and users debug from it.
- YAML files are linted with yamllint relaxed, line length 140.
- Comments explain *why*; several constraints above are encoded as comments next to
  the code they protect — keep them in sync when you change behavior.

## Release Procedures

- Tags in the main repository `/project` follow semantic versioning: `v0.10`, `v0.11`, `v0.12`
- Wine source tags in `/project/wine-src` use the pattern: `zwift-snap-v0.12`
- When releasing a new version:
  1. Ensure both repositories are clean, don't proceed otherwise.
  2. Figure out the next version in sequence.
  3. Tag wine-src.
  4. Tag main repository
  5. Rebuild snap with `sudo snapcraft pack --destructive-mode`
  6. Move/re-tag main repository's v0.12 to point to the final commit
  7. Run all validation and tests
  8. Upload with snapcraft upload (will need credentials so this may be done by user)
