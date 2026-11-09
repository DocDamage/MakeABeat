"""Build a fixed retrieval corpus from publisher-supplied annotations."""
import argparse
from collections import defaultdict
import csv
import io
import json
from pathlib import Path
import tarfile
from zipfile import ZipFile


def put(root, label, name, audio):
    directory = root / label
    directory.mkdir(parents=True, exist_ok=True)
    (directory / name).write_bytes(audio)


def esc(root, archive):
    with ZipFile(archive) as zipped:
        prefix = next(path.removesuffix("meta/esc50.csv") for path in zipped.namelist()
                      if path.endswith("meta/esc50.csv"))
        rows = list(csv.DictReader(io.StringIO(zipped.read(prefix + "meta/esc50.csv").decode())))
        labels = sorted({row["category"] for row in rows})
        for label in labels:
            for fold in ("1", "2", "3", "4"):
                candidates = [row["filename"] for row in rows if row["category"] == label and row["fold"] == fold]
                name = min(candidates)
                put(root, label, name, zipped.read(prefix + "audio/" + name))
        return [{"text": label.replace("_", " "), "label": label, "suite": "ESC-50"} for label in labels]


def nsynth(root, archive):
    with tarfile.open(archive, "r:gz") as tar:
        members = {entry.name: entry for entry in tar.getmembers() if entry.isfile()}
        meta = next(name for name in members if name.endswith("/examples.json"))
        labels = json.load(tar.extractfile(members[meta]))
        choices = defaultdict(list)
        for key, note in sorted(labels.items()):
            if 45 <= note["pitch"] <= 76 and note["velocity"] == 75:
                choices[note["instrument_family_str"]].append((key, note["instrument"]))
        pick = {}
        for family, entries in sorted(choices.items()):
            distinct = {}
            for key, instrument in entries:
                distinct.setdefault(instrument, key)
            selected = list(distinct.values())[:4]
            selected.extend(key for key, _ in entries if key not in selected)
            pick.update({key: "nsynth-" + family for key in selected[:4]})
        lookup = {Path(name).stem: entry for name, entry in members.items() if name.endswith(".wav")}
        for key in sorted(pick, key=lambda key: lookup[key].offset):
            put(root, pick[key], key + ".wav", tar.extractfile(lookup[key]).read())
        words = {"reed": "a reed instrument playing a note", "mallet": "a mallet instrument playing a note",
                 "keyboard": "a keyboard playing one musical note", "bass": "a low bass instrument note",
                 "brass": "a brass instrument playing a note", "string": "a sustained string instrument note",
                 "synth_lead": "a synthesizer lead note", "vocal": "a sung vowel note"}
        return [{"text": words.get(family, f"a {family} playing a single note"),
                 "label": "nsynth-" + family, "suite": "NSynth"} for family in sorted(choices)]


parser = argparse.ArgumentParser()
parser.add_argument("esc_zip", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--nsynth", type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
queries = esc(args.output, args.esc_zip)
if args.nsynth:
    queries.extend(nsynth(args.output, args.nsynth))
(args.output / "queries.json").write_text(json.dumps(queries, indent=2), encoding="utf8")
