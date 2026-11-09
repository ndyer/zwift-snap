# AGENTS.md

Guidance for AI coding agents working in this repository.

## What this project is

An unofficial snap launcher for [Zwift](https://zwift.com) for Linux. Zwift itself is
proprietary and NOT part of the snap — the Rust launcher downloads and updates it
into a wine prefix at runtime. Wine is built from source by the snap's `wine` part:
[ndyer/wine](https://github.com/ndyer/wine), a fork of the official wine-mirror/wine
GitHub mirror, at the `source-branch` in `snap/snapcraft.yaml` (currently
`zwift-radios-rebased`: wine-11.21 plus BLE support — advertisement watcher, GATT,
Windows.Devices.Radios; new-style WoW64, no i386 host libs). Strict confinement, amd64 only, base `core26`.

## Layout

```
launcher-gui/      Rust/gtk-rs launcher, lifecycle, logging and Rust unit/integration tests
launcher/          shared wine-env.sh bootstrap, pulse-client.conf and Windows window helper
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
  tools/dl-cache/    the smoke test's Zwift download cache helper dl-cache.sh, with the
                   tests/dl-cache.sh entrypoint for it one level up
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

### Recovering the pkgproxy snap cache

The `project-pkgproxy` SDK seeds its snap cache from `.workshop/pkgproxy/cache/`
before the project is mounted. Each cached snap is a file named by its 96-character
SHA3-384 digest, paired with `<digest>.meta.json`. The live cache is under
`/var/snap/pkgproxy/common/cache/snap/objects/sha3-384` inside the workshop.

To recover snap objects after an interrupted or failed launch:

1. Check `workshop list`, `workshop changes` and `workshop tasks <change-id>` first.
   A failed launch normally rolls back and deletes its container; downloads after
   the last SDK snapshot are then lost. If the change is still running, do not
   interfere with it. For future troubleshooting, `workshop launch --wait-on-error`
   preserves the failed instance for investigation until continued or aborted.
2. Locate the surviving instance or SDK snapshots with `lxc list --all-projects`.
   The workshop is normally in `workshop.$USER`; SDK snapshots are stopped
   containers in `workshop-snapshots.$USER`. Check creation times and task order:
   a `pkgproxy-*` snapshot taken before the next SDK's setup cannot contain its
   later downloads. Older `zwift-snap-sdk-*` or `rust-*` snapshots may contain
   useful objects, but are not evidence of downloads from the latest launch.
3. List the cache directory without starting the instance. Substitute the actual
   project and instance names (the project can use a numeric UID instead of the
   username):

   ```sh
   lxc query '/1.0/instances/<instance>/files?project=<project>&path=/var/snap/pkgproxy/common/cache/snap/objects/sha3-384'
   ```

   Compare the names with `.workshop/pkgproxy/cache/`; a missing directory means
   that snapshot has no cache at that path. Do not copy incomplete objects or
   overwrite existing local files.
4. For each missing pair, pull both files to a temporary directory, including
   from stopped snapshot containers:

   ```sh
   lxc file pull --project <project> <instance>/var/snap/pkgproxy/common/cache/snap/objects/sha3-384/<digest> <temporary-directory>/
   lxc file pull --project <project> <instance>/var/snap/pkgproxy/common/cache/snap/objects/sha3-384/<digest>.meta.json <temporary-directory>/
   ```

   Verify the blob's SHA3-384 against its filename, and its size and content hash
   against the JSON sidecar before copying the pair into `.workshop/pkgproxy/cache/`.
   Sidecars may encode `ContentHash` as `sha3-384:<digest>` or
   `sha3-384/<digest>`. Do not replace existing entries. `unsquashfs -cat <blob>
   meta/snap.yaml` identifies a snap if needed. Remove only the temporary files
   created for recovery.

## Build

Use the `build` action to build and the `install` action to install the result.

When running the `build` action (or any long-running action) from an agent,
pipe the output through `tee` instead of swallowing it with `tail`: a
piped `tail` hides the build's live progress (and makes it look hung), while
`tee` shows the output as it streams and keeps it in a file for later reading,
for example by appending `| tee build.log` to the appropriate command. Keep
`pipefail` in mind when chaining commands afterwards.

### Fast iteration with snap try

For launcher changes, avoid a full `build` on each edit. `snap try` mounts an
unpacked snap directory in place of the installed revision, so the launcher can
run from the prime tree:

1. Refresh the prime tree (rebuilds only changed parts, e.g. the launcher):
   `sudo snapcraft prime --destructive-mode`
2. Mount it: `sudo snap try /project/prime`. `snap list zwift` then shows the
   revision marked `try` (mounted from `/project/prime`).
3. Run it: `snap run zwift`. Check `~/snap/zwift/common/zwift.log` for the launcher's log.

Notes:
- Run `snap try` only after a successful `snapcraft prime`. An empty or missing
  prime tree fails with a missing `snap.yaml`.
- Running the dev build directly (`cargo run`) outside the snap fails with
  `No such file or directory (os error 2)` at `wine start`, because the wine
  binaries exist only inside the snap. Use `snap try` to test runtime behaviour.
- Dialogs and the game need a display: start the launcher from a session that
  has one. The game process keeps running after the launcher is closed.
- To undo `snap try`, install a `.snap` file over it with
  `sudo snap install --dangerous <file>.snap` (or `--amend`). This has not been
  verified against a `try` mount here. Avoid `sudo snap remove zwift` between
  iterations: it is slow, because the snap's multi-GB data is snapshotted. Do not
  use `--purge`, because it deletes the wine prefix and login state in
  `~/snap/zwift/common`.
- Do not `snap try` a directory that is not a prime tree (for example, the project root).

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
[yarf](https://documentation.ubuntu.com/yarf): test 1 looks for the GTK log button,
test 2 requires the Zwift SIGN UP screen through OCR. The first run on a fresh wine prefix installs wine prerequisites
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
- OCR (`Match Text`) can misread small text. A separate screenshot/OCR pass may
  succeed while the running yarf client's Wayland capture has failed; inspect both.
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

## Zwift download cache (smoke test)

The smoke test's slow parts are downloads, and they are unchanged between runs, so
`tests/smoke.sh` keeps them in `~/zwift-cache` (test tooling only — the launcher never
reads or writes it, and nothing of it ships):

- `game/` — the whole `drive_c/Program Files (x86)/Zwift` directory of the wine
  prefix: what `ZwiftSetup.exe` installs and what the patcher then downloads into.
  Restoring it satisfies the launcher's `installed()` check, so a staged prefix
  launches without installing anything.
- `winetricks/` — winetricks' own download cache, its `W_CACHE` directory.
- `meta.json` — the Zwift `sversion` and capture time, so staleness is visible.

`smoke.sh` restores the cache into the prefix before starting and captures it again
after a passing run; `--fresh` or `SMOKE_CACHE=0` (defined in `smoke.sh`, never
forwarded to yarf) skips both and wipes what was staged. The `dl-cache` action
(`tests/dl-cache.sh`) exposes `status`, `capture [--force]`, `stage [--force]` and
`clear` for inspecting the cache or restoring a prefix by hand after
`sudo snap remove --purge zwift`. `tests/tools/dl-cache/dl-cache.sh` holds the
implementation and `tests/unit/dl-cache.bats` its tests.

Two details it depends on, both verified live:

- The cache lives in `$HOME`, not under `~/snap/zwift`, because
  `snap remove --purge` deletes the latter — the case the cache exists for.
- `smoke.sh` exports `W_CACHE` (`~/snap/zwift/common/winetricks-cache`) before `snap
  run` (the snap sees that absolute path unchanged). Without it the launcher's
  cleanup deletes the default `$XDG_CACHE_HOME/winetricks` every run, so winetricks'
  downloads can never be captured. `cp -a` everywhere, because the Zwift patcher
  verifies local files by size, checksum *and modification time*.

Only the game directory is cached, not the whole prefix: winetricks prerequisites
live elsewhere in `drive_c` and are detected separately (`installed()` checks their
proof paths), so a restored game tree in a prefix without them still reinstalls
them. The cache is valid for the Zwift version in `meta.json`; a version change is
left to the patcher, which downloads the delta.

Because `installed()` also requires the winetricks prerequisite proofs, a restored
game tree alone does not satisfy it: the run takes the `install()` path and
reinstalls the prerequisites. That path re-checks the version before downloading,
so a staged tree already at the latest version is recognised and the game download
is skipped. `--fresh` (or a prefix purged without the cache) is what still covers
the full download and installation.

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
repeat the launcher sequence in `launcher-gui/src/launch.rs`: `wine start ZwiftLauncher.exe
SilentLaunch`, read its pid, then `wine start /unix $SNAP/bin/runfromprocess-rs.exe <pid>
ZwiftApp.exe <args>`. Run it from a script file in `~/snap/zwift/common` through
`snap run --shell zwift.wine -c 'bash "$HOME/../common/script.sh"'` and adapt
the test-local task helpers in `tests/pairing.sh`. Use `ZwiftApp.exe --token=<json>` to skip the
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
  after). The game is driven directly (the launcher's Wine parent-process sequence, with fake
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

## Login and "Remember me" detection

The launcher logs in through a WebKitGTK WebView in the bottom panel
(`launcher-gui/src/gui/login_panel.rs`, shown at all times; it loads Zwift's launcher
embed page until the login gate opens). It does OAuth PKCE against Keycloak
(`launcher-gui/src/oauth.rs`), watches navigation for the `http://zwift/?code=...`
redirect, checks `state`, exchanges the code and saves `pairing-token.json`, which
`launch.rs` passes to the game as `--token`.

- The launcher cannot read the "Remember me" checkbox. The token has no remember-me claim: the
  refresh-token lifetime was 691200 s whether the box was ticked or not.
- The setting is visible as a cookie in the WebView's network session. The cookies are scoped to
  `https://secure.zwift.com/auth/realms/zwift/`. Querying `https://secure.zwift.com/` alone returns
  only `_dd_s_v2` and `sticky`, so use the realm path.
- Ticked: `KEYCLOAK_REMEMBER_ME` is present (31 bytes), and `KEYCLOAK_IDENTITY` and
  `KEYCLOAK_SESSION` expire in about a year. Unticked: `KEYCLOAK_REMEMBER_ME` is absent, and
  `KEYCLOAK_IDENTITY` is a session cookie with no expiry.
- The login WebView keeps its cookies in `~/snap/zwift/common/webkit/cookies.sqlite`
  (`persistent_webview` in `gui.rs`), so a remember-me cookie survives a launcher restart. The
  directory is 0700 and the database 0600. Those cookies are live session secrets: never copy, log
  or commit them.
- webkit6 0.4 `CookieManager::all_cookies` needs the `v2_42` feature, which the crate does not enable.
  Use `CookieManager::cookies(uri, cancellable, callback)` instead.
- When probing cookies, log names, flags, expiry and length, or a hash prefix. Never log values:
  they are session secrets and would end up in `zwift.log`.

### zoffline reference (fatsbrown/zwift-offline)

A clone of [fatsbrown/zwift-offline](https://github.com/fatsbrown/zwift-offline), the maintained fork of
the deprecated zoffline, lives at `~/zwift-offline` (outside the repo). It is not part of the snap. It is a
local stand-in for Zwift's servers, so it shows the shape of the real login flow but not the real
account chooser. Findings:

- Its launcher login form, `cdn/static/web/launcher/login_form.html`, has the same "Remember me"
  checkbox (`name="remember"`). It is a single username and password form. There is no account list.
- Remember-me is server-side. `login()` in `zwift_offline.py` (~line 651) stores `user.remember` and
  sets Flask-Login's `remember_token` cookie. This is an analogy for `KEYCLOAK_REMEMBER_ME`, not
  the same cookie.
- The launcher handoff is `zwift_offline.py` (~line 4930): the `.../openid-connect/auth` route
  redirects to `http://zwift/?code=zwift_refresh_token<JWT>`. The JWT carries the session cookie.
  `oauth.rs` intercepts the same redirect for the real login.
- The token endpoint (`zwift_offline.py` ~line 4968) returns a fake access and refresh token pair.
  It does no real identity check.
- The real token endpoints used by the fork's scripts are `secure.zwift.com/auth/realms/zwift/protocol/openid-connect/token`
  and `.../tokens/access/codes` (`scripts/get_profile.py` ~line 57).
- No account-list endpoint was found, either in zoffline or in the launcher's strings. The
  `ZwiftLauncher.exe` account list is still unexplained.

## Graphics profile

The launcher can override Zwift's GPU table, which rates many cards below what they can
render. `ZWIFT_GRAPHICS_PROFILE` (parsed in `application.rs`, applied in
`graphics.rs`) selects `auto` (default), `basic`, `medium`, `high`, `ultra` or `default`:

- `auto` measures the GPU at launch (`gpu_test.rs`, a port of the benchmark from
  [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap), see the module doc for
  attribution) and maps the frame time to a profile with `MAX_FRAME_MS_FOR_60_FPS`.
- A fixed profile skips the measurement.
- `default` leaves Zwift's files untouched, so its own GPU table decides again.

The profiles are the game's `data/configs/*.txt`. The launcher writes the chosen profile into
*all four* files (so it does not matter which the game reads) and keeps the originals in
`~/snap/zwift/common/graphics/` plus an `applied` marker recording what it wrote. On the next
run a file whose content differs from the marker is treated as patcher-updated and its saved
original is refreshed before the profile is applied again. `graphics.rs` has the unit tests;
`tests/entrypoint.rs` covers the end-to-end write/restore/validation paths.

`gpu_test.rs` uses bpauli's `glow` + `glutin_egl_sys` + `libloading` stack (not the
`khronos-egl` + `glow` combination that was first considered) so the published frame-time
thresholds stay calibrated against the same measurement. The GPU test needs EGL; without it
the launcher logs a warning and leaves the files to the game.

### Not adopted from bpauli/zwift-bootstrap

- ANT+ support: strict confinement has no `CAP_NET_ADMIN`, so the snap cannot open the raw
  sockets an ANT+ USB stick needs. Not implementable here.
- The 30–50 ms BLE connection interval: same `CAP_NET_ADMIN` constraint.
- The direct-CDN installer: the snap keeps downloading Zwift through the official launcher
  flow (`install.rs`), which the patcher understands; a hand-rolled CDN download was deferred.

## Bluetooth discovery vs. connect (wine fork)

BlueZ drops LE connections while a discovery is running, so `winebth.c` in the
`zwift-radios-rebased` wine branch stops LE discovery before it connects a device and starts
it again once the connection is up. The state is per radio (`le_discovery_refs`,
`discovery_paused`, guarded by `discovery_cs`) and driven by the `connecting` flags of the
radio's devices, so a device destroyed mid-connect cannot leak a pause. Discovery is paused
*before* `props_cs` is taken, because stopping it blocks on the event loop thread, which needs
`props_cs`. When the driver is removed (`IRP_MN_REMOVE_DEVICE`) it disconnects any device
that is still connected, so a killed prefix does not leave the peer advertising it as
connected. `tests/pairing.sh` asserts the pause/resume lines and the shutdown disconnect from
the `+winebth` trace.

## Snap specifics that are easy to break

- **Pruning**: the wine part aggressively prunes (man pages, locales, wine's build
  toolchain, ~776 static import libraries, plus everything unreachable via the
  `snap/prune-closure.sh` closure walk). Guards in `runtime-prune`'s `override-prime` fail
  the build on over-pruning (runtime file missing) or a stale prime directory (needs
  the `clean` action). Bump the `v<N>` comment there when editing
  `prune-closure.sh`, otherwise craft-parts will not re-run the prime step.
- **Build ordering**: `runtime-prune` depends on Wine, the Rust launcher and the
  runtime helpers; GPU cleanup follows it. Never make Wine depend on the launcher
  just to order pruning: this invalidates Wine's build on every launcher change.
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
- **winetricks invocation**: the launcher installs all four prerequisites with one
  `winetricks -q dotnet48 d3dcompiler_47 webview2 renderer=vulkan` call, not one
  call per verb: winetricks runs every command-line verb in a single process, in
  order, and exits non-zero on the first failure, so this is faster and still
  stops the install before Zwift is downloaded. The per-verb dialog titles are
  read from the `Executing w_do_call <verb>` lines winetricks prints to stderr
  (`WINETRICKS_VERB_MARKER` in `launcher-gui/src/install.rs`), so the pinned
  winetricks and the launcher must change together if that output changes.
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
  directories because GTK image loading uses absolute paths from its config.
- **gtk data at runtime**: `wine-env.sh` generates the gdk-pixbuf loaders cache and
  the mime cache per snap revision, and compiles glib schemas at build time
  (`glib-compile-schemas` in override-prime). Keep these runtime data available for GTK.

## Conventions

- Launcher: Rust with gtk-rs GTK4; `cargo fmt`, `cargo clippy` and `cargo test`
  are included in the lint/test actions. GUI is optional: no display or
  `ZWIFT_NO_GUI=1` selects terminal operation. The shared `wine-env.sh` bootstrap
  remains bash because library paths must be set before loading the GUI binary.
- User-visible launcher messages go through the Rust logger and are written
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
