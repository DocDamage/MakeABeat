"""Prepare deterministic synthetic bass and annotated GuitarSet excerpts."""
import argparse
import json
from pathlib import Path
from zipfile import ZipFile

import numpy as np
from scipy.io import wavfile


def synthetic(folder):
    rate = 48_000
    bass = np.zeros(6 * rate, dtype=np.float32)
    mix = np.zeros_like(bass)
    noise = np.random.default_rng(41)
    reference = []
    for i, pitch in enumerate([33, 33, 40, 36, 38, 38, 36, 40, 33, 33, 40, 36]):
        frequency = 440 * 2 ** ((pitch - 69) / 12)
        start = i * rate // 2
        t = np.arange(rate * 2 // 5) / rate
        envelope = np.minimum(t * 200, 1) * np.exp(-7 * t)
        bass[start:start + len(t)] += 0.36 * (
            np.sin(2 * np.pi * frequency * t) + 0.35 * np.sin(4 * np.pi * frequency * t)
        ) * envelope
        drums = np.arange(rate // 6) / rate
        hit = (np.sin(2 * np.pi * (55 * drums + 25 * (1 - np.exp(-15 * drums)) / 15))
               if i % 2 == 0 else noise.normal(0, 0.65, len(drums)))
        mix[start:start + len(drums)] += 0.55 * hit * np.exp(-35 * drums)
        # Treat the backing chord and drums as distractors for the bass-only task.
        for interval in (24, 27, 31):
            chord = np.sin(2 * np.pi * frequency * 2 ** (interval / 12) * t)
            mix[start:start + len(t)] += 0.1 * chord * envelope
        reference.append({"start": i / 2, "end": i / 2 + 0.4, "pitch": pitch})
    for name, samples in [("Isolated bass", bass), ("Bass in dense mix", (bass + mix).clip(-1, 1))]:
        filename = name.lower().replace(" ", "-") + ".wav"
        wavfile.write(folder / filename, rate, samples)
        yield {"group": name, "file": filename, "notes": reference}


def guitarset(folder, annotations, audio):
    with ZipFile(annotations) as labels, ZipFile(audio) as recordings:
        available = {Path(name).stem: name for name in labels.namelist() if name.endswith(".jams")}
        wavs = {Path(name).stem.removesuffix("_mic"): name for name in recordings.namelist()
                if name.endswith(".wav")}
        for player in range(6):
            for role in ("solo", "comp"):
                names = sorted(name for name in available if name in wavs and name.startswith(f"{player:02}_")
                               and name.endswith("_" + role))
                if not names:
                    raise ValueError(f"Missing GuitarSet clip for {player:02} {role}")
                name = names[0]
                filename = name + ".wav"
                (folder / filename).write_bytes(recordings.read(wavs[name]))
                jams = json.loads(labels.read(available[name]))
                notes = [{"start": row["time"], "end": row["time"] + row["duration"],
                          "pitch": round(row["value"])}
                         for track in jams["annotations"] if track["namespace"] == "note_midi"
                         for row in track["data"] if row["duration"] > 0]
                yield {"group": "GuitarSet " + role, "file": filename, "notes": notes}


parser = argparse.ArgumentParser()
parser.add_argument("output", type=Path)
parser.add_argument("--annotations", type=Path)
parser.add_argument("--audio", type=Path)
args = parser.parse_args()
if bool(args.annotations) != bool(args.audio):
    parser.error("Supply both GuitarSet archives.")
args.output.mkdir(parents=True, exist_ok=True)
cases = list(synthetic(args.output))
if args.annotations:
    cases.extend(guitarset(args.output, args.annotations, args.audio))
(args.output / "labels.json").write_text(json.dumps(cases, indent=2), encoding="utf-8")
