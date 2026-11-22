# Preface

What to run before declaring a task done, and the stale-data trap that makes a
smoke test silently exercise the old install instead of the new one.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Before declaring a task done

Before declaring a task done run `lint` and `test`. Run `smoke` when the change
affects anything the launcher does at runtime (plugs, command-chain, wine
environment, dialogs).

CI runs `lint`, `test`, `gtk-preview`, `inhibit`, `build`, `install` and `bluetooth`
on every change to `main`, so a green run there covers those; `smoke` and `pairing` stay
local because they need a host display (see `.github/.kb/edge-build-workflow.md`).

A **tagged release** runs more: `release.yml` runs every suite above *plus* `smoke` and
`pairing` on a virtual display before uploading the snap artifact, and `publish.yml` then
pushes it to `latest/beta` and creates the GitHub Release (see
`.github/.kb/release-workflow.md` and `.kb/release-procedure.md`).

The BLE connection-interval override (`bt_tuning`) has no runnable end-to-end check:
bluezoo mocks only the BlueZ D-Bus interface, not the raw kernel management socket that
`bluetooth-control` unlocks, and the workshop's own `hci0` is rfkill-blocked. Verify it with
the unit tests in `bt_tuning.rs` (`cargo test`), plus — on real BLE hardware — a
`sudo snap connect zwift:bluetooth-control` and a `btmon` capture showing the override land
after BlueZ's 0x2a04 update.

## Screensaver inhibitor

The inhibitor's D-Bus marshalling is checked by `workshop run -- inhibit`, which fakes
the session's screen-saver services on a private bus (see `tests/.kb/inhibit-test.md`).
The matching cargo test is opt-in, so a plain `cargo test` silently skips it: run the
workshop action, not the test binary, when touching `inhibit.rs`.

## Stale snap data

**Stale snap data**: reinstalling the snap (`snap install --dangerous --amend`)
does not remove the old wine prefix in `~/snap/zwift/common`. Before smoke
testing a fresh build, purge first (`sudo snap remove --purge zwift`), install
the new snap and reconnect the `gpu-2604` interface — otherwise the test runs
against the stale installation and the new wine is never exercised. `tests/smoke.sh`
expects the snap to be installed beforehand and never installs or removes it.
