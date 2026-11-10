"""Robot library that keeps every frame yarf captures, for debugging a failed suite.

yarf already has the plumbing: `_grab_and_save_screenshot` is the single funnel through
which every OCR pass and template match gets its frame, and it saves each one when
`VideoInputBase._screenshots_dir` is set. Nothing ever sets it, though: the field is only
filled by `_start_suite`/`_end_suite`, and those are named with a leading underscore, so
Robot's library-listener API (which calls `start_suite`/`end_suite`) never invokes them.
The frames are therefore captured and thrown away, and a failed match only leaves the one
frame Robot logs at the end of the wait.

Import this library from a suite and point `YARF_FRAME_DIR` at a directory under the real
home (yarf is a strict snap: it cannot write to /project or /tmp) to keep all of them:

    YARF_FRAME_DIR=~/zwift-pairing-out/frames yarf --platform Mir SUITE --outdir OUT

Each frame lands in `<dir>/<10 digits>.png` in capture order, and `<dir>/index.log` maps
that number to the wait that captured it (`match_text 'KICKR'`), so a failure can be
replayed: OCR the frames of the failing wait with `tests/tools/yarf/ocr_frame.py` and see
exactly what the suite saw, frame by frame.

`YARF_FRAME_LIMIT` caps the number of frames saved (0, the default, keeps all of them).
A whole run stays in the tens of megabytes, because every OCR pass takes about a second.
Several yarf processes can share one directory: the numbering continues from the frames
already there and the index is appended to. Point it at a fresh directory per run anyway
(the harnesses wipe their output directory, tests/gtk-preview.sh makes a new one), or the
frames of the previous run stay mixed in.
Without `YARF_FRAME_DIR` the library does nothing, so a plain `yarf` run is unaffected.
"""

import functools
import os
import time
from pathlib import Path

from yarf.rf_libraries.libraries.video_input_base import VideoInputBase

FRAME_DIR_ENV = "YARF_FRAME_DIR"
FRAME_LIMIT_ENV = "YARF_FRAME_LIMIT"

# what the next captured frame belongs to, set by the outermost wrapped wait
_context = "startup"

# >0 while a wrapped wait is running, so a nested one (match_text calls find_text) does
# not replace the context of the wait the user actually wrote
_depth = 0

# a suite directory can import this library from several suites; patch only once
_installed = False


def _frame_writer():
    """Return a function that stores one frame per call, or None when disabled."""
    directory = os.environ.get(FRAME_DIR_ENV)
    if not directory:
        return None

    path = Path(os.path.expanduser(directory))
    try:
        path.mkdir(parents=True, exist_ok=True)
        existing = sorted(path.glob("[0-9]" * 10 + ".png"))
    except OSError:
        return None
    try:
        limit = int(os.environ.get(FRAME_LIMIT_ENV) or 0)
    except ValueError:
        limit = 0

    # continue past the frames of an earlier yarf process in this run, so several
    # suites can share one directory without overwriting each other
    count = int(existing[-1].stem) if existing else 0
    index = (path / "index.log").open("a", encoding="utf-8")

    def write(image):
        nonlocal count
        if limit and count >= limit:
            return
        count += 1
        # compress_level=1 is what yarf uses: a failed wait can capture a frame per
        # second for minutes, so the saving must stay cheap.
        try:
            image.save(path / f"{count:010d}.png", compress_level=1)
        except OSError:
            return
        index.write(f"{count:010d} {time.strftime('%H:%M:%S')} {_context}\n")
        # an interrupted run still leaves the index as far as it got
        index.flush()

    return write


def _wrap(wrapped, describe):
    """Set the frame context to `describe(...)` for the duration of a yarf wait."""

    @functools.wraps(wrapped)
    async def wrapper(self, *args, **kwargs):
        global _context, _depth
        if _depth == 0:
            _context = describe(args, kwargs)
        _depth += 1
        try:
            return await wrapped(self, *args, **kwargs)
        finally:
            _depth -= 1

    return wrapper


def _first(args, kwargs, name):
    """The first positional argument, or the named one, of a yarf keyword call."""
    if args:
        return args[0]
    return kwargs.get(name, "")


def _install():
    global _installed
    if _installed:
        return
    write = _frame_writer()
    if write is None:
        return
    _installed = True

    original_grab = VideoInputBase._grab_and_save_screenshot

    @functools.wraps(original_grab)
    async def grab_and_save(self):
        image = await original_grab(self)
        write(image)
        return image

    VideoInputBase._grab_and_save_screenshot = grab_and_save
    VideoInputBase.match_text = _wrap(
        VideoInputBase.match_text, lambda a, k: f"match_text {_first(a, k, 'text')!r}"
    )
    VideoInputBase.find_text = _wrap(
        VideoInputBase.find_text, lambda a, k: f"find_text {_first(a, k, 'text')!r}"
    )
    VideoInputBase.read_text = _wrap(VideoInputBase.read_text, lambda a, k: "read_text")
    VideoInputBase._do_match = _wrap(
        VideoInputBase._do_match,
        lambda a, k: f"_do_match {_first(a, k, 'templates')!r}",
    )


_install()
