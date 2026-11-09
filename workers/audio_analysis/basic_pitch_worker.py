"""Decode local audio to note and pitch-contour candidates with the pinned ONNX model."""
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import sys

MODEL_HASH = "2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec"
os.environ.setdefault("OMP_NUM_THREADS", "4")


def run(request):
    import basic_pitch
    import numpy as np
    import onnxruntime
    from basic_pitch.constants import AUDIO_SAMPLE_RATE, FFT_HOP
    from basic_pitch.inference import Model, run_inference
    from basic_pitch.note_creation import get_pitch_bends, model_frames_to_time, output_to_notes_polyphonic

    if importlib.metadata.version("basic-pitch") != "0.4.0" or onnxruntime.__version__ != "1.23.2":
        raise RuntimeError("Install Basic Pitch 0.4.0 and ONNX Runtime 1.23.2 from the worker setup guide.")
    path = Path(basic_pitch.__file__).parent / "saved_models" / "icassp_2022" / "nmp.onnx"
    if hashlib.sha256(path.read_bytes()).hexdigest() != MODEL_HASH:
        raise RuntimeError("The installed Basic Pitch ONNX model failed verification.")
    outputs = run_inference(Path(request["audio"]), Model(path))
    settings = request["detection"]
    notes = output_to_notes_polyphonic(
        outputs["note"], outputs["onset"], onset_thresh=settings["onset"],
        frame_thresh=settings["frame"], infer_onsets=True, min_freq=None, max_freq=None,
        min_note_len=max(1, int(np.round(settings["min_ms"] * AUDIO_SAMPLE_RATE / (1_000 * FFT_HOP)))),
        melodia_trick=settings.get("recover_sustains", False),
    )
    contour = outputs["contour"]
    times = model_frames_to_time(contour.shape[0])
    offset = request["offset"]
    if len(notes) > 8_192:
        raise RuntimeError("Too many note candidates. Use a shorter or more isolated selection.")
    return [
        {"start": float(times[start]) + offset, "end": float(times[end]) + offset,
         "pitch": int(pitch), "strength": float(strength),
         "bends": [round(int(bend) * 100 / 3) for bend in (bends or [])]}
        for start, end, pitch, strength, bends in get_pitch_bends(contour, notes)
    ]


try:
    request = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    reply = {"notes": run(request)}
except Exception as error:
    reply = {"error": str(error)[:600]}
Path(sys.argv[2]).write_text(json.dumps(reply, allow_nan=False, separators=(",", ":")), encoding="utf-8")
