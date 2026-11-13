# Preface

The visual smoke test: what it drives, how to read its output, the frame-dump replay
that makes a failed OCR match diagnosable, and the yarf and Mir gotchas that cost
time to discover. Read this before changing `tests/smoke.sh` or the robot suite.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What the smoke test does

`tests/smoke.sh` starts a `mir-test-tools` Mir compositor, runs the real launcher
against it and drives the Robot Framework suite in `tests/smoke/` with
[yarf](https://documentation.ubuntu.com/yarf): test 1 looks for the launcher's
`Settings` button, test 2 requires the Zwift SIGN UP screen through OCR. The first run on a fresh wine prefix installs wine prerequisites
and downloads Zwift (tens of minutes), later runs are quick. No login is performed.
Extra arguments to the `smoke` action are passed to yarf (for example,
`--variant light`). The harness installs `yarf` and `mir-test-tools` and connects
`zwift:gpu-2604` to `mesa-2604` when needed.

## Reading the output

On failure check `~/zwift-smoke-out/`: `yarf.log`, `log.html` (screenshots yarf
compared), `zwift.out`, the per-mode `mir-<mode>.log`, and the tail of
`~/snap/zwift/common/zwift.log` that the harness prints.

Two logfiles show live progress while the suite runs:

- `~/snap/zwift/common/zwift.log` — the launcher log: the Wine prerequisite steps,
  the Zwift download, the launcher's own state.
- `~/snap/zwift/common/.wine/drive_c/users/<user>/AppData/Local/Zwift/Logs/Log.txt`
  — Zwift's own log, which only exists once the game has started.

Window decorations and themes differ between machines: when the suite reports a
missing template, recapture it with the recipe in `tests/smoke/README.md` and commit
it. `SMOKE_DISPLAY=nested|headless` overrides the auto detection; nested renders on
the host display through the workshop desktop interface and, with interfaces missing,
fails instead of rendering elsewhere (see the Mir gotcha below).

## Yarf gotchas (all verified live)

- The yarf snap is strict: it sees only `$HOME` — not `/project`, not `/tmp`, and not
  `~/.cache`. The harness stages the suite to `~/zwift-smoke-suite` for that reason.
- yarf exits 120 with EMPTY output when its stdout is a regular file; piped output is
  fine. The harness pipes it through `tee` (with `pipefail`). This is not a yarf quirk:
  `snap run` (and `snap run --shell`) behaves the same — with stdout redirected to a
  plain file it fails silently with rc=1 and no stderr, while a terminal or pipe works.
  `tests/bluetooth.sh` hits the same wall and pipes its snap-run output through `tee`.
- The suite argument must be a directory containing `*.robot` files, not a file.
- `Match`/`Match Text` timeouts are integer seconds, not Robot time strings.
- OCR (`Match Text`) can misread small text. A separate screenshot/OCR pass may
  succeed while the running yarf client's Wayland capture has failed; inspect both.
- Do not trust the suite's own `print` lines in `yarf.log`: Robot's output buffering loses and
  interleaves them, so keywords that demonstrably ran can be missing from it. Read the keyword
  order in `output.xml` (or `log.html`) to see what actually executed.
- A single missed frame happens even when the dialog is visible: "gone" checks must
  require several consecutive misses.
- `mir-test-tools.demo-server` names its own wayland socket after `WAYLAND_DISPLAY`
  (export `wayland-99` before starting it) and refuses to start when that socket
  already exists — clean stale sockets first. To nest on the host compositor it
  also needs `--wayland-host` set to the host socket name, captured before that
  export, plus GPU device access from the workshop gpu interface — without either
  it hangs with an empty log and the harness falls back to headless.
- Nested Mir can also abort on startup with `Failed to read Wayland events: Broken pipe`
  when a required host/workshop interface is disconnected — the usual cause is a workshop
  desktop or gpu plug that is not connected. On the workshop host `workshop connections`
  lists them; connect the missing one with `workshop connect <workshop>/<sdk>:desktop` (or
  `:gpu`), remembering that the desktop plug never connects on its own, so a `connections:`
  entry does not wire it. `SMOKE_DISPLAY=nested` reports exactly that and fails; only
  `SMOKE_DISPLAY=auto` still falls back to a headless compositor, so check the startup
  output to confirm which mode ran. mir's output is written through `tee` so that message
  survives: snap apps write nothing to a plain file, so a reproduction redirected to a
  regular file shows an empty log.

## Frame dump and OCR replay

A failed OCR match is hard to argue with after the fact, so the suites keep every frame
they capture. `tests/tools/yarf/yarf_frames.py` is imported as a Robot `Library` next to
`yarf_ocr.py` in all five suites and patches yarf's
`VideoInputBase._grab_and_save_screenshot` plus the `match_text`/`find_text`/`read_text`/
`_do_match` paths, so capture, matching and timeouts all dump their frame. It exists as a
patch rather than a yarf plugin because yarf never calls `_start_suite`, so a suite-level
hook would not fire.

- `YARF_FRAME_DIR` selects the directory and is required — without it the library does
  nothing, so a plain `yarf` run is unaffected. `YARF_FRAME_LIMIT` caps how many frames are
  kept (oldest dropped first; `0`, the default, keeps all of them). The harnesses set the
  former: `~/zwift-pairing-out/frames`, `~/zwift-smoke-out/frames` and gtk-preview's staged
  `frames/`.
- Each frame is a PNG named by its 10-digit capture number, and `index.log` maps that
  number to the wait that captured it (`match_text 'KICKR'`) — the index is what makes a
  failure diagnosable after the run. Capture number and index continue from the frames
  already in the directory, so point it at a fresh directory per run; otherwise a
  previous run's frames stay mixed in.
- `tests/tools/yarf/yarf.sh ocr [-g SUBSTRING] PATH [PATH ...]` replays OCR over dumped
  frames (paths, or a directory of them) without a compositor: it prints what each frame
  reads and, with `-g`, keeps only the frames whose text contains the substring. Use it to
  prove whether the text was on screen (a yarf/OCR problem) or absent (a game/test problem)
  when a match timed out — the usual case is that the text was there and `Match Text`
  misread it.
- `tests/tools/yarf/ocr_frame.py` is the OCR entry point that `yarf.sh ocr` runs inside the
  yarf snap; it is frame-file based, so it needs no Wayland connection.

## OCR and capture diagnostics

OCR runs sequentially and does not queue frames. If the test keeps waiting while
the expected screen is visible, compare a fresh `workshop run yarf screenshot`
capture with yarf's diagnostics; a separate capture succeeding does not establish
that the running yarf client's Wayland connection is healthy.
