use crate::{audio::Clip, stretch::Marker};
use serde::{Deserialize, Serialize};

pub const MAX_BEATS: usize = 4096;
const MAX_PHRASES: usize = 128;
const MIN_BEAT_GAP: f64 = 0.035;
const MATCH_SECONDS: f64 = 0.10;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    pub seconds: f64,
    #[serde(default)]
    pub downbeat: bool,
    #[serde(default)]
    pub anchor: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

impl Beat {
    pub fn manual(seconds: f64) -> Self {
        Self {
            seconds,
            downbeat: false,
            anchor: true,
            confidence: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub model: String,
    pub version: String,
    pub fingerprint: String,
    pub range: (f64, f64),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeatMap {
    pub beats: Vec<Beat>,
    pub first_downbeat: Option<f64>,
    pub provenance: Option<Provenance>,
    pub phrases: Vec<f64>,
    suppressed: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WarpPlan {
    pub range: (f64, f64),
    pub beats: f64,
    pub markers: Vec<Marker>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarpSource {
    pub parent: usize,
    pub algorithm: String,
    pub fingerprint: String,
    pub range: (f64, f64),
    pub target_bpm: u16,
    pub target_beats: f64,
    pub markers: Vec<Marker>,
}

impl BeatMap {
    pub fn validate(&mut self, duration: f64) {
        self.beats.retain_mut(|beat| {
            beat.confidence = beat
                .confidence
                .filter(|value| value.is_finite())
                .map(|value| value.clamp(0.0, 1.0));
            beat.seconds.is_finite() && (0.0..=duration).contains(&beat.seconds)
        });
        self.beats.sort_by(|a, b| a.seconds.total_cmp(&b.seconds));
        let mut kept: Vec<Beat> = Vec::with_capacity(self.beats.len().min(MAX_BEATS));
        for beat in self.beats.drain(..) {
            if let Some(last) = kept.last_mut()
                && beat.seconds - last.seconds < MIN_BEAT_GAP
            {
                if beat.anchor && !last.anchor {
                    *last = beat;
                }
                continue;
            }
            if kept.len() == MAX_BEATS {
                break;
            }
            kept.push(beat);
        }
        self.beats = kept;
        self.suppressed
            .retain(|&at| at.is_finite() && (0.0..=duration).contains(&at));
        self.suppressed.truncate(MAX_BEATS);
        self.phrases
            .retain(|&at| at.is_finite() && (0.0..=duration).contains(&at));
        self.phrases.sort_by(f64::total_cmp);
        self.phrases.dedup();
        self.phrases.truncate(MAX_PHRASES);
        self.first_downbeat = self
            .first_downbeat
            .filter(|at| self.beats.iter().any(|beat| beat.seconds == *at));
        self.rebar();
    }

    pub fn adopt(&mut self, beats: &[Beat], provenance: Provenance, duration: f64) {
        let (start, end) = provenance.range;
        self.beats
            .retain(|beat| beat.anchor || beat.seconds < start || beat.seconds > end);
        let anchors: Vec<_> = self.beats.iter().map(|beat| beat.seconds).collect();
        self.beats.extend(beats.iter().copied().filter(|beat| {
            !anchors
                .iter()
                .chain(&self.suppressed)
                .any(|&at| (at - beat.seconds).abs() < MATCH_SECONDS)
        }));
        self.provenance = Some(provenance);
        self.validate(duration);
    }

    pub fn add(&mut self, at: f64, duration: f64) -> Option<usize> {
        if self.beats.len() >= MAX_BEATS || !(0.0..=duration).contains(&at) {
            return None;
        }
        if let Some(index) = self
            .beats
            .iter()
            .position(|beat| (beat.seconds - at).abs() < MIN_BEAT_GAP)
        {
            self.beats[index].anchor = true;
            self.beats[index].confidence = None;
            return Some(index);
        }
        let index = self.beats.partition_point(|beat| beat.seconds < at);
        self.beats.insert(index, Beat::manual(at));
        self.rebar();
        Some(index)
    }

    pub fn move_beat(&mut self, index: usize, at: f64, duration: f64) -> bool {
        let Some(old) = self.beats.get(index).copied() else {
            return false;
        };
        let start = index
            .checked_sub(1)
            .map_or(0.0, |before| self.beats[before].seconds + MIN_BEAT_GAP);
        let end = self
            .beats
            .get(index + 1)
            .map_or(duration, |beat| beat.seconds - MIN_BEAT_GAP);
        if !(start..=end).contains(&at) || at == old.seconds {
            return false;
        }
        self.suppress(old.seconds);
        self.beats[index] = Beat {
            downbeat: old.downbeat,
            ..Beat::manual(at)
        };
        if self.first_downbeat == Some(old.seconds) {
            self.first_downbeat = Some(at);
        }
        for phrase in &mut self.phrases {
            if *phrase == old.seconds {
                *phrase = at;
            }
        }
        self.phrases.sort_by(f64::total_cmp);
        self.rebar();
        true
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.beats.len() {
            return;
        }
        let beat = self.beats.remove(index);
        self.suppress(beat.seconds);
        if self.first_downbeat == Some(beat.seconds) {
            self.first_downbeat = None;
        }
        self.phrases.retain(|&phrase| phrase != beat.seconds);
        self.rebar();
    }

    fn suppress(&mut self, at: f64) {
        if self.suppressed.len() < MAX_BEATS
            && !self
                .suppressed
                .iter()
                .any(|old| (old - at).abs() < MIN_BEAT_GAP)
        {
            self.suppressed.push(at);
        }
    }

    pub fn set_downbeat(&mut self, index: usize) {
        if let Some(beat) = self.beats.get_mut(index) {
            beat.anchor = true;
            beat.confidence = None;
            self.first_downbeat = Some(beat.seconds);
            self.rebar();
        }
    }

    fn rebar(&mut self) {
        if let Some(at) = self.first_downbeat
            && let Some(phase) = self.beats.iter().position(|beat| beat.seconds == at)
        {
            for (index, beat) in self.beats.iter_mut().enumerate() {
                beat.downbeat = index >= phase && (index - phase).is_multiple_of(4);
            }
        }
    }

    pub fn reinterpret(&mut self, double: bool, duration: f64) {
        let phase = self
            .beats
            .iter()
            .position(|beat| beat.downbeat)
            .unwrap_or(0);
        if self.first_downbeat.is_none() {
            self.set_downbeat(phase);
        }
        if double {
            let inserted: Vec<_> = self
                .beats
                .windows(2)
                .filter(|pair| pair[1].seconds - pair[0].seconds >= MIN_BEAT_GAP * 2.0)
                .take(MAX_BEATS.saturating_sub(self.beats.len()))
                .map(|pair| Beat {
                    seconds: (pair[0].seconds + pair[1].seconds) * 0.5,
                    downbeat: false,
                    anchor: false,
                    confidence: None,
                })
                .collect();
            self.beats.extend(inserted);
        } else {
            let mut index = 0;
            self.beats.retain(|beat| {
                let keep = beat.anchor || index % 2 == phase % 2;
                index += 1;
                keep
            });
        }
        self.validate(duration);
    }

    pub fn clear_analysis(&mut self) {
        self.beats.retain(|beat| beat.anchor);
        self.provenance = None;
        self.rebar();
    }

    pub fn toggle_phrase(&mut self, index: usize) {
        if let Some(beat) = self.beats.get(index) {
            match self
                .phrases
                .binary_search_by(|at| at.total_cmp(&beat.seconds))
            {
                Ok(index) => {
                    self.phrases.remove(index);
                }
                Err(index) if self.phrases.len() < MAX_PHRASES => {
                    self.phrases.insert(index, beat.seconds)
                }
                _ => {}
            }
        }
    }

    pub fn suggest_phrases(&mut self, clip: &Clip) {
        let bars: Vec<_> = self
            .beats
            .iter()
            .filter(|beat| beat.downbeat)
            .map(|beat| beat.seconds)
            .collect();
        let Some(&first) = bars.first() else { return };
        let mut result = vec![first];
        let mut index = 0;
        while index + 3 < bars.len() && result.len() < MAX_PHRASES {
            let score = |candidate: usize| {
                let at = bars[candidate];
                let before = clip.peak_between((at - 0.16).max(0.0), at);
                let attack = clip.peak_between(at, at + 0.075);
                (attack - before) * 0.3 - (candidate - index).abs_diff(4) as f32 * 0.04
            };
            index = (index + 3..=(index + 5).min(bars.len() - 1))
                .max_by(|&a, &b| score(a).total_cmp(&score(b)))
                .unwrap_or(index + 3);
            result.push(bars[index]);
        }
        self.phrases = result;
    }

    pub fn slice_points(&self, range: (f64, f64), bars: bool, subdivisions: usize) -> Vec<f64> {
        let inside = |at| range.0 + 0.001 < at && at < range.1 - 0.001;
        if bars {
            return self
                .beats
                .iter()
                .filter(|beat| beat.downbeat && inside(beat.seconds))
                .take(super::STEPS - 1)
                .map(|beat| beat.seconds)
                .collect();
        }
        let subdivisions = subdivisions.clamp(1, 4);
        let points = self
            .beats
            .windows(2)
            .flat_map(|pair| {
                (0..subdivisions).map(move |subdivision| {
                    pair[0].seconds
                        + (pair[1].seconds - pair[0].seconds) * subdivision as f64
                            / subdivisions as f64
                })
            })
            .chain(self.beats.last().map(|beat| beat.seconds));
        points
            .filter(|&at| inside(at))
            .take(super::STEPS - 1)
            .collect()
    }

    pub fn warp_plan(&self, range: (f64, f64), bpm: u16) -> crate::files::Result<WarpPlan> {
        let beats: Vec<_> = self
            .beats
            .iter()
            .filter(|beat| range.0 - 0.001 <= beat.seconds && beat.seconds <= range.1 + 0.001)
            .map(|beat| beat.seconds)
            .collect();
        if !(2..=129).contains(&beats.len()) {
            return Err("Select between 2 and 129 beat markers to propose a warp.".into());
        }
        let period = 60.0 / f64::from(bpm);
        for (index, pair) in beats.windows(2).enumerate() {
            let ratio = period / (pair[1] - pair[0]);
            if !(0.5..=2.0).contains(&ratio) {
                return Err(format!("Beats {}–{} require {:.2}× stretch. Correct that interval or choose another range.", index + 1, index + 2, ratio).into());
            }
        }
        let end = beats.len() - 1;
        let stride = (end - 1).div_ceil(Marker::MAX).max(1);
        let markers = (1..end)
            .step_by(stride)
            .map(|index| Marker {
                source_seconds: beats[index],
                target_beat: index as f64,
            })
            .collect();
        Ok(WarpPlan {
            range: (beats[0], beats[end]),
            beats: end as f64,
            markers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inferred(seconds: f64) -> Beat {
        Beat {
            seconds,
            downbeat: false,
            anchor: false,
            confidence: Some(0.4),
        }
    }

    #[test]
    fn reanalysis_respects_moved_and_removed_beats_and_a_pickup() {
        let input: Vec<_> = [0.25, 0.75, 1.3, 1.85, 2.42, 3.0, 3.57, 4.1]
            .into_iter()
            .map(inferred)
            .collect();
        let mut map = BeatMap {
            beats: input.clone(),
            ..BeatMap::default()
        };
        map.set_downbeat(1);
        assert!(!map.beats[0].downbeat);
        assert!(map.move_beat(2, 1.42, 5.0));
        map.remove(3);
        map.adopt(
            &input,
            Provenance {
                model: "test".into(),
                version: "1".into(),
                fingerprint: "fixture".into(),
                range: (0.0, 5.0),
            },
            5.0,
        );
        assert_eq!(map.first_downbeat, Some(0.75));
        assert!(
            map.beats
                .iter()
                .any(|beat| beat.seconds == 1.42 && beat.anchor)
        );
        assert!(
            !map.beats
                .iter()
                .any(|beat| [1.3, 1.85].contains(&beat.seconds))
        );
        let stored = serde_json::to_vec(&map).unwrap();
        let mut reopened: BeatMap = serde_json::from_slice(&stored).unwrap();
        reopened.clear_analysis();
        assert_eq!(reopened.beats.len(), 2);
        assert_eq!(reopened.first_downbeat, Some(0.75));
    }

    #[test]
    fn drifted_map_slices_and_maps_to_the_target_grid() {
        let mut map = BeatMap {
            beats: [0.18, 0.66, 1.15, 1.68, 2.21, 2.77, 3.34, 3.9, 4.47]
                .into_iter()
                .map(inferred)
                .collect(),
            ..BeatMap::default()
        };
        map.set_downbeat(0);
        assert_eq!(map.slice_points((0.18, 4.47), true, 1), [2.21]);
        let plan = map.warp_plan((0.0, 4.6), 120).unwrap();
        assert_eq!(plan.range, (0.18, 4.47));
        assert_eq!(plan.beats, 8.0);
        assert_eq!(
            plan.markers[3],
            Marker {
                source_seconds: 2.21,
                target_beat: 4.0
            }
        );
        map.reinterpret(true, 5.0);
        assert_eq!(map.beats.len(), 17);
        assert_eq!(map.first_downbeat, Some(0.18));
        map.reinterpret(false, 5.0);
        assert_eq!(map.beats.len(), 9);
        assert!(map.move_beat(3, 1.19, 5.0));
        assert!(
            map.warp_plan((0.0, 4.6), 120)
                .unwrap_err()
                .to_string()
                .contains("Beats 3–4")
        );
    }
}
