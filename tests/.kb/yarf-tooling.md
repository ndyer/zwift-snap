# Preface

The `yarf` workshop action: taking screenshots of and sending input to the running
game for by-hand debugging. Read this before reaching for a host screenshot tool —
`grim` does not work here, and the yarf snap's filesystem view constrains where
inputs and outputs may live.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Screenshots and input for the running game

To see what Zwift shows (login form, error messages, pairing screen) while debugging, or to
click/type into it, start a compositor and the game as `tests/smoke.sh` does, then use
`tests/tools/yarf/yarf.sh` (the `yarf` workshop action):

```
tests/tools/yarf/yarf.sh screenshot [-s wayland-99] [OUTPUT.png]   # save one frame, view the PNG
tests/tools/yarf/yarf.sh input [-s wayland-99] click 0.5 0.8 then type foo then key Return
```

The default screenshot output is `~/zwift-yarf/shot-<time>.png`. The `input` commands are
`click X Y` / `move X Y` (X, Y are 0.0-1.0 proportional), `type TEXT`, `key NAME`, `sleep SECONDS`.

## Do not use grim

**Do not use `grim`.** It is installed in the workshop but fails against Mir with
`wl_shm@N.create_pool: invalid arguments` / `failed to screenshoot all outputs`, whatever
the options (`-s`, `-t`) and timing (reproduced against an idle, freshly started Mir).
Connecting `mir-test-tools:wayland` does not change that: the `demo-server` app does not
even declare that plug. Not root-caused; do not spend more time on it.

## How the script works around the yarf snap

- The script runs `tests/tools/yarf/screenshot.py` / `input.py` inside the yarf snap, using yarf's own
  `zwlr_screencopy_unstable_v1` and virtual pointer/keyboard clients (`yarf.lib.wayland.*`), the
  same capture/input path as the smoke and pairing tests. Mir must have been started with
  `zwlr_screencopy_manager_v1` in `--add-wayland-extensions` (smoke.sh does).
- yarf's `python3` only exists inside its snap (`/snap/yarf/*/bin/python3` is a dangling
  symlink on the host), so go through `snap run --shell yarf -c ...`.
- Inside the yarf snap `$HOME` is remapped to `~/snap/yarf/<rev>`; the real home is
  `$SNAP_REAL_HOME`. yarf can read and write non-hidden directories under the real home
  (hence `~/zwift-yarf`), but not `/tmp` or `/project`.
- yarf also has its own `XDG_RUNTIME_DIR` (`/run/user/<uid>/snap.yarf`): the compositor
  socket must be symlinked into it, which the script does.
- A black frame means the compositor works but no window is up yet; take it later.
