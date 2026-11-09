# Rust Zwift launcher

The `zwift-gui` binary is the snap's default `zwift` app. It owns installation,
updates, display preferences, process supervision, cancellation and logging.
GTK4 widgets are built programmatically with gtk-rs.

Zwift is downloaded by this crate, not by Zwift's installer: `cdn.rs` transfers
from Zwift's CDN, `manifest.rs` parses the served version and file manifests,
`download.rs` fills a worker pool that verifies each file's length and checksum
before moving it into place, and `sync.rs` diffs the installed files against the
served manifest so an update transfers only what changed. `ZWIFT_CDN_BASE` points
the launcher at another CDN root (`file://` in the tests) and `ZWIFT_DOWNLOAD_JOBS`
sets the number of parallel transfers.

The GTK window and game share `ZWIFT_DISPLAY_MODE`: fullscreen by default, or
`WIDTHxHEIGHT` windowed. The window hides after the first game window appears;
the launcher stays alive until the game exits. Errors retain the in-app log view.
Installation failures offer Retry; update failures offer Launch anyway.
A WebKitGTK 6.0 panel is always shown in the bottom section of the window: it starts
on Zwift's launcher embed page and, once installation/update finishes and the login
gate opens, navigates automatically to the OAuth sign-in page (PKCE and state
validation protect the exchange). A successful login leaves the page as-is and lets
the launch continue; an unsuccessful exchange reloads the login page so it can be
retried, and closing the launcher cancels the wait. Remember-me cookies persist in
`$SNAP_USER_COMMON/webkit`.
The token file in `$SNAP_USER_COMMON/pairing-token.json` has owner-only permissions,
and its compact JSON is read at launch time and forwarded as `--token=<json>`.
`ZWIFT_NO_GUI=1`, no display, or failed GTK initialization selects terminal mode,
which retains the game's own login flow rather than requiring browser authentication.

Rust drains stdout/stderr, writes `$SNAP_USER_COMMON/zwift.log` (rotating `.1`)
and forwards output directly to the GUI. SIGINT, SIGTERM, SIGHUP and closing the
window cancel subprocesses and polling. Wine server cleanup has independent
10-second command timeouts.

One runtime support remains in `../launcher/`:

- `wine-env.sh` is the shared command-chain wrapper for all three snap apps.
  Library paths must be set before the dynamic loader starts the GTK executable;
  it also prepares Wine, GTK caches, GStreamer and audio.

`pulse-client.conf` supplies the audio client configuration.

The two Windows helpers this crate runs come from the sibling
[`../launcher-windows/`](../launcher-windows/) crate: `ZwiftLauncher.exe` is the
parent process `ZwiftApp.exe` demands and `wait-for-window.exe` reports when the
game window is up. Both use Win32 rather than GTK: GDK can enumerate its own
windows, not another process's Wayland windows. Winetricks remains an external
tool.

## Development

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

Run the binary without arguments to launch, `-- GAME_ARGUMENTS...` to forward
game arguments, `--preview` for the simulated UI, or `set-display-mode PREFS_FILE`
to update an existing preferences file. Normal launch requires the snap's
environment (`SNAP`, `SNAP_USER_COMMON` and `wine-env.sh`).
