"""Robot library that caps the threads yarf's OCR uses; import it in a suite's Settings.

yarf builds RapidOCR with its defaults, which run onnxruntime on every core: one pass over a
1280x1024 frame then costs ~32 s of CPU in 1.8 s on a 22-core machine, and a Match Text poll
keeps the machine busy. With 4 threads the same pass takes 0.8 s and ~7 s of CPU (measured).
RapidOCRReader is a singleton that keeps an existing reader, so replacing its reader at import
time applies to every OCR call of the suite.
"""

from rapidocr import RapidOCR
from yarf.rf_libraries.libraries.ocr.rapidocr import RapidOCRReader

OCR_THREADS = 1

RapidOCRReader().reader = RapidOCR(
    params={
        "EngineConfig.onnxruntime.intra_op_num_threads": OCR_THREADS,
        "EngineConfig.onnxruntime.inter_op_num_threads": 1,
    }
)
