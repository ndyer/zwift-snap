# Preface

How to develop and test the launcher: the toolchain, the build-ordering rule that
keeps launcher changes cheap, what each preview/test action covers, and the CLI.
Read this before wiring the crate into snapcraft or adding a test.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Toolchain and build ordering

The Workshop Rust SDK provides the development toolchain. Snapcraft's Rust plugin
uses rustup stable and its default locked install with `gtk-ui` enabled.
The final `runtime-prune` part retains this binary and its GTK dependencies after
all runtime parts are primed. Wine has no dependency on this crate, so changing
the launcher does not invalidate Wine's build.

```sh
workshop run lint
workshop run test
workshop run gtk-preview
workshop run build
```

## What each test covers

`gtk-preview` tests simulated install failure/retry, cancellation and both display
modes, plus the real lifecycle against stub Wine: the update check that finds the
stub CDN's version already installed, live logs, the mandatory login gate (nothing
launches before it) and the cancellation cleanup. Rust unit tests
verify that the worker cannot start the game before login confirmation and reads
fresh token JSON only at launch. Integration tests execute the binary in terminal
mode against a local `file://` CDN, including the cancellation path and SIGTERM.

After installing a build, `workshop run smoke` tests the real confined launcher up
to the mandatory login gate without using an account. Authenticated game startup
requires a manual login test. Bluetooth and pairing tests exercise the packaged
Wine and shared environment, with fake login servers used by the pairing test.

## Command line

Run the binary without arguments to launch, `-- GAME_ARGUMENTS...` to forward
game arguments, `--preview` for the simulated UI, or `set-display-mode PREFS_FILE`
to update an existing preferences file. Normal launch requires the snap's
environment (`SNAP`, `SNAP_USER_COMMON` and `wine-env.sh`).
