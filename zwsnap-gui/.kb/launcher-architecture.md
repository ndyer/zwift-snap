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
caps the number of parallel transfers (default 32); with `download_adapt` on (the
default in `settings.json`) the pool grows toward that ceiling until the link
saturates instead of running a fixed count. See `.kb/zwift-cdn-sync.md` for the full
description.

## Install progress

The window moves through three phases: the install area, the account/login gate,
and the launch spinner. `install.rs` defines a small event protocol in
`InstallProgress`:

- `InstallProgress::Task { task, state }`, where `task` is one of five
  `InstallTask`s (`WineSetup`, `D3dCompiler`, `Vulkan`, `UpdateCheck`, `Game`) and
  `state` is `Running`, `Fraction { percent, rate }` (percent 0..=99, `rate` an
  optional bytes-per-second transfer speed) or `Done`. The bar text renders
  `percent` and, when known, the rate through `stats::transfer_progress`
  (`"42% · 12.3 MB/s"`). `UpdateCheck` is the only
  tile-less one: nothing is transferred while the served version is read, so it has
  no fraction and is over in a moment; it only names the status line.
- `InstallProgress::Launch { message }`, emitted as the game starts.

On a fresh install the Wine prefix is created first (`wineboot`, serial, because
the game's paths depend on it), then two tracks run concurrently on a
`std::thread::scope`: `install_prerequisites` (the d3dcompiler then the Vulkan
renderer) and `install_game` (the CDN sync). The game track reports byte progress
as `Fraction`; the prerequisite tracks pulse while `Running` and go green on
`Done`, because there is no byte count to sample (`ZWIFT_EXPECTED_DRIVE_C_BYTES`
and `ZWIFT_PROGRESS_INTERVAL`, and the drive_c size sampler they configured, were
removed). An update drives the `Game` task, preceded by `UpdateCheck`.

Only transitions reach the log, never the ticks: `application.rs`'s `log_progress`
records a task starting and a transfer's ten-percent milestones
(`Downloading Zwift: 40%`), because the game transfer reports once per downloaded
file and would otherwise fill the log with one line per file.

Because the tracks run in parallel, task events interleave and no fixed order is
guaranteed; consumers must not assume one.

The GUI renders one tile per task in a horizontal, homogeneous row. Each tile is
built hidden and revealed when its task first reports progress; `Running` pulses
it, `Fraction` and `Done` set the bar. Each completed tile hides after two seconds,
independently of the others. A restarted task invalidates its previous timer. The
whole area hides immediately when the login gate opens or the launch begins. The status heading
carries the short message; `Launch` also starts a spinner immediately after the
text (the heading and spinner share an expanding box in the window header),
stopped again on `GameReady`. The heading is a single, non-wrapping line —
keeping long messages out of it is what stops a second row appearing — so a
failure's reason and the final error go to the log panel instead.

## Window, display mode and login panel

The GTK window and game share the `display_mode` setting (`settings.json`,
default fullscreen, or `WIDTHxHEIGHT` windowed). The window hides after the
first game window appears; the launcher stays alive until the game exits.
Installation failures offer a **Retry** button and update failures offer Launch
anyway; Retry is revealed in the header's button group rather than on a row of
its own. The window's top-right corner holds the launcher's controls, built by
`gui/icons.rs` from the **bundled** icon set (Tabler, MIT; compiled into the
binary as a GResource and registered by `icons::init`) — the launcher no longer
uses a system icon theme (`zwsnap-gui/.kb/icons.md`). `icon_button(name, label,
tooltip)` builds a `flat` 16 px button showing either the icon or, in text mode,
the label (see `zwsnap-gui/.kb/icons.md`): the picker's add-account button, a
`zwsnap-log-symbolic` log toggle, a `zwsnap-settings-symbolic` Settings button,
and — in a fullscreen session, which has no decorations to close from — a
`zwsnap-quit-symbolic` Quit button that runs the same cancellation and cleanup as
closing the window. The log toggle is the exception to the `flat`
rule: it stays full-contrast and accented while checked, so its highlight shows
whether the log pane is visible. Settings opens a dialog that edits the settings file, shows
token/`bluez`/GPU status and offers the maintenance actions (see
`gui/settings.rs`); the log view is no longer part of that dialog.
The bottom of the window is a split pane (`gtk::Paned`): a WebKitGTK 6.0 panel on
the left and the launcher log on the right, hidden until its toggle is activated,
so hiding it leaves the WebView the full width. When the toggle reveals the log,
`LogPanel::limit_width` moves the divider so the pane opens at no more than its
`MAX_WIDTH` (420 px) — the log wraps, so it needs little room, and the leftover
width goes to the WebView rather than stretching the log across a wide or
fullscreen window. The divider keeps the login pane at least `LOGIN_WIDTH` (560
px) and the log at least `MIN_WIDTH` (320 px), so a narrow window's split is
unchanged; the user can still drag the divider wider. The WebKit panel starts
on Zwift's launcher embed page and, once installation/update finishes and the login
gate opens, navigates automatically to the OAuth sign-in page (PKCE and state
validation protect the exchange). A successful login puts the embed page back and lets the launch continue; an
unsuccessful exchange reloads the login page so it can be retried, and closing the
launcher cancels the wait. Remember-me cookies persist in
`$SNAP_USER_COMMON/webkit`.

When at least one rider is remembered, the login gate is answered by the account
picker instead: a grid of rider cards appears at the top of the window, where the
install tiles were, above the WebView, which keeps showing its embed page. The
WebView leaves that page only for the gate's own login or "Add another account",
and `LoginPanel::show_embed` puts it back once the login succeeds. With no
accounts at all the gate behaves exactly as described above. The launcher decides
which embed page to load from the store, not from a marker: with no account
remembered (and not a `--preview` run) it opens on the `?registration=true`
variant, otherwise on the plain page. The same startup step also adopts a
pre-picker install (a launch token but no account list) as the first account, so
an upgrade is not sent to the registration page. See `.kb/accounts-picker.md` for
the store, the cards and the GTK4 traps in that code.

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
`ZWIFT_NO_GUI=1`, no display, or failed GTK initialization selects terminal mode. It has
no embedded-browser login gate and retains the game's own login flow (or accepts
explicitly supplied game arguments), and its failure handling differs from the GUI's:
`TerminalUi::failure` returns true only for `Failure::Update`, so an installation error
quits while an update error is non-fatal and the launcher still starts the installed
copy — where the GUI instead offers **Retry** for an install and **Launch anyway** for an
update.

## Logging and cancellation

Rust drains stdout/stderr, writes `$SNAP_USER_COMMON/zwift.log` (rotating `.1`)
and forwards output directly to the GUI. SIGINT, SIGTERM, SIGHUP and closing the
window cancel subprocesses and polling. Wine server cleanup has independent
10-second command timeouts.

## Screensaver inhibition

Zwift runs fullscreen and never asks the session to stay awake itself: it is a
Windows program inside wine, and the launcher only writes its display
preferences, so nothing on the session bus inhibits the idle timer and a rider
who moves only on the trainer can watch the screen blank mid-session. `inhibit.rs`
therefore takes an idle inhibitor for the duration of the game and releases it
afterwards.

The inhibitor is taken in `Runtime::start` and released at the top of
`Runtime::cleanup`, which `run_cancellable` always runs, so an aborted or failed
launch releases it too. It lives in the `Runtime` field `inhibitor` rather than in
a local of `start` because it has to outlive that call; `Backend` also implements
`Drop` as a last resort, and the desktop drops the inhibitor with the session bus
connection anyway.

Only a fullscreen session inhibits, because a windowed game leaves the desktop
visible and blanking it is unhelpful: with any other `DisplayMode`,
`ScreenInhibitor::acquire` returns the inactive backend and logs at debug why.

Two backends are tried in order, both over the session bus:

- `org.freedesktop.ScreenSaver` at `/org/freedesktop/ScreenSaver`, `Inhibit(s
  application, s reason) -> u` and `UnInhibit(u)`. This is the one KDE, GNOME's
  screensaver proxy and most compositors answer, including Mir.
- `org.gnome.SessionManager` at `/org/gnome/SessionManager`, `Inhibit(s app_id, u
  toplevel_xid, s reason, u flags) -> u` and `Uninhibit(u)`, with `flags` = `4 | 8`
  (logout and idle/screensaver). The toplevel xid is `0`, because the game window
  belongs to wine and not to this process.

Both calls are best-effort. A launch never fails because of them: when neither
service answers, the launcher logs at debug which reasons the two attempts gave
and carries on. When one does answer it logs the service and the cookie it was
handed, which is also what makes the behaviour observable in `zwift.log`.

Two details are easy to get wrong and produce a silently ignored inhibitor, so
`tests/inhibit.sh` checks them against fake services (see
`tests/.kb/inhibit-test.md`):

- The parameters of a D-Bus call must be a tuple, so one-argument `UnInhibit`
  passes `(cookie,)` and not a bare `u`.
- The reply is a tuple of the out arguments too, even for a single `u`, so the
  cookie comes from `reply.get::<(u32,)>()`.

The snap's `zwift` app already declares and connects the
`screen-inhibit-control` plug, which is what allows these calls under
confinement; no packaging change is needed.

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
windows, not another process's Wayland windows. The Wine prerequisites the game
needs are installed by the launcher itself rather than by an external tool.
