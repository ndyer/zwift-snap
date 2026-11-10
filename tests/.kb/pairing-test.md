# Preface

The pairing-screen test: reaching Zwift's device pairing screen with no Zwift account,
by faking the login servers and running the bluezoo Bluetooth mock. Read this before
changing `tests/pairing.sh`, `tests/zwift-stub.py` or `tests/pairing/pairing.robot` —
several of its behaviours are deliberate workarounds for game and OCR quirks.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What the test is

`tests/pairing.sh` (the `pairing` workshop action) reaches Zwift's device pairing screen
**without a Zwift account**, with the bluezoo mock running. It is a test aid, never shipped.

It is run with the `pairing` workshop action:

```shell
workshop run zwift-snap pairing            # SMOKE_DISPLAY=auto|nested|headless, extra args go to yarf
```

The zwift snap must be installed and Zwift already downloaded into the prefix, so run the
`smoke` test once first. The script never installs, updates or removes anything.

## The fake login servers

- `tests/zwift-stub.py` fakes the two servers login needs (`secure.zwift.com`,
  `us-or-rly101.zwift.com`). It is an HTTPS proxy: the game is pointed at it with `https_proxy`
  (which `snap run` passes through), and it terminates TLS for those two hosts with a throwaway
  CA. Every other host is tunnelled through unchanged, sentry is refused.
- With only `--token` the game still refreshes at `secure.zwift.com` and the real 400
  `invalid_grant` shows as "Oops. Wrong email/password" (see `.kb/driving-the-game-by-hand.md`), so the
  token endpoint must be faked. The stub answers it with an unsigned token and the handful of
  game API calls the game blocks on before pairing (`/api/auth`, `/api/users/login`,
  `/api/profiles/me`, the world list, some empty 200s). The protobuf response shapes come from
  [zwift-offline](https://github.com/fatsbrown/zwift-offline). Routes it does not know fall back
  to an empty 200 whose content type follows the request's `Accept` header
  (`application/x-protobuf-lite` for protobuf clients, `application/json` otherwise): answering a
  protobuf endpoint with JSON leaves the game retrying in a tight loop and it never reaches the
  pairing screen.
- The stub CA is appended to the prefix's `data/cacert.pem` for the test (backed up, restored
  after). The game is driven directly (the launcher's Wine parent-process sequence, with fake
  `Game_Launcher` OAuth JSON passed via `--token`) rather than through the real launcher, so the
  launcher's sync never re-downloads the game files and cannot overwrite the CA.
- The stub writes fake `Game_Launcher` OAuth token JSON with `--token-file`; the harness passes
  it to `ZwiftApp.exe --token` to bypass the login form. JSON must be compact (no spaces),
  because the token travels on a Windows command line. The temporary token file under
  `~/snap/zwift/common` is removed on exit; token refresh goes through the stub.
- The stub profile pre-completes EULA / sport / reason: `saved_game` contains the
  license-acceptance integer record (key `0xE412BB7B`, value `1`) and an end chunk, and its
  private attributes select `CYCLIST` and the `RACING` goal. These values were captured from the
  fake profile updates the client made when completing onboarding. They belong to
  `PlayerProfile`, not `LoginResponse`; the attribute IDs and protobuf fields are documented in
  zwift-offline's `protobuf/profile.proto`. The suite does not click or wait for those screens.

## Driving the UI

`tests/pairing/pairing.robot` drives the UI with yarf and clicks buttons by their OCR text
(resolution independent). Clicks pause briefly after moving the pointer and hold the button for
100 ms so Zwift can observe the press across game frames instead of missing an immediate
press/release.

- The pairing screen is detected by its POWER SOURCE tile, not the PAIRED DEVICES title: Zwift
  can open a "No ANT+ dongle connected?" tip over the title. The suite then pairs the mock
  trainer: POWER SOURCE → PAIR DEVICE dialog → click KICKR → CLOSE. Clicking a device in the
  dialog toggles it, and clicking a *connected* tile unpairs it, so a returning profile is first
  given time to reconnect by itself. The dialog's own title is not a usable anchor (OCR reads
  it as PAIRED DEVICES, the screen behind it), so the suite waits for the trainer name instead:
  the reconnect check just before it proves KICKR is not on the pairing screen, so the name can
  only appear once the dialog is open.

## Bash-side assertions

yarf cannot read the snap's data dir, so `tests/pairing.sh` asserts in bash on the game
`Log.txt`: `ZSF_PAIRING_SCREEN`, `Device: "KICKR" has new connection status: connected`, the
`Power: KICKR` role in the last `Device List:`, and the indoor-bike-data / cycling-power
notify subscriptions in `trainer.log`. A fresh pairing assigns only the Power role; Cadence
and Controllable Trainer show up on some layouts or after re-selecting, so they are not
required. Zwift reports the connection asynchronously, so `check_trainer` polls the log for
the connected status (30 s) instead of sampling it once.

The last assertion is the shutdown disconnect: `stop_game` stops the winedevice group
(`pairing-stop.sh`, see `.kb/wine-fork-bluetooth.md`) before killing the prefix, because that is the
only path that reaches winebth's device-removal disconnect. A `wineserver -k` on its own
would make the assertion unpassable — it was, until the group stop was added.

The stub's `--token-file` writes a temporary fake token file under `~/snap/zwift/common`,
removed on exit; token refresh goes through the stub. No login-form clicks or credentials are
needed.

## Bluetooth in the pairing test

bluezoo (the same BlueZ D-Bus mock as `tests/bluetooth.sh`, via
`tests/tools/bluetooth/bluezoo.sh`) runs two adapters: hci0 is the PC, hci1 is a BLE smart
trainer named KICKR
(`tests/tools/bluetooth/bluetooth-trainer.py`: FTMS,
Cycling Power, Device Information, supporting standard services and the Zwift custom service
over GATT, streaming 150 W / 85 rpm).
`ROUVY_BT_ADAPTER=hci0` keeps wine on the PC adapter, and
`tests/tools/bluetooth/bluezoo-run.py` gives the trainer a
changing RSSI so wine keeps reporting its advertisements to Zwift.

On the pairing screen Zwift starts a native BLE search (BleWin10Lib); wine's advertisement
watcher turns it into a BlueZ `SetDiscoveryFilter(Transport="le")` + `StartDiscovery` on the
default radio. Classic BR/EDR devices are therefore never reported to Zwift.
`tests/pairing.sh --explore` keeps the game on the pairing screen for an hour to watch and
click live (game log, `bluezoo.log`, `trainer.log`, `tests/tools/yarf/yarf.sh`); run it detached
so an interrupted caller does not close the game. Explore mode also tolerates a failed yarf
suite and skips the post-conditions below, so a failing run can be examined live instead of
tearing the game down the moment it fails. Set `WINEDEBUG` (e.g. `+bluetooth,+winebth`, channels
comma-separated) to trace wine's side.

To reproduce with Bluetooth tracing on the nested desktop, run inside the workshop:

```shell
SMOKE_DISPLAY=nested WINEDEBUG=+bluetooth,+winebth tests/pairing.sh
```

## The Zwift custom GATT service (crash surface)

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

## Artifacts and misc gotchas

On success and failure, artifacts are in `~/zwift-pairing-out/`: `pairing-screen.png`, the yarf
`log.html` (screenshots yarf compared), `yarf.log`, `stub/requests.log` (the requests the game
made, with request bodies redacted), `bluezoo.log`, `trainer.log`, `scan.log`, `game.log`,
`stop.log` and the per-mode `mir-*.log`.

- A killed run skips its EXIT trap and leaves the stub CA in `cacert.pem`; the next run strips it
  before taking its backup.
- yarf's OCR (RapidOCR on onnxruntime) uses every core by default: one pass over a 1280×1024
  frame cost ~32 s of CPU. `tests/tools/yarf/yarf_ocr.py`, imported as a Robot `Library` by both
  suites (and staged next to them), caps it at 4 intra-op threads (~6 s CPU, faster wall time).
  yarf's own match loops already sleep at least 0.1 s per iteration.
- `tests/tools/yarf/yarf.sh` (the `yarf` action) sends pointer/keyboard input and takes screenshots
  of the running compositor (yarf's own wayland clients inside the yarf snap), for exploring the
  screens by hand when updating the suite for a new Zwift version.
