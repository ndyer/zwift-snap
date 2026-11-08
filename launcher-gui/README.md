# Rust Zwift launcher

The `zwift-gui` binary is the snap's default `zwift` app. It owns installation,
updates, display preferences, process supervision, cancellation and logging.
GTK4 widgets are built programmatically with gtk-rs.

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

Two runtime supports remain in `../launcher/`:

- `wine-env.sh` is the shared command-chain wrapper for all three snap apps.
  Library paths must be set before the dynamic loader starts the GTK executable;
  it also prepares Wine, GTK caches, GStreamer and audio.
- `wait-for-window.c` builds a Windows PE helper using Win32 window enumeration.
  GTK can enumerate its own windows, not another process's Wayland windows.
  The helper therefore stays inside Wine rather than being replaced by GDK calls.

`pulse-client.conf` supplies the audio client configuration. Winetricks and
`runfromprocess-rs.exe` remain external tools, not reimplemented by this crate.

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
modes, plus the real lifecycle with stub Wine for update failure, live logs,
Launch anyway, the mandatory login gate and cancellation cleanup. Rust unit tests
verify that the worker cannot start the game before login confirmation and reads
fresh token JSON only at launch. Integration tests execute the binary in terminal
mode, including failures and SIGTERM.

After installing a build, `workshop run smoke` tests the real confined launcher up
to the mandatory login gate without using an account. Authenticated game startup
requires a manual login test. Bluetooth and pairing tests exercise the packaged
Wine and shared environment, with fake login servers used by the pairing test.

Run the binary without arguments to launch, `-- GAME_ARGUMENTS...` to forward
game arguments, `--preview` for the simulated UI, or `set-display-mode PREFS_FILE`
to update an existing preferences file. Normal launch requires the snap's
environment (`SNAP`, `SNAP_USER_COMMON` and `wine-env.sh`).
