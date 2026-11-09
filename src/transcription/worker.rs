use super::{Candidate, Detection, MAX_NOTES, MAX_SECONDS};
use crate::{
    audio::Clip,
    background,
    files::{self, Result},
    model::melody::{BEND_RANGE, MAX_POINTS},
    subprocess::{self, Capture},
};
use serde::Deserialize;
use serde_json::json;
use std::{fs, path::Path, process::Command, time::Duration};

const SCRIPT: &str = include_str!("../../workers/audio_analysis/basic_pitch_worker.py");

#[derive(Deserialize)]
struct Reply {
    notes: Option<Vec<Candidate>>,
    error: Option<String>,
}

pub fn analyze(
    python: &Path,
    clip: &Clip,
    range: (f64, f64),
    detection: Detection,
) -> Result<Vec<Candidate>> {
    if range.1 - range.0 > MAX_SECONDS {
        return Err("Select at most three minutes of audio.".into());
    }
    let temporary = tempfile::tempdir()?;
    let audio = temporary.path().join("input.wav");
    let request = temporary.path().join("request.json");
    let output = temporary.path().join("result.json");
    files::export_region(&audio, clip, range)?;
    fs::write(
        &request,
        serde_json::to_vec(&json!({
            "audio": audio, "offset": range.0, "detection": detection,
        }))?,
    )?;
    background::stage("Detecting notes with Basic Pitch")?;
    let mut command = Command::new(python);
    command.args(["-u", "-c", SCRIPT]).arg(request).arg(&output);
    let result = subprocess::run(
        &mut command,
        "Basic Pitch",
        Duration::from_secs(240),
        Capture::default(),
    )?;
    if !result.status.success() {
        return Err("Basic Pitch stopped before returning notes.".into());
    }
    if fs::metadata(&output)?.len() > 8 * 1024 * 1024 {
        return Err("The transcription result is too large. Choose a shorter selection.".into());
    }
    let reply: Reply = serde_json::from_slice(&fs::read(output)?)?;
    if let Some(error) = reply.error {
        return Err(error.into());
    }
    let notes = reply.notes.ok_or("Basic Pitch did not return notes.")?;
    if notes.len() > MAX_NOTES
        || notes.iter().any(|note| {
            !note.start.is_finite()
                || !note.end.is_finite()
                || note.end <= note.start
                || note.start < range.0 - 1.0
                || note.end > range.1 + 1.0
                || !note.strength.is_finite()
                || note.bends.len() > MAX_POINTS
                || note
                    .bends
                    .iter()
                    .any(|&cents| !(-BEND_RANGE..=BEND_RANGE).contains(&cents))
        })
    {
        return Err("Basic Pitch returned invalid note data.".into());
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, path::PathBuf, time::Instant};

    #[derive(Deserialize)]
    struct Label {
        start: f64,
        end: f64,
        pitch: u8,
    }

    #[derive(Deserialize)]
    struct Case {
        group: String,
        file: PathBuf,
        notes: Vec<Label>,
    }

    #[derive(Default)]
    struct Score {
        expected: usize,
        returned: usize,
        matched: usize,
        matched_offset: usize,
        onset_error: f64,
        duration_error: f64,
    }

    #[test]
    #[ignore = "requires the pinned Basic Pitch environment and the labeled evaluation corpus"]
    fn basic_pitch_evaluates_labeled_corpus() -> Result<()> {
        let root = PathBuf::from(
            std::env::var_os("LOST_SAMPLES_PITCH_CORPUS")
                .ok_or("Set LOST_SAMPLES_PITCH_CORPUS.")?,
        );
        let python = PathBuf::from(
            std::env::var_os("LOST_SAMPLES_PITCH_PYTHON").unwrap_or_else(|| "python".into()),
        );
        let cases: Vec<Case> = serde_json::from_slice(&fs::read(root.join("labels.json"))?)?;
        let mut groups: BTreeMap<String, Score> = BTreeMap::new();
        for case in cases {
            let clip = files::decode_path(&root.join(&case.file))?;
            let started = Instant::now();
            let candidates = analyze(&python, &clip, (0.0, clip.duration()), Detection::default())?;
            let score = groups.entry(case.group).or_default();
            score.expected += case.notes.len();
            score.returned += candidates.len();
            let mut pairs = Vec::new();
            for (expected, note) in case.notes.iter().enumerate() {
                for (returned, candidate) in candidates.iter().enumerate() {
                    let error = (note.start - candidate.start).abs();
                    if error <= 0.05 && note.pitch == candidate.pitch {
                        pairs.push((error, expected, returned));
                    }
                }
            }
            pairs.sort_unstable_by(|a, b| {
                a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
            });
            let mut expected = vec![false; case.notes.len()];
            let mut returned = vec![false; candidates.len()];
            let mut matches = 0;
            for (error, a, b) in pairs {
                if expected[a] || returned[b] {
                    continue;
                }
                expected[a] = true;
                returned[b] = true;
                let note = &case.notes[a];
                let candidate = &candidates[b];
                let duration = note.end - note.start;
                matches += 1;
                score.matched += 1;
                score.matched_offset +=
                    usize::from((candidate.end - note.end).abs() <= 0.05_f64.max(0.2 * duration));
                score.onset_error += error;
                score.duration_error += ((candidate.end - candidate.start) - duration).abs();
            }
            println!(
                "{}: {matches}/{} onset+pitch matches, {} candidates in {:.2}s",
                case.file.display(),
                case.notes.len(),
                candidates.len(),
                started.elapsed().as_secs_f64()
            );
        }
        for (group, score) in &groups {
            let n = score.matched.max(1) as f64;
            println!(
                "{group}: TP={}, expected={}, candidates={}, offset matches={}, F1={:.3}, onset MAE={:.1} ms, duration MAE={:.1} ms",
                score.matched,
                score.expected,
                score.returned,
                score.matched_offset,
                2.0 * score.matched as f64 / (score.expected + score.returned).max(1) as f64,
                score.onset_error * 1000.0 / n,
                score.duration_error * 1000.0 / n
            );
        }
        assert!(
            groups
                .get("Isolated bass")
                .is_some_and(|score| score.matched >= 10)
        );
        Ok(())
    }
}
