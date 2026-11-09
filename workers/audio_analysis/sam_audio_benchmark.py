"""Prepare fixed extraction cases and score aligned SAM Audio probe outputs."""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path
from urllib.request import Request, urlopen

import numpy as np
import soundfile as sf


RATE = 48_000
SECONDS = 6
FRAMES = RATE * SECONDS
COMMONS = "https://upload.wikimedia.org/wikipedia/commons/"
CHORALES = "https://www.audiolabs-erlangen.de/resources/MIR/2025-ChoraleBricks"
SOURCES = {
    "bass.ogg": (
        COMMONS + "d/d7/Bass_chorus_sound_1_%28new_wave_post-punk%29.ogg",
        "a7385bcfb1b53b898f93a367ca8c3ed8d716473f1c8f6a379cfca20755de7f51",
        "Bass chorus sound 1 (new wave post-punk), Sard112, CC BY-SA 4.0",
        "https://commons.wikimedia.org/wiki/File:Bass_chorus_sound_1_(new_wave_post-punk).ogg",
        2.0,
    ),
    "flute.ogg": (
        COMMONS + "e/e0/Flute_2.ogg",
        "a5977f27f733c5d815a05476c173b63769f953a39fd43ef90e742a7da373db16",
        "Flute 2, hokuspokus, public domain",
        "https://commons.wikimedia.org/wiki/File:Flute_2.ogg",
        2.0,
    ),
    "dwschorale.ogg": (
        COMMONS + "b/b1/Henry_VIII_pastime_with_good_company_performed_by_the_dwsChorale.ogg",
        "ded4907c12aae2c5ed4a5ec833cb73c1e7d240b495d365a680e9a7d500f87dd7",
        "Pastime with Good Company, David Warin Solomons (dwsChorale), CC BY-SA 3.0",
        "https://commons.wikimedia.org/wiki/File:Henry_VIII_pastime_with_good_company_performed_by_the_dwsChorale.ogg",
        7.0,
    ),
    "chorale-flute.m4a": (
        CHORALES + "/mixes/Bach_IchStehAnDeinerKrippe_fl_cl_bar_tba.m4a",
        "c21fb44d89721e60f0c8b5bff39f1141d832baaadba4f09dbfd4d7072c048178",
        "Ich steh' an Deiner Krippe hier, ChoraleBricks (Balke, Berndt, Müller), CC BY 4.0",
        CHORALES + "/Bach_IchStehAnDeinerKrippe",
        8.0,
    ),
    "chorale-trumpet.m4a": (
        CHORALES + "/mixes/Anonymous_AusMeinesHerzensGrunde_tp_fh_bar_tba.m4a",
        "670ec709430561c2dc5b33115d067efeecaee3d5732829c4ff955236eca0f391",
        "Aus meines Herzens Grunde, ChoraleBricks (Balke, Berndt, Müller), CC BY 4.0",
        CHORALES + "/Anonymous_AusMeinesHerzensGrunde",
        8.0,
    ),
}


def excerpt(cache, name, ffmpeg):
    url, digest, _, _, start = SOURCES[name]
    path = cache / name
    if not path.exists():
        request = Request(url, headers={"User-Agent": "LostSamplesDesktop/0.1 (benchmark)"})
        with urlopen(request, timeout=45) as response:
            content = response.read(16 * 1024**2)
        if hashlib.sha256(content).hexdigest() != digest:
            raise ValueError(f"Unexpected source checksum for {name}")
        path.write_bytes(content)
    if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
        raise ValueError(f"Unexpected cached source checksum for {name}")
    decoded = subprocess.check_output([
        ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error", "-ss", str(start),
        "-i", str(path), "-t", str(SECONDS), "-ac", "1", "-ar", str(RATE),
        "-f", "f32le", "pipe:1",
    ])
    samples = np.frombuffer(decoded, dtype="<f4").astype(np.float64)
    if samples.shape != (FRAMES,) or not np.isfinite(samples).all():
        raise ValueError(f"Invalid six-second excerpt from {name}")
    return samples


def percussion():
    time = np.arange(RATE // 5) / RATE
    phase = 2 * np.pi * (50 * time + 4 * (1 - np.exp(-24 * time)))
    kick = np.sin(phase) * np.exp(-22 * time)
    rng = np.random.default_rng(2319)
    snare = rng.normal(0, 0.6, size=len(time)) * np.exp(-35 * time)
    beat = np.zeros(FRAMES)
    for index, start in enumerate(range(0, FRAMES, RATE // 2)):
        beat[start:start + len(time)] = (kick, snare)[index % 2]
    return beat


def rms(samples):
    return float(np.sqrt(np.mean(np.square(samples))))


def level(samples, db):
    amplitude = rms(samples)
    if amplitude < 1e-6:
        raise ValueError("Reference is silent")
    return samples * (10 ** (db / 20) / amplitude)


def prepare(corpus, cache, ffmpeg):
    cache.mkdir(parents=True, exist_ok=True)
    audio = {name: excerpt(cache, name, ffmpeg) for name in SOURCES}
    bass = level(audio["bass.ogg"], -19)
    flute = level(audio["flute.ogg"], -18)
    choir = level(audio["dwschorale.ogg"], -24)
    drums = level(percussion(), -19)
    cases = [
        ("bass", "bass guitar", bass, drums + 0.5 * flute, ["bass.ogg", "flute.ogg"]),
        ("flute", "flute", flute, drums + bass, ["flute.ogg", "bass.ogg"]),
        ("vocals", "backing vocals", choir, flute + 0.5 * (bass + drums),
         ["dwschorale.ogg", "flute.ogg", "bass.ogg"]),
        ("drums", "kick and snare drums", drums, bass + flute, ["bass.ogg", "flute.ogg"]),
        ("wind-quartet", "flute", audio["chorale-flute.m4a"], None, ["chorale-flute.m4a"]),
        ("brass-quartet", "trumpet", audio["chorale-trumpet.m4a"], None, ["chorale-trumpet.m4a"]),
    ]
    corpus.mkdir(parents=True, exist_ok=False)
    manifest = []
    for name, prompt, target, residual, sources in cases:
        folder = corpus / name
        folder.mkdir()
        mix = target if residual is None else target + residual
        gain = min(1.0, 0.9 / np.max(np.abs(mix)))
        sf.write(folder / "mix.wav", mix * gain, RATE, subtype="FLOAT")
        row = {"id": name, "prompt": prompt, "references": residual is not None, "sources": sources}
        if residual is not None:
            sf.write(folder / "reference-target.wav", target * gain, RATE, subtype="FLOAT")
            sf.write(folder / "reference-residual.wav", residual * gain, RATE, subtype="FLOAT")
            row["target_to_residual_db"] = round(20 * np.log10(rms(target) / rms(residual)), 2)
        manifest.append(row)
    sources = {
        name: dict(download=url, sha256=digest, credit=credit, page=page, start_seconds=start)
        for name, (url, digest, credit, page, start) in SOURCES.items()
    }
    (corpus / "manifest.json").write_text(
        json.dumps({
            "cases": manifest,
            "sources": sources,
            "preprocessing": "Six-second excerpts, mono resampling to 48 kHz, "
                             "level adjustment and summing for controlled mixes.",
        }, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"Prepared {len(manifest)} cases in {corpus}")


def read_wave(path):
    samples, rate = sf.read(path, dtype="float64")
    if rate != RATE or samples.shape != (FRAMES,) or not np.isfinite(samples).all():
        raise ValueError(f"Expected {FRAMES} finite mono frames at {RATE} Hz in {path}")
    return samples


def db_ratio(numerator, denominator):
    return round(20 * float(np.log10(max(numerator, 1e-12) / max(denominator, 1e-12))), 2)


def measure(estimate, wanted, unwanted):
    references = np.column_stack([wanted, unwanted])
    gains, _, rank, _ = np.linalg.lstsq(references, estimate, rcond=None)
    if rank != 2:
        raise ValueError("References must be linearly independent")
    return {
        "wanted_gain_db": db_ratio(abs(gains[0]), 1.0),
        "unwanted_gain_db": db_ratio(abs(gains[1]), 1.0),
        "error_to_wanted_db": db_ratio(rms(estimate - references @ gains), rms(wanted)),
    }


def score(corpus, results):
    manifest = json.loads((corpus / "manifest.json").read_text(encoding="utf-8"))
    report = []
    for case in manifest["cases"]:
        name = case["id"]
        mix = read_wave(corpus / name / "mix.wav")
        original = read_wave(results / name / "original.wav")
        if not np.array_equal(original, mix):
            raise ValueError(f"Probe original differs from the benchmark input for {name}")
        target = read_wave(results / name / "target.wav")
        residual = read_wave(results / name / "residual.wav")
        row = {
            "id": name,
            "prompt": case["prompt"],
            "reconstruction_error_db": db_ratio(rms(target + residual - mix), rms(mix)),
        }
        if case["references"]:
            wanted = read_wave(corpus / name / "reference-target.wav")
            unwanted = read_wave(corpus / name / "reference-residual.wav")
            row["target"] = measure(target, wanted, unwanted)
            row["residual"] = measure(residual, unwanted, wanted)
        report.append(row)
    print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare")
    prep.add_argument("corpus", type=Path)
    prep.add_argument("--cache", type=Path, required=True)
    prep.add_argument("--ffmpeg", default="ffmpeg")
    scoring = commands.add_parser("score")
    scoring.add_argument("corpus", type=Path)
    scoring.add_argument("results", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.corpus, args.cache, args.ffmpeg)
    else:
        score(args.corpus, args.results)


if __name__ == "__main__":
    main()
