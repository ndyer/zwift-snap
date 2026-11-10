# Preface

How to start the game yourself, without the launcher script, when debugging the
pairing screens or wine behaviour. Read this before editing `tests/pairing.sh`'s
task helpers — the parent-process requirement and the wrong default prefix both
bite immediately.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Driving the game by hand (no launcher script)

`zwift.wine` defaults to `WINEPREFIX=~/snap/zwift/x1/.wine`, not the real prefix, so set
`WINEPREFIX=$HOME/snap/zwift/common/.wine` (inside the snap `$SNAP_USER_COMMON/.wine`).
Starting `ZwiftApp.exe` directly does not work: it reads its parent's image path and only
runs under `ZwiftLauncher.exe`, `ZwiftHelper.exe` or `devenv.exe`. The launcher's sequence in
`zwsnap-gui/src/launch.rs` is one command — `wine start /d <install dir> ZwiftLauncher.exe
ZwiftApp.exe <args>` — where `ZwiftLauncher.exe` is the snap's own shim
(`zwsnap-helpers/src/shim.rs`), copied next to `ZwiftApp.exe` first. Zwift's real launcher
is not involved anywhere. Run it from a script file in `~/snap/zwift/common` through
`snap run --shell zwift.wine -c 'bash "$HOME/../common/script.sh"'` and adapt
the test-local task helpers in `tests/pairing.sh`. Use `ZwiftApp.exe --token=<json>` to skip the
login form; the game log is `AppData/Local/Zwift/Logs/Log.txt` in the prefix user dir.
A hand-made token is accepted by the client, but the game then refreshes it at
`secure.zwift.com`, whose real 400 `invalid_grant` shows as "Oops. Wrong email/password".
