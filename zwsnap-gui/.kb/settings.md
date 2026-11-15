# Preface

The launcher's user-facing settings: where the file lives, how it is seeded, the keys it
holds, how `display_mode` is written into the game, and how the Settings dialog edits it.
Read this before adding, renaming or repurposing a settings key or touching
`zwsnap-gui/src/settings.rs` or `zwsnap-gui/src/display.rs`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## The settings file

The launcher's options live in `$SNAP_USER_COMMON/settings.json`
(`~/snap/zwift/common/settings.json`). The snap does not create it directly:
`settings::ensure_initialized` copies the read-only template
`$SNAP/etc/zwift/settings.json` into place on first run, written 0600. An existing user
file is never overwritten, so a snap upgrade does not lose edits, while a missing or
invalid field in a stale file falls back to its built-in default — which is what lets a
newer snap add a key without breaking an old file. Saves are atomic (a temporary file
renamed over the target) and 0600.

`Settings::load` reads the file before GTK starts, so it reports problems on stderr (there
is no logger yet) and never fails a launch: unreadable, non-JSON, non-object and
wrongly-typed values all fall back to the defaults with a warning. A change made in the
Settings dialog therefore takes effect on the *next* launch, which the dialog says
explicitly.

## Keys

- `display_mode` — the display mode shared by the launcher window and the game:
  `fullscreen` (the default) or a resolution like `1920x1080`. A custom resolution implies
  windowed mode (fullscreen is off). See "Display mode" below.
- `graphics_profile` — which of the game's own graphics profiles to write: `auto` (the
  default), `basic`, `medium`, `high`, `ultra` or `default`. See
  `.kb/graphics-profile.md`.
- `disable_egl` — `true` disables EGL in Wine, forcing GLX.
- `bluetooth_debug` — `true` enables Wine's `+bluetooth` and `+winebth` debug channels for
  hardware testing. See "Bluetooth debug tracing" below.
- `download_adapt` — `true` (the default) grows the CDN download pool toward
  `ZWIFT_DOWNLOAD_JOBS` until the link saturates, instead of a fixed count. See
  `.kb/zwift-cdn-sync.md`.
- `verbosity` — log verbosity, `0` (quiet) to `3` (debug); the default is `1`.

## Display mode

`display::configure_file` writes `display_mode` into the installed game's preferences file,
`<prefix>/drive_c/users/<user>/AppData/Local/Zwift/prefs.xml`. Only the `<CONFIG>` section
is touched: it sets `FULLSCREEN` (`1` fullscreen, `0` windowed, keeping any existing
`USER_RESOLUTION_PREF` in fullscreen) and, for a resolution, `USER_RESOLUTION_PREF` to
`WIDTHxHEIGHT`. A fresh install's self-closing `<CONFIG/>` is expanded and a missing
section is created, with the rest of the file preserved. The file is replaced atomically
and left untouched when it cannot be parsed or updated; in that case the launcher reports
`Unable to update display mode` and refuses to launch, rather than start the game in the
wrong mode.

## Bluetooth debug tracing

With `bluetooth_debug` set, the launcher appends `+bluetooth` and `+winebth` to
`WINEDEBUG` before starting the game, so the wine Bluetooth driver's trace is written
wherever the game's output goes — `$SNAP_USER_COMMON/zwift.log` and the terminal. This is
the tracing used to diagnose device discovery and pairing; it is off by default and the
trace is verbose, so leave it off in normal use.

## The Settings dialog

`gui/settings.rs` builds the GTK dialog behind the window's Settings button. It edits a
copy of the loaded settings and writes them back on **Save**; the file is read before GTK
starts, so the dialog states that changes apply on the next launch. Alongside the editable
fields it shows a read-only status block (the account token's presence, the `bluez` plug
connection, and the graphics probe result) with a **Run graphics probe** button, and it
offers the maintenance actions that reset the Wine prefix or delete the downloaded game
data. It no longer contains the log view, which is now the window's own log toggle.
