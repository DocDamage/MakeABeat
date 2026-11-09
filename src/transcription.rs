pub mod midi;
pub mod worker;

use crate::{
    audio::{Clip, PadMode},
    files::Result,
    model::{
        ChopInput, MAX_BARS, MAX_SONG_BARS, PIANO_HIGH, PIANO_LOW, PlayedChop, Sequence,
        TICKS_PER_BAR,
        beat_map::BeatMap,
        melody::{self, Bend, Expression, Instrument},
    },
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_SECONDS: f64 = 180.0;
pub const MAX_NOTES: usize = 8192;
const PPQ: f64 = TICKS_PER_BAR as f64 / 4.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Selection,
    FirstNote,
    Downbeat,
}

#[derive(Clone, Copy, Serialize)]
pub struct Detection {
    pub onset: f32,
    pub frame: f32,
    pub min_ms: u16,
    pub recover_sustains: bool,
    pub low: u8,
    pub high: u8,
}

impl Default for Detection {
    fn default() -> Self {
        Self {
            onset: 0.5,
            frame: 0.3,
            min_ms: 110,
            recover_sustains: false,
            low: 21,
            high: 108,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Candidate {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub strength: f32,
    pub bends: Vec<i16>,
}

pub struct Conversion {
    pub notes: Vec<PlayedChop>,
    pub unsupported: Vec<u8>,
}

fn beat_position(map: &BeatMap, seconds: f64) -> f64 {
    let i = map
        .beats
        .partition_point(|beat| beat.seconds <= seconds)
        .saturating_sub(1)
        .min(map.beats.len() - 2);
    let (left, right) = (map.beats[i].seconds, map.beats[i + 1].seconds);
    i as f64 + (seconds - left) / (right - left)
}

pub fn convert(
    candidates: &[Candidate],
    range: (f64, f64),
    map: Option<&BeatMap>,
    bpm: u16,
    alignment: Alignment,
    detection: Detection,
) -> Result<Conversion> {
    let map = map.filter(|map| map.beats.len() >= 2);
    if map.is_none() && !(30..=300).contains(&bpm) {
        return Err("Choose a source tempo between 30 and 300 BPM.".into());
    }
    let beat = |seconds: f64| {
        map.map_or(seconds * f64::from(bpm) / 60.0, |map| {
            beat_position(map, seconds)
        })
    };
    let mut unsupported = Vec::new();
    let selected: Vec<_> = candidates
        .iter()
        .filter(|note| {
            if !(PIANO_LOW..=PIANO_HIGH).contains(&note.pitch) {
                unsupported.push(note.pitch);
                return false;
            }
            (detection.low..=detection.high).contains(&note.pitch)
                && note.start < range.1
                && note.end > range.0
        })
        .collect();
    unsupported.sort_unstable();
    unsupported.dedup();
    let start = match alignment {
        Alignment::Selection => range.0,
        Alignment::FirstNote => selected
            .iter()
            .map(|note| note.start)
            .min_by(f64::total_cmp)
            .unwrap_or(range.0),
        Alignment::Downbeat => range.0,
    };
    let mut origin = beat(start);
    if alignment == Alignment::Downbeat
        && let Some(downbeat) = map.and_then(|map| map.beats.iter().find(|beat| beat.downbeat))
    {
        let anchor = beat(downbeat.seconds);
        origin = anchor + ((origin - anchor) / 4.0).floor() * 4.0;
    }
    let tick = |seconds: f64| ((beat(seconds) - origin) * PPQ).round() as i64;
    let mut notes = Vec::with_capacity(selected.len());
    for candidate in selected {
        let first = tick(candidate.start);
        let start = tick(candidate.start.max(range.0)).max(0) as u32;
        let end = (tick(candidate.end.min(range.1)).max(0) as u32).max(start.saturating_add(1));
        if end > MAX_SONG_BARS as u32 * TICKS_PER_BAR {
            return Err(
                "The converted phrase exceeds the song length limit. Choose a shorter passage."
                    .into(),
            );
        }
        let duration = candidate.end - candidate.start;
        let mut bends: Vec<Bend> = Vec::new();
        for (index, &cents) in candidate.bends.iter().enumerate() {
            let seconds = candidate.start
                + duration * index as f64 / candidate.bends.len().saturating_sub(1).max(1) as f64;
            let offset = tick(seconds).saturating_sub(first).max(0) as u32;
            let point = Bend {
                tick: offset,
                cents,
            };
            if let Some(last) = bends.last_mut().filter(|last| last.tick == offset) {
                *last = point;
            } else if bends.len() >= 2
                && bends[bends.len() - 2..]
                    .iter()
                    .all(|prev| prev.cents == cents)
            {
                *bends.last_mut().unwrap() = point;
            } else {
                bends.push(point);
            }
        }
        let bends = melody::trim(
            &bends,
            (i64::from(start) - first) as u32,
            (i64::from(end) - first) as u32,
        );
        notes.push(PlayedChop {
            id: Default::default(),
            tick: start,
            input: ChopInput::Key(candidate.pitch),
            velocity: (candidate.strength.clamp(0.0, 1.0) * 127.0)
                .round()
                .max(1.0) as u8,
            mode: PadMode::Gate,
            length: Some(end - start),
            sound: Default::default(),
            expression: Some(Arc::new(Expression {
                bends,
                ..Expression::default()
            })),
        });
    }
    notes.sort_by_key(|note| (note.tick, note.input));
    Ok(Conversion { notes, unsupported })
}

pub fn phrase(notes: &[PlayedChop], instrument: Instrument) -> Sequence {
    let mut notes = notes.to_vec();
    for note in &mut notes {
        if let Some(expression) = &mut note.expression {
            Arc::make_mut(expression).instrument = instrument;
        }
    }
    notes.sort_by_key(|note| (note.tick, note.input));
    let end = notes
        .iter()
        .map(|note| note.tick.saturating_add(note.length.unwrap_or(1)))
        .max()
        .unwrap_or(0);
    Sequence {
        bars: vec![Default::default(); end.div_ceil(TICKS_PER_BAR) as usize],
        performance: notes,
        ..Sequence::default()
    }
}

pub fn split(sequence: Sequence, mut chain: impl FnMut() -> u64) -> Vec<Sequence> {
    let mut parts: Vec<_> = sequence
        .bars
        .chunks(MAX_BARS)
        .map(|bars| Sequence {
            bars: bars.to_vec(),
            ..Sequence::default()
        })
        .collect();
    let span = MAX_BARS as u32 * TICKS_PER_BAR;
    for note in sequence.performance {
        let end = note.tick + note.length.unwrap_or(1);
        let mut cursor = note.tick;
        let id = ((end - 1) / span != note.tick / span).then(&mut chain);
        while cursor < end {
            let part = cursor / span;
            let next = end.min((part + 1) * span);
            let mut segment = PlayedChop {
                tick: cursor % span,
                length: Some(next - cursor),
                ..note.clone()
            };
            if let Some(id) = id
                && let Some(expression) = &mut segment.expression
            {
                let expression = Arc::make_mut(expression);
                expression.chain = Some(id);
                expression.continuation = cursor != note.tick;
                expression.bends =
                    melody::trim(&expression.bends, cursor - note.tick, next - note.tick);
            }
            parts[part as usize].performance.push(segment);
            cursor = next;
        }
    }
    for part in &mut parts {
        part.performance.sort_by_key(|note| (note.tick, note.input));
    }
    parts
}

pub fn sample_instrument(
    source: usize,
    clip: &Clip,
    range: (f64, f64),
    root: u8,
) -> Option<Instrument> {
    let at = |time: f64| {
        (time * f64::from(clip.rate))
            .round()
            .clamp(0.0, clip.frames.len() as f64) as u32
    };
    let (start, end) = (at(range.0), at(range.1));
    (start < end).then_some(Instrument::Sample {
        source,
        start,
        end,
        root,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Section, SectionKind, Studio, beat_map::Beat};

    pub fn note(tick: u32, length: u32, key: u8, cents: i16) -> PlayedChop {
        PlayedChop {
            id: Default::default(),
            tick,
            input: ChopInput::Key(key),
            velocity: 100,
            mode: PadMode::Gate,
            length: Some(length),
            sound: Default::default(),
            expression: Some(Arc::new(Expression {
                bends: if cents == 0 {
                    Vec::new()
                } else {
                    vec![Bend { tick: 0, cents }]
                },
                ..Expression::default()
            })),
        }
    }

    #[test]
    fn maps_irregular_beats_and_note_ends_without_quantizing_starts() -> Result<()> {
        let mut map = BeatMap::default();
        map.beats = [1.0, 1.4, 2.0, 2.5, 3.0]
            .into_iter()
            .map(Beat::manual)
            .collect();
        map.set_downbeat(0);
        let make = |start, end, pitch| Candidate {
            start,
            end,
            pitch,
            strength: 0.8,
            bends: vec![0, 33, 0],
        };
        let candidates = [make(1.1, 1.7, 42), make(1.35, 2.2, 47), make(1.2, 2.4, 12)];
        let result = convert(
            &candidates,
            (1.1, 2.5),
            Some(&map),
            90,
            Alignment::Downbeat,
            Detection::default(),
        )?;
        assert_eq!(result.unsupported, [12]);
        assert_eq!(
            (result.notes[0].tick, result.notes[0].length),
            (120, Some(600))
        );
        assert_eq!(
            (result.notes[1].tick, result.notes[1].length),
            (420, Some(732))
        );
        assert!(
            result.notes[0]
                .expression
                .as_ref()
                .unwrap()
                .bends
                .iter()
                .any(|p| p.cents == 33)
        );
        let manual = convert(
            &candidates,
            (1.0, 2.5),
            None,
            120,
            Alignment::FirstNote,
            Detection::default(),
        )?;
        assert_eq!(
            (manual.notes[0].tick, manual.notes[0].length),
            (0, Some(576))
        );
        assert_eq!(manual.notes[1].tick, 240);
        Ok(())
    }

    #[test]
    fn a_phrase_split_and_join_retains_duration_and_bends() {
        let boundary = MAX_BARS as u32 * TICKS_PER_BAR;
        let mut held = note(boundary - 240, 480, 50, 0);
        Arc::make_mut(held.expression.as_mut().unwrap()).bends = vec![
            Bend { tick: 0, cents: 0 },
            Bend {
                tick: 480,
                cents: 100,
            },
        ];
        let mut studio = Studio::default();
        let sequence = phrase(&[held, note(boundary + 30, 160, 55, -33)], Instrument::Sine);
        let parts = split(sequence, || 9);
        assert_eq!(
            parts.iter().map(|p| p.bars.len()).collect::<Vec<_>>(),
            [16, 1]
        );
        for (index, part) in parts.into_iter().enumerate() {
            studio
                .save_generated_pattern(&format!("Phrase {index}"), part)
                .unwrap();
            studio
                .sections
                .push(Section::new(SectionKind::Verse, index, 1));
        }
        let song = studio.song_sequence().unwrap();
        assert_eq!(song.performance.len(), 2);
        let held = &song.performance[0];
        assert_eq!((held.tick, held.length), (boundary - 240, Some(480)));
        let bends = &held.expression.as_ref().unwrap().bends;
        assert_eq!(melody::cents_at(bends, 240.0), 50.0);
        assert_eq!(melody::cents_at(bends, 360.0), 75.0);
    }
}
