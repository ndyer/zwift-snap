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
  form and clicks through the onboarding screens to the pairing screen. tests/pairing.sh then
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

bluezoo (the same BlueZ D-Bus mock as `tests/bluetooth.sh`, via `tests/lib/bluezoo.sh`) runs so
the pairing screen has adapters to talk to, but Zwift still finds no devices: wine 11.21 does not
implement the BLE advertisement watcher Zwift uses (see the top-level README, "Bluetooth does not
work in Zwift"). The saved `pairing-screen.png` shows the Bluetooth tile with its warning icon;
the test asserts only that the pairing screen is reached, not the Bluetooth state.

## Artifacts

On success and failure, artifacts are in `~/zwift-pairing-out/`: `pairing-screen.png`, the yarf
`log.html` (screenshots yarf compared), `yarf.log`, `stub/requests.log` (the requests the game
made, with request bodies redacted), `bluezoo.log`, `scan.log` and the per-mode `mir-*.log`.

## Driving the game by hand

[`tests/tools/yarf.sh`](../tools/yarf.sh) (the `yarf` workshop action) takes screenshots of and
sends pointer/keyboard input to the running compositor, for exploring the screens by hand
(for example while updating this suite for a new Zwift version). It runs yarf's own wayland
clients inside the yarf snap. See the comments in that script.
