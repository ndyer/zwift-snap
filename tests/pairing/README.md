# Pairing-screen test

`tests/pairing.sh` reaches Zwift's device pairing screen without a Zwift account, with the
bluezoo Bluetooth mock running. It is a test aid; nothing here ships in the snap.

It is run with the `pairing` workshop action:

```shell
workshop run zwift-snap pairing            # SMOKE_DISPLAY=auto|nested|headless, extra args go to yarf
```

The zwift snap must be installed and Zwift already downloaded into the prefix, so run the
`smoke` test once first. The script never installs, updates or removes anything.

## How it works

- [`tests/zwift-stub.py`](../zwift-stub.py) stands in for the two Zwift servers the game needs
  to log in: the login server `secure.zwift.com` and the game server `us-or-rly101.zwift.com`.
  It is an HTTPS proxy: the game is pointed at it with `https_proxy`, and it terminates TLS for
  those two hosts with a throwaway CA. Every other host (the cdn, sentry, winetricks) is
  tunnelled straight through, so only login and the game API are faked.
- The game trusts the stub because its CA is appended to the prefix's `data/cacert.pem` for the
  duration of the test (backed up and restored afterwards). The game is driven directly (the
  parent-process sequence of `zwsnap-gui/src/launch.rs`, with a fake login token) rather than through the real
  launcher, so the Zwift patcher never runs and cannot overwrite the CA. That sequence starts
  the snap's own `bin/ZwiftLauncher.exe` (copied into the game directory as the real launcher
  does), which is the parent process name `ZwiftApp.exe` demands and forwards the command line
  it is given.
- The stub answers the OAuth token endpoint with a fake (unsigned) token and the handful of game
  API calls the game blocks on before the pairing screen: `/api/auth`, `/api/users/login` (a
  protobuf `LoginResponse`), `/api/profiles/me` (a pre-onboarded protobuf profile), the world list and
  a few empty 200s. The protobuf shapes come from
  [zwift-offline](https://github.com/fatsbrown/zwift-offline).
- The stub writes fake `Game_Launcher` OAuth token JSON with `--token-file`; the harness passes
  it to `ZwiftApp.exe --token` to bypass the login form. The client refreshes it through the
  stub's OAuth endpoint, not the real service. JSON must be compact (no spaces), because
  the token travels on a Windows command line. The temporary token file is removed on exit.
- [`tests/pairing/pairing.robot`](pairing.robot) drives the UI with yarf: it waits directly
  for the pairing screen, then pairs the mock
  trainer as Power Source (see Bluetooth below). tests/pairing.sh then
  confirms in the game log that it reached `ZSF_PAIRING_SCREEN` (yarf cannot read the snap's data
  directory) and saves a screenshot of the pairing screen.

The suite finds every button by its on-screen text (OCR), which is resolution independent.
Token login bypasses both SIGN UP and LOG IN. The stub profile skips the onboarding screens
(EULA, sport, reason): its `saved_game`
contains the license-acceptance integer record (key `0xE412BB7B`, value `1`) and an end
chunk, and its private attributes select `CYCLIST` and the `RACING` goal. These values were
captured from the fake profile updates made by the client when completing onboarding.
They belong to `PlayerProfile`, not `LoginResponse`; the attribute IDs and protobuf
fields are documented in zwift-offline's `protobuf/profile.proto`.

Clicks pause briefly after moving the pointer and hold the button for 100 ms so
Zwift can observe the press across game frames instead of missing an immediate press/release.

## Bluetooth

bluezoo (the same BlueZ D-Bus mock as `tests/bluetooth.sh`, via
`tests/tools/bluetooth/bluezoo.sh`) runs two adapters: hci0 is the PC, hci1 is a BLE smart
trainer named KICKR
([`tests/tools/bluetooth/bluetooth-trainer.py`](../tools/bluetooth/bluetooth-trainer.py): FTMS,
Cycling Power, Device Information, supporting standard services and the Zwift custom service
over GATT, streaming 150 W / 85 rpm).
`ROUVY_BT_ADAPTER=hci0` keeps wine on the PC adapter, and
[`tests/tools/bluetooth/bluezoo-run.py`](../tools/bluetooth/bluezoo-run.py) gives the trainer a
changing RSSI so wine keeps reporting its advertisements to Zwift.

After reaching the pairing screen (detected by its POWER SOURCE tile, since a "No ANT+ dongle"
tip can cover the PAIRED DEVICES title), the suite pairs the trainer: POWER SOURCE → PAIR DEVICE
→ KICKR → CLOSE. Clicking a connected tile would unpair the device, so a returning profile is
first given time to reconnect by itself. tests/pairing.sh then checks in the game log that KICKR
connected and holds the Power role, and in `trainer.log` that
Zwift subscribed to the power, indoor bike data, FTMS control point and Zwift custom channels.
Cadence and Controllable Trainer roles are not required. Explicit Resistance selection is
unnecessary for reproducing the crash.

The Zwift custom service (`00000001-19ca-4651-86e5-fa29dcdd09d1`) exposes async notifications
(`00000002-…`), sync-rx writes (`00000003-…`) and sync-tx indications (`00000004-…`).
It is discovered over GATT, without changing the eight advertised service UUIDs. The real-trainer
trace selects sync-rx immediately before the `DataWriter` activation and fatal Wine
`vccorlib140.dll.?GetIBoxArrayVtable@Details@Platform@@YAPEAXPEAX@Z` call. Power-only pairing
and FTMS subscriptions do not prove this write path was exercised.
This service is a crash-reproduction surface, not a Zwift protocol emulator: incoming writes
are logged and rejected as unsupported, rather than fabricating a handshake response.
The existing FTMS control point still responds to requests with success indications.
A Wine abort is a test failure, not a passing reproduction; the harness prints the abort
from `game.log` alongside the normal failure artifacts.
With the custom service present, the same `GetIBoxArrayVtable` abort is reproducible during
initial Power pairing, before any write reaches the mock. Without that service, both Power
and Resistance pairing succeed.

To reproduce with Bluetooth tracing on the nested desktop, run inside the workshop:

```shell
SMOKE_DISPLAY=nested WINEDEBUG=+bluetooth,+winebth tests/pairing.sh
```

Use `tests/pairing.sh --explore` (best run detached) to keep the game on the pairing screen for
an hour and drive it with `tests/tools/yarf/yarf.sh`; set `WINEDEBUG=+bluetooth,+winebth` to trace
wine.

## Artifacts

On success and failure, artifacts are in `~/zwift-pairing-out/`: `pairing-screen.png`, the yarf
`log.html` (screenshots yarf compared), `yarf.log`, `stub/requests.log` (the requests the game
made, with request bodies redacted), `bluezoo.log`, `trainer.log`, `scan.log`, `game.log` and
the per-mode `mir-*.log`.

## Driving the game by hand

[`tests/tools/yarf/yarf.sh`](../tools/yarf/yarf.sh) (the `yarf` workshop action) takes screenshots of and
sends pointer/keyboard input to the running compositor, for exploring the screens by hand
(for example while updating this suite for a new Zwift version). It runs yarf's own wayland
clients inside the yarf snap. See the comments in that script.

## Trying real authentication

To request real `Game_Launcher` tokens independently of Wine:

```shell
python3 tests/tools/zwift-token.py ~/zwift-token.json
```

Run this in a terminal. Open the printed authorization URL in a browser (on the
host if running inside the workshop) and enter your credentials on Zwift's site.
After login, the browser redirects to `http://zwift/?...`, which may fail to load.
Copy that complete URL from the address bar and paste it at the script's hidden
prompt. Do not share the redirect URL: it contains an authorization code.

The script uses the `Game_Launcher` authorization-code flow with S256 PKCE and a
random state checked on return. The token exchange contacts `secure.zwift.com`
directly over verified HTTPS, ignoring proxy environment variables, and refuses
redirects. Recognized OAuth error codes are reported without printing raw server
responses. `Game_Launcher` rejects password grants with `unauthorized_client`;
mobile-client tokens cannot be refreshed by the game because their client differs.

The full response is saved to a new owner-only file, never printed or overwritten.
Treat this file like a password, keep it outside the repository, do not share it,
and delete it when finished. This helper does not launch the game or configure
persistent login.
