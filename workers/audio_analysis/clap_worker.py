"""Pinned CLAP worker. Setup is the only operation that allows downloads."""
import hashlib
import json
import os
import sys

MODEL = "laion/clap-htsat-unfused"
REVISION = "8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a"
WEIGHTS_SHA256 = "1cd3c601bc4afe0fa87be3de4c13dd2cfadd249fac1e29acf74a9b296c3219bb"
FILES = (
    "config.json", "merges.txt", "preprocessor_config.json", "pytorch_model.bin",
    "special_tokens_map.json", "tokenizer.json", "tokenizer_config.json", "vocab.json",
)
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
reply = sys.stdout
sys.stdout = sys.stderr


def send(**fields):
    print(json.dumps(fields, allow_nan=False, separators=(",", ":")), file=reply, flush=True)


def serve():
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["TOKENIZERS_PARALLELISM"] = "false"
    import math
    import numpy as np
    import scipy
    from scipy.signal import resample_poly
    import torch
    import transformers
    from huggingface_hub import snapshot_download
    from transformers import ClapModel, ClapProcessor

    if (transformers.__version__, np.__version__, scipy.__version__) != ("5.14.1", "2.2.6", "1.15.3") \
            or torch.__version__.split("+")[0] != "2.13.0":
        raise RuntimeError("Use the pinned CLAP Python environment from the worker README.")
    torch.set_num_threads(4)
    torch.set_num_interop_threads(1)
    try:
        folder = snapshot_download(MODEL, revision=REVISION, local_files_only=True)
        processor = ClapProcessor.from_pretrained(folder, local_files_only=True)
        model = ClapModel.from_pretrained(folder, local_files_only=True, use_safetensors=False).eval()
    except OSError as error:
        raise RuntimeError("Pinned CLAP files are unavailable. Run Download CLAP model once in Search.") from error
    send(ready=True, revision=REVISION)
    for line in sys.stdin:
        try:
            request = json.loads(line)
            with torch.inference_mode():
                if "text" in request:
                    inputs = processor(text=[request["text"]], return_tensors="pt",
                                       truncation=True, max_length=77)
                    output = model.get_text_features(**inputs).pooler_output
                else:
                    rate = request["rate"]
                    samples = np.fromfile(request["path"], dtype="<f4")
                    if sum(request["lengths"]) != samples.size:
                        raise ValueError("Invalid input frame count")
                    audio = np.split(samples, np.cumsum(request["lengths"])[:-1])
                    if rate != 48_000:
                        gcd = math.gcd(rate, 48_000)
                        audio = [resample_poly(segment, 48_000 // gcd, rate // gcd) for segment in audio]
                    inputs = processor(audio=audio, sampling_rate=48_000, return_tensors="pt",
                                       padding="repeatpad", truncation="rand_trunc")
                    output = model.get_audio_features(**inputs).pooler_output
                vectors = torch.nn.functional.normalize(output, dim=-1).tolist()
            send(vectors=vectors)
        except Exception as error:
            send(error=str(error)[:500])


try:
    if len(sys.argv) == 2 and sys.argv[1] == "--setup":
        from huggingface_hub import snapshot_download
        from pathlib import Path
        folder = Path(snapshot_download(MODEL, revision=REVISION, allow_patterns=FILES, token=False))
        digest = hashlib.sha256()
        with (folder / "pytorch_model.bin").open("rb") as weights:
            for block in iter(lambda: weights.read(1024 * 1024), b""):
                digest.update(block)
        if digest.hexdigest() != WEIGHTS_SHA256:
            raise RuntimeError("The downloaded CLAP weights failed verification.")
        send(ready=True, revision=REVISION)
    else:
        serve()
except Exception as error:
    send(error=str(error)[:500])
