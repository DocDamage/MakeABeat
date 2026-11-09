use crate::model::{QueueEntry, Record, Source, Verdict};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const UNDO_LIMIT: usize = 32;
pub const RANK_LIMIT: usize = 96;
pub const FEEDBACK_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    #[default]
    Unspecified,
    Context,
    Sound,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub record: Record,
    pub range: Option<(f64, f64)>,
    pub verdict: Verdict,
    pub heard: bool,
    pub reason: SkipReason,
}

impl Feedback {
    pub(crate) fn negative_weight(&self) -> f32 {
        if self.verdict != Verdict::Skip || !self.heard {
            return 0.0;
        }
        match self.reason {
            SkipReason::Unspecified => 0.12,
            SkipReason::Context => 0.0,
            SkipReason::Sound => 0.30,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Change {
    source: Source,
    verdict: Verdict,
    previous: Option<(usize, Feedback)>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub enabled: bool,
    pub adventure: u8,
    pub feedback: Vec<Feedback>,
    undo: Vec<Vec<Change>>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            enabled: false,
            adventure: 35,
            feedback: Vec::new(),
            undo: Vec::new(),
        }
    }
}

impl Profile {
    pub fn is_default(&self) -> bool {
        !self.enabled && self.adventure == 35 && self.feedback.is_empty() && self.undo.is_empty()
    }

    pub fn counts(&self) -> (usize, usize) {
        (
            self.feedback
                .iter()
                .filter(|entry| entry.verdict == Verdict::Keep)
                .count(),
            self.feedback
                .iter()
                .filter(|entry| entry.negative_weight() > 0.0)
                .count(),
        )
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    fn take_feedback(&mut self, source: &Source) -> Option<(usize, Feedback)> {
        let index = self
            .feedback
            .iter()
            .position(|item| &item.record.source == source)?;
        Some((index, self.feedback.remove(index)))
    }

    pub fn decide(
        &mut self,
        queue: &mut [QueueEntry],
        sources: &[Source],
        verdict: Verdict,
        reason: SkipReason,
    ) -> bool {
        let mut changes = Vec::new();
        for entry in queue {
            if !sources.contains(&entry.record.source) || entry.verdict == verdict {
                continue;
            }
            let previous = self.take_feedback(&entry.record.source);
            changes.push(Change {
                source: entry.record.source.clone(),
                verdict: entry.verdict,
                previous,
            });
            entry.verdict = verdict;
            if self.enabled && matches!(verdict, Verdict::Keep | Verdict::Skip) {
                self.feedback.push(Feedback {
                    record: entry.record.clone(),
                    range: entry.match_range,
                    verdict,
                    heard: entry.taste_heard,
                    reason,
                });
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.undo.push(changes);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.drain(..self.undo.len() - UNDO_LIMIT);
        }
        true
    }

    pub fn undo(&mut self, queue: &mut [QueueEntry]) -> Option<(Source, Verdict)> {
        let changes = self.undo.pop()?;
        let focus = changes
            .first()
            .map(|change| (change.source.clone(), change.verdict));
        for change in changes.into_iter().rev() {
            self.take_feedback(&change.source);
            if let Some((index, previous)) = change.previous {
                self.feedback
                    .insert(index.min(self.feedback.len()), previous);
            }
            if let Some(entry) = queue
                .iter_mut()
                .find(|entry| entry.record.source == change.source)
            {
                entry.verdict = change.verdict;
            }
        }
        focus
    }

    pub fn reset(&mut self) {
        self.feedback.clear();
        for change in self.undo.iter_mut().flatten() {
            change.previous = None;
        }
    }

    pub fn relink(&mut self, old: &Source, new: &Source) {
        if old == new {
            return;
        }
        if self.feedback.iter().any(|item| &item.record.source == new) {
            self.take_feedback(old);
        }
        for item in &mut self.feedback {
            if &item.record.source == old {
                item.record.source = new.clone();
            }
        }
        for change in self.undo.iter_mut().flatten() {
            if &change.source == old {
                change.source = new.clone();
            }
            if let Some((_, previous)) = &mut change.previous
                && &previous.record.source == old
            {
                previous.record.source = new.clone();
            }
        }
    }
}

pub type Vectors = HashMap<Source, Vec<f32>>;

#[derive(Clone)]
pub struct Item {
    pub record: Record,
    pub range: Option<(f64, f64)>,
}

#[derive(Clone, Default)]
struct Match {
    audio: f32,
    fields: Vec<&'static str>,
    metadata: f32,
}

impl Match {
    fn similarity(&self) -> f32 {
        self.audio.max(self.metadata)
    }

    fn affinity(&self) -> f32 {
        self.audio.max(self.metadata) + self.audio.min(self.metadata) * 0.25
    }
}

fn compare(a: &Record, b: &Record, vectors: &Vectors) -> Match {
    let equal = |a: &str, b: &str| !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim());
    let mut result = Match::default();
    if let (Some(a), Some(b)) = (vectors.get(&a.source), vectors.get(&b.source)) {
        let cosine = a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
        result.audio = ((cosine - 0.25) / 0.75).clamp(0.0, 1.0);
    }
    for (matched, field, weight) in [
        (
            equal(&a.artist, &b.artist)
                && !a.artist.starts_with("Lost Samples")
                && !["local audio", "unknown", "unknown artist"]
                    .iter()
                    .any(|name| a.artist.trim().eq_ignore_ascii_case(name)),
            "artist",
            0.40,
        ),
        (
            equal(&a.genre, &b.genre)
                && !["all", "local", "other", "unknown"]
                    .iter()
                    .any(|genre| a.genre.trim().eq_ignore_ascii_case(genre)),
            "genre",
            0.30,
        ),
        (
            a.release
                .as_ref()
                .zip(b.release.as_ref())
                .is_some_and(|(a, b)| equal(&a.label, &b.label)),
            "label",
            0.25,
        ),
        (
            a.release
                .as_ref()
                .zip(b.release.as_ref())
                .is_some_and(|(a, b)| a.year > 0 && b.year > 0 && a.year / 10 == b.year / 10),
            "decade",
            0.15,
        ),
    ] {
        if matched {
            result.fields.push(field);
            result.metadata += weight;
        }
    }
    result.metadata = result.metadata.min(1.0);
    result
}

#[derive(Clone)]
struct Evidence {
    title: String,
    matches: Match,
    value: f32,
}

#[derive(Clone)]
pub struct Ranked {
    /// Position in the supplied candidate list.
    pub candidate: usize,
    pub score: f32,
    pub explanation: String,
}

struct Score {
    positive: Option<Evidence>,
    negative: Option<Evidence>,
    affinity: f32,
}

fn evidence(candidate: &Record, feedback: &[Feedback], vectors: &Vectors) -> Score {
    let mut positive: Option<Evidence> = None;
    let mut negative: Option<Evidence> = None;
    for item in feedback {
        let keep = item.verdict == Verdict::Keep;
        let weight = if keep { 1.0 } else { item.negative_weight() };
        if weight == 0.0 || item.record.source == candidate.source {
            continue;
        }
        let matches = compare(candidate, &item.record, vectors);
        let value = matches.affinity() * weight;
        let best = if keep { &mut positive } else { &mut negative };
        if value > 0.0 && best.as_ref().is_none_or(|current| value > current.value) {
            *best = Some(Evidence {
                title: item.record.title.clone(),
                matches,
                value,
            });
        }
    }
    let affinity = positive.as_ref().map_or(0.0, |item| item.value)
        - negative.as_ref().map_or(0.0, |item| item.value);
    Score {
        positive,
        negative,
        affinity,
    }
}

fn describe(item: &Evidence, verdict: &str) -> String {
    let mut fields = item.matches.fields.clone();
    if item.matches.audio > 0.0 {
        fields.insert(0, "audio");
    }
    format!(
        "{} match with {verdict} “{}”",
        fields.join(", "),
        item.title
    )
}

pub fn rank(
    candidates: &[Item],
    feedback: &[Feedback],
    vectors: &Vectors,
    adventure: u8,
) -> Vec<Ranked> {
    if !feedback.iter().any(|entry| entry.verdict == Verdict::Keep) {
        return Vec::new();
    }
    let candidates = &candidates[..candidates.len().min(RANK_LIMIT)];
    let scores: Vec<_> = candidates
        .iter()
        .map(|item| evidence(&item.record, feedback, vectors))
        .collect();
    if !scores.iter().any(|score| score.positive.is_some()) {
        return Vec::new();
    }
    let adventure = f32::from(adventure.min(100)) / 100.0;
    let mut selected = vec![false; candidates.len()];
    let mut overlap = vec![0.0_f32; candidates.len()];
    let mut ranked = Vec::with_capacity(candidates.len());
    for position in 0..candidates.len() {
        let variety = position % 4 == 3;
        let mut choice = None;
        for (index, score) in scores
            .iter()
            .enumerate()
            .filter(|(index, _)| !selected[*index])
        {
            let penalty = if variety {
                0.95
            } else {
                0.12 + 0.63 * adventure
            };
            let affinity = if variety {
                0.25
            } else {
                1.0 - 0.55 * adventure
            };
            let value = score.affinity * affinity - overlap[index] * penalty;
            if choice.is_none_or(|(_, current)| value > current) {
                choice = Some((index, value));
            }
        }
        let (index, score) = choice.unwrap();
        let mut reasons = Vec::new();
        if let Some(keep) = &scores[index].positive {
            reasons.push(describe(keep, "kept"));
        }
        if let Some(skip) = &scores[index].negative {
            reasons.push(describe(skip, "skipped"));
        }
        if overlap[index] > 0.0 {
            reasons.push("Overlap with earlier suggestions reduced rank score".into());
        }
        if variety
            && overlap
                .iter()
                .enumerate()
                .any(|(i, overlap)| !selected[i] && *overlap > 0.0)
        {
            reasons.push("Variety slot gives more weight to lower overlap".into());
        }
        if reasons.is_empty() {
            reasons.push("No matching feedback signals".into());
        }
        ranked.push(Ranked {
            candidate: index,
            score,
            explanation: reasons.join(" · "),
        });
        selected[index] = true;
        for (other, item) in candidates.iter().enumerate().filter(|(i, _)| !selected[*i]) {
            overlap[other] = overlap[other]
                .max(compare(&candidates[index].record, &item.record, vectors).similarity());
        }
    }
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{files, library_index::Index};
    use std::path::PathBuf;

    fn record(index: usize, genre: &str) -> Record {
        Record {
            title: format!("Record {index}"),
            artist: String::new(),
            genre: genre.into(),
            source: Source::Demo(index),
            release: None,
        }
    }

    fn feedback(record: Record, verdict: Verdict, heard: bool, reason: SkipReason) -> Feedback {
        Feedback {
            record,
            verdict,
            heard,
            reason,
            range: None,
        }
    }

    #[test]
    fn undo_survives_serialization_and_reset_cannot_restore_feedback() -> files::Result<()> {
        let mut queue: Vec<_> = (0..4)
            .map(|index| QueueEntry::new(record(index, "Jazz"), None))
            .collect();
        let mut profile = Profile::default();
        assert!(profile.decide(
            &mut queue,
            &[Source::Demo(0)],
            Verdict::Keep,
            SkipReason::Unspecified
        ));
        assert!(profile.feedback.is_empty());
        profile.enabled = true;
        queue[1].taste_heard = true;
        queue[3].taste_heard = true;
        assert!(profile.decide(
            &mut queue,
            &[Source::Demo(0)],
            Verdict::Pending,
            SkipReason::Unspecified
        ));
        assert!(profile.decide(
            &mut queue,
            &[Source::Demo(0)],
            Verdict::Keep,
            SkipReason::Unspecified
        ));
        assert!(profile.decide(
            &mut queue,
            &[Source::Demo(1)],
            Verdict::Skip,
            SkipReason::Context
        ));
        assert!(profile.decide(
            &mut queue,
            &[Source::Demo(2), Source::Demo(3)],
            Verdict::Skip,
            SkipReason::Sound
        ));
        assert_eq!(profile.counts(), (1, 1));
        let before = profile.feedback.clone();
        assert!(profile.decide(
            &mut queue,
            &[Source::Demo(0), Source::Demo(3)],
            Verdict::Later,
            SkipReason::Unspecified
        ));
        assert_eq!(profile.counts(), (0, 0));
        let (mut profile, mut queue): (Profile, Vec<QueueEntry>) =
            serde_json::from_slice(&serde_json::to_vec(&(profile, queue))?)?;
        assert_eq!(
            profile.undo(&mut queue),
            Some((Source::Demo(0), Verdict::Keep))
        );
        assert_eq!(profile.feedback, before);
        assert_eq!(queue[3].verdict, Verdict::Skip);
        profile.reset();
        while profile.can_undo() {
            profile.undo(&mut queue);
            assert!(profile.feedback.is_empty());
        }
        assert!(queue.iter().all(|item| item.verdict == Verdict::Pending));
        Ok(())
    }

    #[test]
    fn uses_audio_and_metadata_and_reserves_variety_in_the_familiar_view() {
        let candidates: Vec<_> = (0..7)
            .map(|index| Item {
                record: record(
                    index,
                    if index == 0 || index == 6 {
                        "Soul"
                    } else {
                        "Jazz"
                    },
                ),
                range: None,
            })
            .collect();
        let mut vectors = Vectors::new();
        for index in 1..=5 {
            vectors.insert(Source::Demo(index), vec![1.0, 0.0]);
        }
        vectors.insert(Source::Demo(10), vec![1.0, 0.0]);
        assert!(rank(&candidates, &[], &vectors, 0).is_empty());
        let mut feedback = vec![feedback(
            record(10, "Jazz"),
            Verdict::Keep,
            true,
            SkipReason::Unspecified,
        )];
        let familiar = rank(&candidates, &feedback, &vectors, 0);
        assert_eq!(familiar[0].candidate, 1);
        assert!(
            familiar[0]
                .explanation
                .contains("audio, genre match with kept")
        );
        assert_eq!(familiar[3].candidate, 0);
        assert!(familiar[3].explanation.contains("Variety slot"));
        let diverse = rank(&candidates, &feedback, &vectors, 100);
        assert_eq!(diverse[1].candidate, 0);
        feedback.push(self::feedback(
            record(11, "Jazz"),
            Verdict::Skip,
            false,
            SkipReason::Sound,
        ));
        let unheard = rank(&candidates, &feedback, &vectors, 0);
        assert_eq!(unheard[0].score, familiar[0].score);
        assert!(!unheard[0].explanation.contains("skipped"));
        feedback.last_mut().unwrap().heard = true;
        let heard = rank(&candidates, &feedback, &vectors, 0);
        assert!(heard[0].score < familiar[0].score);
        assert!(heard[0].explanation.contains("genre match with skipped"));
    }

    #[test]
    #[ignore = "requires the pinned CLAP snapshot and the Phase 5 NSynth evaluation corpus"]
    fn taste_ranking_on_held_out_nsynth_feedback() -> files::Result<()> {
        let corpus = PathBuf::from(
            std::env::var_os("LOST_SAMPLES_SEARCH_CORPUS")
                .ok_or("Set LOST_SAMPLES_SEARCH_CORPUS.")?,
        );
        let python = PathBuf::from(
            std::env::var_os("LOST_SAMPLES_SEARCH_PYTHON").unwrap_or_else(|| "python".into()),
        );
        let tmp = tempfile::tempdir()?;
        let mut index = Index::open(tmp.path())?;
        let mut classes = Vec::new();
        for folder in std::fs::read_dir(corpus)? {
            let folder = folder?;
            let class = folder.file_name().to_string_lossy().into_owned();
            if !class.starts_with("nsynth-") {
                continue;
            }
            index.add_folder(&folder.path(), "")?;
            let mut paths = std::fs::read_dir(folder.path())?
                .map(|file| file.map(|file| file.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            paths.sort();
            let items = paths
                .into_iter()
                .take(4)
                .map(|path| Item {
                    record: files::local_record(path),
                    range: None,
                })
                .collect::<Vec<_>>();
            assert_eq!(items.len(), 4);
            classes.push((class, items));
        }
        classes.sort_by(|a, b| a.0.cmp(&b.0));
        assert!(classes.len() >= 8);
        index.scan()?;
        index.index(&mut None, &python)?;
        let items: Vec<_> = classes
            .iter()
            .flat_map(|(_, items)| items.iter().cloned())
            .collect();
        let vectors = index.taste_vectors(&items)?;
        assert_eq!(vectors.len(), items.len());
        let dcg = |order: &[usize], candidates: &[(usize, Item)], target: usize| {
            order
                .iter()
                .take(5)
                .enumerate()
                .filter(|(_, index)| candidates[**index].0 == target)
                .map(|(rank, _)| 1.0 / ((rank + 2) as f64).log2())
                .sum::<f64>()
                / (1.0 + 1.0 / 3.0_f64.log2())
        };
        let mut sums = [0.0; 3];
        let mut sessions = 0;
        for target in 0..classes.len() {
            for split in 0..4 {
                let keep = &classes[target].1;
                let skip = &classes[(target + 1) % classes.len()].1;
                let feedback = [
                    feedback(
                        keep[split].record.clone(),
                        Verdict::Keep,
                        true,
                        SkipReason::Unspecified,
                    ),
                    feedback(
                        keep[(split + 1) % 4].record.clone(),
                        Verdict::Keep,
                        true,
                        SkipReason::Unspecified,
                    ),
                    feedback(
                        skip[split].record.clone(),
                        Verdict::Skip,
                        true,
                        SkipReason::Sound,
                    ),
                ];
                let mut available: Vec<_> = classes
                    .iter()
                    .enumerate()
                    .flat_map(|(class, (_, items))| {
                        items
                            .iter()
                            .filter(|item| {
                                !feedback
                                    .iter()
                                    .any(|f| f.record.source == item.record.source)
                            })
                            .cloned()
                            .map(move |item| (class, item))
                    })
                    .collect();
                let len = available.len();
                available.rotate_left((target * 11 + split * 7) % len);
                let candidates = available
                    .iter()
                    .map(|(_, item)| item.clone())
                    .collect::<Vec<_>>();
                sums[0] += dcg(&(0..len).collect::<Vec<_>>(), &available, target);
                for (column, adventure) in [(1, 0), (2, 80)] {
                    let ranked = rank(&candidates, &feedback, &vectors, adventure);
                    let order = if ranked.is_empty() {
                        (0..len).collect()
                    } else {
                        ranked
                            .into_iter()
                            .map(|row| row.candidate)
                            .collect::<Vec<_>>()
                    };
                    sums[column] += dcg(&order, &available, target);
                }
                sessions += 1;
            }
        }
        let scores = sums.map(|sum| sum / f64::from(sessions));
        println!(
            "{sessions} simulated held-out feedback sessions. nDCG@5 queue={:.3} familiar={:.3} adventurous={:.3}",
            scores[0], scores[1], scores[2]
        );
        Ok(())
    }
}
