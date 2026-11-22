"""Robot library that caps the threads yarf's OCR uses; import it in a suite's Settings.

yarf builds RapidOCR with its defaults, which run onnxruntime on every core: one pass over a
1280x1024 frame then costs ~32 s of CPU in 1.8 s on a 22-core machine, and a Match Text poll
keeps the machine busy. With 4 threads the same pass takes 0.8 s and ~7 s of CPU (measured).
RapidOCRReader is a singleton that keeps an existing reader, so replacing its reader at import
time applies to every OCR call of the suite.

Det.limit_side_len is raised from RapidOCR's default of 736. The detector scales a frame so its
longer side is at most that value, so a 1280x1024 frame is downscaled to 736 wide before text
detection. At that scale the pair-device dialog's device name (e.g. KICKR) sits on a white row
in a size the detector sometimes drops entirely: the dialog's own chrome (PAIR DEVICE, CLOSE,
ALL) is found but the device row is invisible to OCR, so Match Text KICKR times out even with
the dialog open and the trainer listed on screen. This is long-standing flakiness, not a change
in any particular Zwift version. Keeping the longer side at the frame width (1280) leaves the
frame unscaled, which reliably detects the device name. See .kb/pairing-test.md.
"""

from rapidocr import RapidOCR
from yarf.rf_libraries.libraries.ocr.rapidocr import RapidOCRReader

OCR_THREADS = 1
# keep the 1280-wide frame unscaled so small on-screen text is still detected,
# see the module docstring
OCR_DETECT_SIDE_LEN = 1280

RapidOCRReader().reader = RapidOCR(
    params={
        "EngineConfig.onnxruntime.intra_op_num_threads": OCR_THREADS,
        "EngineConfig.onnxruntime.inter_op_num_threads": 1,
        "Det.limit_side_len": OCR_DETECT_SIDE_LEN,
    }
)
