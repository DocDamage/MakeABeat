"""Run a local CUDA smoke test for the Phase 8 text-prompt provider."""

import argparse
import tempfile
import time
from pathlib import Path


MODEL = "facebook/sam-audio-small"
REVISION = "20b65f56888142eebe7c37448c6f6b3b32600e9b"
MAX_PROBE_SECONDS = 6


def extract(input_path, prompt, output):
    import soundfile as sf
    import torch
    import torchaudio
    from huggingface_hub import snapshot_download
    from huggingface_hub.errors import HfHubHTTPError
    from sam_audio import SAMAudio, SAMAudioProcessor

    if not torch.cuda.is_available():
        raise RuntimeError("Install a CUDA-enabled PyTorch build for this probe.")

    with sf.SoundFile(input_path) as source:
        if not 0 < source.frames <= source.samplerate * MAX_PROBE_SECONDS:
            raise ValueError(f"Choose a clip of up to {MAX_PROBE_SECONDS} seconds.")
        if source.channels not in (1, 2):
            raise ValueError("Choose a mono or stereo clip.")
        input_frames = source.frames
        audio = torch.from_numpy(source.read(dtype="float32", always_2d=True).T)
        source_rate = source.samplerate
    if not torch.isfinite(audio).all():
        raise ValueError("The source contains non-finite samples.")

    print(f"Resolving {MODEL} at {REVISION}", flush=True)
    try:
        snapshot = snapshot_download(
            MODEL, revision=REVISION, allow_patterns=["config.json", "checkpoint.pt"]
        )
    except HfHubHTTPError as error:
        if error.response.status_code == 401:
            raise RuntimeError(
                "Hugging Face rejected the local credential. Run hf auth login --force."
            ) from None
        if error.response.status_code == 403:
            raise RuntimeError(
                f"Hugging Face denied checkpoint access. Confirm model approval at "
                f"https://huggingface.co/{MODEL} and use a token with read access."
            ) from None
        raise

    processor = SAMAudioProcessor.from_pretrained(snapshot)
    sample_rate = processor.audio_sampling_rate
    audio = torchaudio.functional.resample(audio, source_rate, sample_rate)
    frames = (input_frames * sample_rate + source_rate - 1) // source_rate
    if audio.shape[-1] == frames - 1:
        # TorchAudio can round down by one when its floating-point length rounds to an integer.
        audio = torch.cat((audio, audio[..., -1:]), dim=-1)
    if audio.shape[-1] != frames:
        raise ValueError("Unexpected resampled selection length.")
    torch.manual_seed(0)
    torch.cuda.reset_peak_memory_stats()
    started = time.perf_counter()
    # The text-only pass uses a single candidate and never predicts anchors.
    model = SAMAudio.from_pretrained(
        snapshot, text_ranker=None, visual_ranker=None, span_predictor=None
    ).eval().to("cuda")
    torch.cuda.synchronize()
    load_seconds = time.perf_counter() - started

    batch = processor(audios=[audio], descriptions=[prompt]).to("cuda")
    started = time.perf_counter()
    with torch.inference_mode():
        result = model.separate(batch, predict_spans=False, reranking_candidates=1)
    torch.cuda.synchronize()
    inference_seconds = time.perf_counter() - started

    waves = {"original": audio.mean(dim=0)}
    for name, samples in (("target", result.target[0]), ("residual", result.residual[0])):
        if samples.ndim != 1 or not (
            frames <= samples.numel() < frames + processor.audio_hop_length
        ):
            raise ValueError(f"Unexpected {name} length or shape: {tuple(samples.shape)}.")
        if not torch.isfinite(samples).all():
            raise ValueError(f"The {name} contains non-finite samples.")
        waves[name] = samples[:frames].float().cpu()

    with tempfile.TemporaryDirectory(prefix="sam-audio-", dir=output.parent) as staging:
        rendered = Path(staging) / "rendered"
        rendered.mkdir()
        for name, samples in waves.items():
            sf.write(rendered / f"{name}.wav", samples.numpy(), sample_rate, subtype="FLOAT")
        rendered.rename(output)

    peak_mib = torch.cuda.max_memory_allocated() / 1024**2
    print(f"{torch.cuda.get_device_name()} | PyTorch {torch.__version__}")
    print(
        f"{frames} frames at {sample_rate} Hz | load {load_seconds:.2f} s "
        f"| inference {inference_seconds:.2f} s | peak PyTorch CUDA {peak_mib:.0f} MiB"
    )
    print(f"Wrote original, target, and residual to {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="a mono or stereo clip up to six seconds")
    parser.add_argument("prompt", help="a sound description, for example 'piano melody'")
    parser.add_argument("output", type=Path, help="new output directory under an existing parent")
    args = parser.parse_args()
    if not args.input.is_file():
        parser.error("input must be an existing audio file")
    if not args.prompt.strip():
        parser.error("prompt must describe a sound")
    if args.output.exists() or not args.output.parent.is_dir():
        parser.error("output must be a new directory under an existing parent")
    extract(args.input, args.prompt.strip(), args.output)


if __name__ == "__main__":
    main()
