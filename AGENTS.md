# AGENTS.md

Guidance for AI coding agents working in this repository.

## What this project is

An unofficial snap packaging of [Zwift](https://zwift.com) for Linux. Zwift itself is
proprietary and NOT part of the snap — the launcher scripts download and update it
into a wine prefix at runtime. Wine is built from source by the snap's `wine` part:
[ndyer/wine](https://github.com/ndyer/wine), a fork of the official wine-mirror/wine
GitHub mirror, at the `source-tag` in `snap/snapcraft.yaml` (currently `wine-11.21`,
new-style WoW64, no i386 host libs). Strict confinement, amd64 only, base `core26`.
The old `core24` variant of the snap is gone from the tree; it only lives in git history (pre-`6e31f55` commits
or the `origin/core26` / `v0.11` refs).

## Layout

```
launcher/          source of the launcher scripts (zwift-run/gui/setup/common.sh, wine-env.sh)
snap/snapcraft.yaml     the snap definition (parts, prune filters, plugs, layouts)
snap/prune-closure.sh   DT_NEEDED closure walk that prunes unreachable libraries
tests/             unit tests (run-tests.sh + scenarios.sh + stubs) and the visual smoke test
  .workshop/         workshop definition (build environment) + in-project SDK zwift-snap-sdk
overlay/, prime/, stage/, parts/   build trees, gitignored, never edit by hand
```

The snap has three apps: `zwift` (the launcher), `wine`, `winecfg`; the last two let
users run arbitrary wine commands. All share the command-chain
`gpu-2604-wrapper -> wine-env.sh`.

## Build and verification (workshop)

Builds happen in a [Workshop](https://ubuntu.com/workshop/docs/) VM in destructive
mode. All commands run from the project root:

```shell
workshop run zwift-snap build    # sudo snapcraft pack --destructive-mode
workshop run zwift-snap test     # unit tests of the launcher scripts (fast, no GUI)
workshop run zwift-snap smoke    # visual smoke test with yarf, ~2 min warm, see below
workshop run zwift-snap install  # sudo snap install --dangerous ./zwift_*.snap
workshop run zwift-snap lint     # shellcheck + yamllint
workshop run zwift-snap size     # where the bytes are
workshop run zwift-snap stats    # pkgproxy cache savings
```

There is no `setup` action: snapcraft, shellcheck, yamllint and the pkgproxy cache
are installed by the `setup-base` hook of the `zwift-snap-sdk` SDK at launch and
refresh (see `.workshop/zwift-snap-sdk/hooks/setup-base`).

When running `workshop run zwift-snap build` (or any long-running action) from an
agent, pipe the output through `tee` instead of swallowing it with `tail`: a
piped `tail` hides the build's live progress (and makes it look hung), while
`tee` shows the output as it streams and keeps it in a file for later reading,
for example `workshop run zwift-snap build | tee build.log`. Keep `pipefail` in
mind when chaining commands afterwards.

- **Stale snap data**: reinstalling the snap (`snap install --dangerous --amend`)
  does not remove the old wine prefix in `~/snap/zwift/common`. Before smoke
  testing a fresh build, purge first (`sudo snap remove --purge zwift`), install
  the new snap and reconnect the `gpu-2604` interface — otherwise the test runs
  against the stale installation and the new wine is never exercised. `tests/smoke.sh`
  expects the snap to be installed beforehand and never installs or removes it.

The canonical agent documentation for operating Workshop is the vendored skill under
`.github/skills/` (`use-workshop`, `onboard-workshop`, `design-sdk`, imported from
github.com/canonical/use-workshop-skill). Route Workshop CLI questions there — in
particular `.github/skills/use-workshop/references/in-project-sdk.md` for the SDK
layout and hook rules used by this project.

Before declaring a task done run `lint` and `test`. Run `smoke` when the change
affects anything the launcher does at runtime (plugs, command-chain, wine
environment, dialogs).

The workshop definition is `.workshop/zwift-snap.yaml`; actions are plain bash
with `errexit`/`pipefail` set. In-project SDKs are listed with a `project-` prefix in
`sdks:` (`project-zwift-snap-sdk` for `.workshop/zwift-snap-sdk/sdk.yaml`), but plug and
slot references elsewhere (e.g. `connections:`) use the bare name (`zwift-snap-sdk`),
because the `project-` prefix only selects the source and is not part of the name.
The workshop desktop and gpu interfaces are NOT auto-connected, the `connections:`
entries are required.

## Visual smoke test (yarf)

`tests/smoke.sh` starts a `mir-test-tools` Mir compositor, runs the real launcher
against it and drives the Robot Framework suite in `tests/yarf/` with
[yarf](https://documentation.ubuntu.com/yarf): test 1 requires the yad progress
dialog (template match on its static title bar), test 2 requires the Zwift window
(`zwift-login.png`). The first run on a fresh wine prefix installs wine prerequisites
and downloads Zwift (tens of minutes), later runs are quick. No login is performed.

On failure check `~/zwift-smoke-out/`: `yarf.log`, `log.html` (screenshots yarf
compared), `zwift.out`, the per-mode `mir-<mode>.log`, and the tail of
`~/snap/zwift/common/zwift.log` that the harness prints.

Two logfiles show live progress while the suite runs:

- `~/snap/zwift/common/zwift.log` — the launcher log: winetricks steps
  (`Executing` lines), the Zwift download, the launcher's own state.
- `~/snap/zwift/common/.wine/drive_c/users/<user>/AppData/Local/Zwift/Logs/Log.txt`
  — Zwift's own log, which only exists once the game has started.

Window decorations and themes differ between machines: when the suite reports a
missing template, recapture it with the recipe in `tests/yarf/README.md` and commit
it. `SMOKE_DISPLAY=nested|headless` overrides the auto detection; nested renders on
the host display through the workshop desktop interface.

Yarf gotchas that cost time to discover (all verified live):

- The yarf snap is strict: it sees only `$HOME` — not `/project`, not `/tmp`, and not
  `~/.cache`. The harness stages the suite to `~/zwift-smoke-suite` for that reason.
- yarf exits 120 with EMPTY output when its stdout is a regular file; piped output is
  fine. The harness pipes it through `tee` (with `pipefail`).
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

## Snap specifics that are easy to break

- **Pruning**: the wine part aggressively prunes (man pages, locales, wine's build
  toolchain, ~776 static import libraries, plus everything unreachable via the
  `snap/prune-closure.sh` closure walk). Guards in the part's `override-prime` fail
  the build on over-pruning (runtime file missing) or a stale prime directory (needs
  `workshop run zwift-snap clean`). Bump the `v<N>` comment there when editing
  `prune-closure.sh`, otherwise craft-parts will not re-run the prime step.
- **Do not prune** `usr/lib/*/samba` (ntlm_auth needs it) and the nss libraries
  listed in the `lint.ignore` block of `snap/snapcraft.yaml` (dlopened by name).
- **Wine source**: the `wine` part builds [ndyer/wine](https://github.com/ndyer/wine)
  (fork of wine-mirror/wine) at the `source-tag` in `snap/snapcraft.yaml`; a git
  source only rebuilds when the tag changes, unlike a local source. To update
  wine, change `source-tag`. For snap-specific patches, put them on a branch in
  the fork and point the part at that branch with a pinned `source-commit`.
- **pkgproxy** caches apt and snap downloads in the workshop; the base snap,
  https sources and the wine git clone are not cached. The `stats` action shows
  the savings.
- **No `home` plug**: wine user folders live inside the wine prefix
  (`~/snap/zwift/common/.wine/drive_c/users/<user>/`); the launcher relinks them to
  the real home at every start.
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
