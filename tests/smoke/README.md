# Yarf smoke test

`workshop run smoke` starts a Mir compositor, runs the confined `zwift` Rust
launcher with `--skip-login` and drives this suite with yarf. It is
credential-free: the login gate is bypassed, so authenticated game startup is
tested manually (see the [pairing suite](../pairing/)).

The compositor renders on the host display through the workshop `desktop` interface
when one is available, otherwise on a virtual output. Nested rendering needs the
`desktop` and `gpu` interfaces connected to the host compositor: `SMOKE_DISPLAY=nested`
fails with the `workshop connect` commands to fix it when they are missing, while the
default `auto` falls back to the virtual output.

## Where to look

- [../.kb/smoke-test.md](../.kb/smoke-test.md) — how the suite works, the
  first-run cost, and how to read its diagnostics.
- [../.kb/dl-cache.md](../.kb/dl-cache.md) — the `~/zwift-cache` game download
  cache, the `dl-cache status`/`clear` actions, and `--fresh`.

## Recapturing icon templates

The launcher's header controls are icon-only, so the suites match small PNG
templates instead of text. Each suite keeps its own copy:

- `tests/smoke/settings.png` — the `preferences-other-symbolic` settings gear, the
  only thing that proves the GTK window is up (`smoke.robot`, test 1).
- `tests/gtk-preview/log.png` — the `text-x-generic-symbolic` log toggle
  (`preview.robot`, `runtime.robot`).
- `tests/gtk-preview/retry.png` — the failure Retry button (`preview.robot`).
- `tests/gtk-preview/remove.png` — the account card's remove icon
  (`accounts.robot`).

Recapture the relevant template when a suite reports "no match found" because a
theme or layout change moved or recoloured the glyph. Frames are 1280x1024.

1. Run the suite once so it dumps its frames and note the frames directory. For
   the GTK preview the harness prints `GTK preview artifacts: <stage>` and the
   frames are `<stage>/frames/`; for the smoke test they are
   `~/zwift-smoke-out/frames/`. `<frames>/index.log` maps each frame number to the
   wait that captured it, which identifies the frame showing the control (e.g. the
   `Match .../log.png` wait for the header, or the "Choose a rider" wait for a card).
2. Crop the glyph out with the PIL inside the yarf snap — the host python has no
   PIL, and the snap sees only `$HOME`, so keep both paths under `$HOME` and copy
   the result into the repo afterwards:

   ```sh
   cat > ~/crop.py <<'PY'
   import sys
   from PIL import Image
   l, t, r, b = (int(v) for v in sys.argv[2].split(","))
   Image.open(sys.argv[1]).crop((l, t, r, b)).save(sys.argv[3])
   PY
   snap run --shell yarf -c 'python3 "$HOME/crop.py" "$1" "$2" "$3"' _ \
       "$frames/0000000010.png" "1183,30,1204,51" "$HOME/settings.png"
   cp ~/settings.png tests/smoke/settings.png
   ```

   The boxes below are starting hints measured on a fullscreen 1280x1024 capture
   (the header controls sit at the window's top-right; crop a couple of pixels
   around the glyph):

   | template        | box (left,top,right,bottom) | size  |
   | --------------- | --------------------------- | ----- |
   | `settings.png`  | 1183,30,1204,51             | 21x21 |
   | `log.png`       | 1141,31,1158,51             | 17x20 |
   | `retry.png`     | 1095,31,1116,51             | 21x20 |
   | `remove.png`    | 885,304,906,322             | 21x18 |

   The `remove.png` box is from an account-picker frame, not the header; locate it
   from the "Choose a rider" frame.
3. View the crop to confirm it is a tight, clean glyph, then re-run the suite.
   Templates match at yarf's default tolerance (0.8), so a few pixels of slack is
   fine, but a crop that includes neighbouring chrome (hover highlight, the card
   edge) can match the wrong place.
