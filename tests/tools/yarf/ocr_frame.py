"""Print what yarf's OCR reads in saved frames, with the settings the suites use.

The frames come from tests/tools/yarf/yarf_frames.py (YARF_FRAME_DIR); this reads them back
with the same RapidOCR engine and thread cap, so the text printed here is what
`Match Text`/`Get Text Position` saw in that frame.

usage: python3 ocr_frame.py [-g SUBSTRING] PATH [PATH ...]

PATH is a frame file or a directory of frames (read in name order, matching capture order).
With -g only the frames whose text contains SUBSTRING (case insensitive) are printed, which
answers "did the trainer ever appear?" without reading hundreds of frames.

Runs inside the yarf snap, which is where RapidOCR lives:
    tests/tools/yarf/yarf.sh ocr ~/zwift-pairing-out/frames -g KICKR
"""

import sys
from pathlib import Path

import numpy as np
from PIL import Image
from rapidocr import RapidOCR

# the same cap the suites use (yarf_ocr.py), so the text read here matches theirs
OCR_THREADS = 1


def frames(paths):
    for path in paths:
        candidate = Path(path)
        if candidate.is_dir():
            yield from sorted(candidate.glob("*.png"))
        else:
            yield candidate


def main(argv):
    needle = None
    if "-g" in argv:
        index = argv.index("-g")
        if index + 1 >= len(argv):
            print(__doc__.strip(), file=sys.stderr)
            return 2
        needle = argv[index + 1].lower()
        argv = argv[:index] + argv[index + 2 :]
    if not argv:
        print(__doc__.strip(), file=sys.stderr)
        return 2

    engine = RapidOCR(
        params={
            "EngineConfig.onnxruntime.intra_op_num_threads": OCR_THREADS,
            "EngineConfig.onnxruntime.inter_op_num_threads": 1,
            # every model it loads is announced at INFO, which buries the text read
            "Global.log_level": "WARNING",
        }
    )

    seen = 0
    for path in frames(argv):
        if not path.is_file():
            print(f"no such frame: {path}", file=sys.stderr)
            return 1
        out = engine(np.array(Image.open(path).convert("RGB")))
        texts = list(out.txts or [])
        seen += 1
        if needle is not None and not any(needle in text.lower() for text in texts):
            continue
        print(f"==> {path}")
        for text, score in zip(texts, out.scores or []):
            print(f"    {score:.2f}  {text!r}")

    print(f"\n{seen} frame(s) OCR'd", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
