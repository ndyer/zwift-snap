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
  launcher dance of `launcher/zwift-run.sh`, without a login token) rather than through the real
  launcher, so the Zwift patcher never runs and cannot overwrite the CA.
- The stub answers the OAuth token endpoint with a fake (unsigned) token and the handful of game
  API calls the game blocks on before the pairing screen: `/api/auth`, `/api/users/login` (a
  protobuf `LoginResponse`), `/api/profiles/me` (a minimal protobuf profile), the world list and
  a few empty 200s. The protobuf shapes come from
  [zwift-offline](https://github.com/fatsbrown/zwift-offline).
- [`tests/pairing/pairing.robot`](pairing.robot) drives the UI with yarf: it logs in through the
  form and clicks through the onboarding screens to the pairing screen, then pairs the mock
  trainer (see Bluetooth below). tests/pairing.sh then
  confirms in the game log that it reached `ZSF_PAIRING_SCREEN` (yarf cannot read the snap's data
  directory) and saves a screenshot of the pairing screen.

The suite finds every button by its on-screen text (OCR), which is resolution independent. The
login form is the exception: its Username and Password fields are low-contrast placeholders OCR
cannot read, so they are clicked at a fraction of the vertical span between two text anchors it
can read, "LOG IN" and "create new user". The suite branches on the two ways the login form is
reached: a first run opens on a SIGN UP screen with a link to the form and empty fields; a
returning run opens on the LOG IN form with the email already filled, so only the password is
typed. The onboarding screens (EULA, sport, reason) are each optional, because a returning
profile skips them.

## Bluetooth

bluezoo (the same BlueZ D-Bus mock as `tests/bluetooth.sh`, via
`tests/tools/bluetooth/bluezoo.sh`) runs two adapters: hci0 is the PC, hci1 is a BLE smart
trainer named KICKR
([`tests/tools/bluetooth/bluetooth-trainer.py`](../tools/bluetooth/bluetooth-trainer.py): FTMS,
Cycling Power and Device Information over GATT, streaming 150 W / 85 rpm).
`ROUVY_BT_ADAPTER=hci0` keeps wine on the PC adapter, and
[`tests/tools/bluetooth/bluezoo-run.py`](../tools/bluetooth/bluezoo-run.py) gives the trainer a
changing RSSI so wine keeps reporting its advertisements to Zwift.

After reaching the pairing screen (detected by its POWER SOURCE tile, since a "No ANT+ dongle"
tip can cover the PAIRED DEVICES title), the suite pairs the trainer: POWER SOURCE → PAIR DEVICE
→ KICKR → CLOSE. Clicking a connected tile would unpair the device, so a returning profile is
first given time to reconnect by itself. tests/pairing.sh then checks in the game log that KICKR
connected and holds the Power role, and in `trainer.log` that Zwift subscribed to the power and
indoor bike data notifications. Cadence and Controllable Trainer roles are not required: a fresh
pairing assigns only Power.

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
