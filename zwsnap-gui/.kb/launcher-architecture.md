# Preface

The launcher's runtime architecture: what the binary owns, how the window and the
WebKit login panel behave, how cancellation and logging work, and where the shared
runtime support lives. Read this before changing `zwsnap-gui/src/`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## The binary

The `zwsnap-gui` binary is the snap's default `zwift` app. It owns installation,
updates, display preferences, process supervision, cancellation and logging.
GTK4 widgets are built programmatically with gtk-rs.

## Download and sync

Zwift is downloaded by this crate, not by Zwift's installer: `cdn.rs` transfers
from Zwift's CDN, `manifest.rs` parses the served version and file manifests,
`download.rs` fills a worker pool that verifies each file's length and checksum
before moving it into place, and `sync.rs` diffs the installed files against the
served manifest so an update transfers only what changed. `ZWIFT_CDN_BASE` points
the launcher at another CDN root (`file://` in the tests) and `ZWIFT_DOWNLOAD_JOBS`
sets the number of parallel transfers. See `.kb/zwift-cdn-sync.md` for the full
description.

## Window, display mode and login panel

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

The very first run loads the `?registration=true` variant of the embed page instead:
the panel writes a marker at `$SNAP_USER_COMMON/first-run` on its first construction
and treats a later run as returning. The marker is independent of login state, and a
`--preview` run never consumes it, so preview runs stay reproducible.

Link clicks and new-window requests in the panel are handed to the system browser
with `xdg-open` rather than followed in the panel; the launcher's own login flow is
unaffected because the launcher loads the Keycloak page itself and the user completes
it by submitting a form. `ZWIFT_OPEN_COMMAND` overrides the opener, for tests.
Inside the snap `xdg-open` must be the base's `/usr/bin/xdg-open`, which is snapd's
proxy to the host's handler through the `desktop` plug (`snapctl user-open`).
`xdg-utils` is therefore not staged: a staged copy would sit in `$SNAP/usr/bin`, win
the PATH lookup ahead of `/usr/bin`, and try to open the URL inside the snap instead
(see `snap/.kb/snap-specifics.md`).

Every handoff is observable. The panel takes the launcher `Logger` and records each
WebKit policy request at debug (`WebKit requested a navigation|new window to <uri>`),
the handoff at info, and the opener's outcome through a helper thread that waits on
the child: a clean exit is debug, a non-zero exit is a warning carrying the handler's
own stderr. Nothing goes to `eprintln!`, because a GUI launch has no visible stderr;
link handling therefore lands in `zwift.log` and the in-app log view like everything
else. This is what distinguishes the two failure modes when a user reports dead
links: no `WebKit requested` line at all means the click never reached
`decide-policy`, while a `warning: ... user-open error: cannot open supplied URL`
means the snap did its part and the *host* has no registered URL handler. The latter
is expected on a headless host: the workshop container ships no browser, so link
clicks there always end in that warning even though the same snap works on a desktop.
The token file in `$SNAP_USER_COMMON/pairing-token.json` has owner-only permissions,
and its compact JSON is read at launch time and forwarded as `--token=<json>`.
`ZWIFT_NO_GUI=1`, no display, or failed GTK initialization selects terminal mode,
which retains the game's own login flow rather than requiring browser authentication.

## Logging and cancellation

Rust drains stdout/stderr, writes `$SNAP_USER_COMMON/zwift.log` (rotating `.1`)
and forwards output directly to the GUI. SIGINT, SIGTERM, SIGHUP and closing the
window cancel subprocesses and polling. Wine server cleanup has independent
10-second command timeouts.

## Shared runtime support

One runtime support remains in `../zwsnap-runtime/`:

- `wine-env.sh` is the shared command-chain wrapper for all three snap apps.
  Library paths must be set before the dynamic loader starts the GTK executable;
  it also prepares Wine, GTK caches, GStreamer and audio.

`pulse-client.conf` supplies the audio client configuration.

## Windows helpers

The two Windows helpers this crate runs come from the sibling
[`../zwsnap-helpers/`](../zwsnap-helpers/) crate: `ZwiftLauncher.exe` is the
parent process `ZwiftApp.exe` demands and `wait-for-window.exe` reports when the
game window is up. Both use Win32 rather than GTK: GDK can enumerate its own
windows, not another process's Wayland windows. Winetricks remains an external
tool.
