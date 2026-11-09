"""Lost Samples analysis protocol v1. The host supplies a verified local checkpoint."""

import importlib.metadata
import json
import math
import os
import sys
import threading

CHECKPOINT = sys.argv[1]
VERSION = "1.1.0-small0-v1"


def read_message(limit):
    # Windows' redirected stdin can wait for a buffered read to fill while the
    # host keeps the pipe open for cancellation. The protocol messages are small.
    line = bytearray()
    while len(line) < limit:
        byte = os.read(sys.stdin.fileno(), 1)
        if not byte:
            return None
        if byte == b"\n":
            return json.loads(line)
        line.extend(byte)
    raise ValueError("Analysis request is too large")


if len(sys.argv) > 2:
    with open(sys.argv[2], "r", encoding="utf-8") as request_file:
        request = json.load(request_file)
else:
    request = read_message(1_048_576)


def emit(kind, **payload):
    print(json.dumps({"version": 1, "job_id": request["job_id"], "type": kind, **payload},
                     allow_nan=False), flush=True)


def result(beats):
    emit("result", beats=beats, **{
        field: request[field] for field in (
            "project_revision", "source_fingerprint", "start_frame", "end_frame",
            "model_id", "model_version")
    })


def run():
    if (request["version"] != 1 or request["type"] != "run"
            or request["model_id"] != "beat-this" or request["model_version"] != VERSION):
        emit("failure", code="invalid_request", message="Unsupported Beat This! request.")
        return
    try:
        version = importlib.metadata.version("beat-this")
        if version != "1.1.0":
            raise RuntimeError(f"Expected beat-this 1.1.0, found {version}.")
        import torch
        from beat_this.inference import File2Beats
    except (ImportError, importlib.metadata.PackageNotFoundError, OSError, RuntimeError) as exc:
        emit("failure", code="unavailable_model",
             message=f"Install beat-this==1.1.0 and CPU PyTorch in the selected Python environment. {exc}")
        return
    if request["operation"] == "probe":
        result([])
        return
    if request["operation"] != "beats" or not os.path.isfile(CHECKPOINT):
        emit("failure", code="unavailable_model", message="Download the verified small0 weights first.")
        return
    try:
        torch.set_num_threads(min(4, os.cpu_count() or 1))
        emit("progress", stage="Loading checkpoint", completed=1, total=3)
        # Passing the existing file path keeps the upstream loader offline.
        tracker = File2Beats(checkpoint_path=CHECKPOINT, device="cpu", dbn=False)
        emit("progress", stage="Tracking beats", completed=2, total=3)
        beats, downbeats = tracker(request["input_path"])
        starts = set(map(float, downbeats))
        result([{"seconds": at, "downbeat": at in starts}
                for at in map(float, beats) if math.isfinite(at) and at >= 0.0][:4096])
    except Exception as exc:
        emit("failure", code="inference_failure", message=f"Beat This! could not analyze the audio. {exc}")


def listen_for_cancel():
    try:
        message = read_message(1024)
        if message == {"version": 1, "job_id": request["job_id"], "type": "cancel"}:
            os._exit(2)
    except (ValueError, OSError):
        pass


threading.Thread(target=listen_for_cancel, daemon=True).start()
run()
