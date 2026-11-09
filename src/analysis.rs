use crate::{audio::Clip, model::NamedSample};

const HOPS_PER_SECOND: f64 = 50.0;

#[derive(Clone, Copy, Default)]
struct Hop {
    power: f64,
    low: f64,
    high: f64,
    onset: f64,
}

pub struct AudioProfile {
    hops: Vec<Hop>,
    hop_seconds: f64,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Sound {
    features: [f64; 7],
    seconds: f64,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Emphasis {
    #[default]
    Balanced,
    Rhythmic,
    Percussive,
    Sparse,
    Tonal,
}

impl Emphasis {
    pub const OPTIONS: [(Self, &'static str); 5] = [
        (Self::Balanced, "Balanced"),
        (Self::Rhythmic, "Rhythmic"),
        (Self::Percussive, "Percussive"),
        (Self::Sparse, "Sparse"),
        (Self::Tonal, "Sustained / tonal"),
    ];

    pub fn label(self) -> &'static str {
        Self::OPTIONS
            .iter()
            .find(|(mode, _)| *mode == self)
            .unwrap()
            .1
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum MatchBy {
    #[default]
    Balanced,
    Rhythm,
    Brightness,
    Energy,
    Texture,
}

impl MatchBy {
    pub const OPTIONS: [(Self, &'static str); 5] = [
        (Self::Balanced, "Balanced"),
        (Self::Rhythm, "Rhythm"),
        (Self::Brightness, "Brightness"),
        (Self::Energy, "Energy"),
        (Self::Texture, "Texture"),
    ];

    pub fn label(self) -> &'static str {
        Self::OPTIONS
            .iter()
            .find(|(mode, _)| *mode == self)
            .unwrap()
            .1
    }
}

impl AudioProfile {
    pub fn new(clip: &Clip, range: (f64, f64)) -> Self {
        let step = (clip.rate / 8_000).max(1) as usize;
        let rate = f64::from(clip.rate) / step as f64;
        let hop = (rate / HOPS_PER_SECOND).round().max(1.0) as usize;
        let mut hops = Vec::new();
        let mut bass = [0.0; 2];
        let mut smooth = [0.0; 2];
        let mut previous: f64 = 0.0;
        let low_coefficient = 1.0 - (-std::f64::consts::TAU * 180.0 / rate).exp();
        let high_coefficient = 1.0 - (-std::f64::consts::TAU * 1_800.0 / rate).exp();
        if let Some(frames) = clip.region(range) {
            for (index, block) in frames.chunks(step * hop).enumerate() {
                if index.is_multiple_of(32)
                    && crate::background::progress(index, frames.len().div_ceil(step * hop)).is_err()
                {
                    break;
                }
                let mut entry = Hop::default();
                for frame in block.iter().step_by(step) {
                    for (channel, &sample) in frame.iter().enumerate() {
                        let sample = f64::from(sample);
                        bass[channel] += low_coefficient * (sample - bass[channel]);
                        smooth[channel] += high_coefficient * (sample - smooth[channel]);
                        entry.power += 0.5 * sample * sample;
                        entry.low += 0.5 * bass[channel].powi(2);
                        entry.high += 0.5 * (sample - smooth[channel]).powi(2);
                    }
                }
                let count = block.len().div_ceil(step) as f64;
                entry.power /= count;
                entry.low /= count;
                entry.high /= count;
                let level = entry.power.sqrt();
                entry.onset = (level - previous).max(0.0);
                previous = level;
                hops.push(entry);
            }
        }
        Self {
            hops,
            hop_seconds: hop as f64 / rate,
        }
    }

    pub fn sound(&self) -> Option<Sound> {
        self.measure(&self.hops)
    }

    fn measure(&self, hops: &[Hop]) -> Option<Sound> {
        let seconds = hops.len() as f64 * self.hop_seconds;
        if seconds < 0.5 {
            return None;
        }
        let power = hops.iter().map(|hop| hop.power).sum::<f64>();
        let rms = (power / hops.len() as f64).sqrt();
        if rms < 0.002 {
            return None;
        }
        let bass = (hops.iter().map(|hop| hop.low).sum::<f64>() / power).sqrt();
        let brightness = (hops.iter().map(|hop| hop.high).sum::<f64>() / power).sqrt();
        let transients = hops.iter().map(|hop| hop.onset).sum::<f64>() / (hops.len() as f64 * rms);
        let sparse = hops
            .iter()
            .filter(|hop| hop.power.sqrt() < rms * 0.45)
            .count() as f64
            / hops.len() as f64;
        let mean = hops.iter().map(|hop| hop.onset).sum::<f64>() / hops.len() as f64;
        let variance = hops
            .iter()
            .map(|hop| (hop.onset - mean).powi(2))
            .sum::<f64>();
        let min_lag = (0.28 / self.hop_seconds).round() as usize;
        let max_lag = ((0.95 / self.hop_seconds).round() as usize).min(hops.len() / 2);
        let mut pulse = 0.0_f64;
        let mut period = 0.5;
        if variance > 1e-8 {
            for lag in min_lag..=max_lag {
                let correlation = hops
                    .iter()
                    .zip(&hops[lag..])
                    .map(|(a, b)| (a.onset - mean) * (b.onset - mean))
                    .sum::<f64>()
                    / variance;
                if correlation > pulse {
                    pulse = correlation;
                    period = lag as f64 * self.hop_seconds;
                }
            }
        }
        Some(Sound {
            features: [
                bass,
                brightness,
                (transients * 4.0).min(1.0),
                sparse,
                pulse,
                period,
                ((20.0 * rms.log10() + 54.0) / 54.0).clamp(0.0, 1.0),
            ],
            seconds,
        })
    }

    pub fn best_match(
        &self,
        reference: Sound,
        priority: MatchBy,
    ) -> Option<((f64, f64), u8, [u8; 4])> {
        let size = ((reference.seconds.clamp(1.0, 16.0) / self.hop_seconds).round() as usize)
            .min(self.hops.len());
        if size == 0 {
            return None;
        }
        self.hops
            .windows(size)
            .step_by((size / 2).max(1))
            .enumerate()
            .take_while(|_| crate::background::check().is_ok())
            .filter_map(|(index, window)| {
                let sound = self.measure(window)?;
                let (distance, aspects) = sound.distance(reference, priority);
                let start = (index * (size / 2).max(1)) as f64 * self.hop_seconds;
                Some(((start, start + sound.seconds), distance, aspects))
            })
            .min_by(|(_, a, _), (_, b, _)| a.total_cmp(b))
            .map(|(range, distance, aspects)| {
                (range, similarity(distance), aspects.map(similarity))
            })
    }

    pub fn suggestions(&self, span: f64, duration: f64, emphasis: Emphasis) -> Vec<NamedSample> {
        let span = span.clamp(0.5, 32.0);
        let size = ((span / self.hop_seconds).round() as usize).min(self.hops.len());
        if size == 0 {
            return Vec::new();
        }
        let step = (size / 4).max(1);
        let windows: Vec<_> = self
            .hops
            .windows(size)
            .step_by(step)
            .enumerate()
            .take_while(|_| crate::background::check().is_ok())
            .filter_map(|(index, window)| self.measure(window).map(|sound| (index * step, sound)))
            .collect();
        let mut suggestions: Vec<NamedSample> = Vec::new();
        let modes = match emphasis {
            Emphasis::Balanced => [Emphasis::Rhythmic, Emphasis::Percussive, Emphasis::Sparse],
            mode => [mode; 3],
        };
        for mode in modes {
            let score = |sound: &Sound| match mode {
                Emphasis::Rhythmic => sound.features[4],
                Emphasis::Percussive => sound.features[2],
                Emphasis::Sparse => sound.features[3],
                Emphasis::Tonal => 1.0 - 0.6 * sound.features[2] - 0.4 * sound.features[3],
                Emphasis::Balanced => 0.0,
            };
            let best = windows
                .iter()
                .filter(|(index, _)| {
                    let start = *index as f64 * self.hop_seconds;
                    suggestions
                        .iter()
                        .all(|sample| (sample.start - start).abs() >= span * 0.75)
                })
                .max_by(|(_, a), (_, b)| score(a).total_cmp(&score(b)));
            if let Some((index, sound)) = best {
                let start = *index as f64 * self.hop_seconds;
                suggestions.push(NamedSample {
                    source: 0,
                    name: format!("{} passage", mode.label()),
                    start,
                    end: (start + sound.seconds).min(duration),
                });
            }
        }
        suggestions
    }
}

impl Sound {
    pub fn distance(self, reference: Sound, priority: MatchBy) -> (f64, [f64; 4]) {
        let difference = std::array::from_fn::<_, 7, _>(|index| {
            (self.features[index] - reference.features[index]).powi(2)
        });
        let [bass, bright, transients, sparse, pulse, period, energy] = difference;
        let confidence = self.features[4].min(reference.features[4]);
        let aspects = [
            transients + 0.65 * pulse + 0.6 * confidence * period,
            1.6 * bright,
            0.45 * energy,
            1.4 * bass + 0.75 * sparse,
        ];
        let weights = match priority {
            MatchBy::Balanced => [1.0; 4],
            MatchBy::Rhythm => [4.0, 1.0, 1.0, 1.0],
            MatchBy::Brightness => [1.0, 4.0, 1.0, 1.0],
            MatchBy::Energy => [1.0, 1.0, 4.0, 1.0],
            MatchBy::Texture => [1.0, 1.0, 1.0, 4.0],
        };
        let distance = aspects.iter().zip(weights).map(|(d, w)| d * w).sum::<f64>();
        (distance, aspects)
    }
}

fn similarity(distance: f64) -> u8 {
    (100.0 * (-2.5 * distance).exp()).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_similar_passages_in_stereo_and_suggests_separate_cuts() {
        let rate = 8_000;
        let tone = |index: usize, frequency: f64| {
            let sample = (std::f64::consts::TAU * frequency * index as f64 / f64::from(rate)).sin()
                as f32
                * 0.3;
            [sample, -sample]
        };
        let clip = Clip::new(
            rate,
            (0..rate as usize * 12)
                .map(|index| {
                    tone(
                        index,
                        if index < rate as usize * 6 {
                            70.0
                        } else {
                            2_400.0
                        },
                    )
                })
                .collect(),
        );
        let reference = AudioProfile::new(&clip, (0.0, 2.0)).sound().unwrap();
        let (range, score, aspects) = AudioProfile::new(&clip, (0.0, 12.0))
            .best_match(reference, MatchBy::Balanced)
            .unwrap();
        assert!(range.1 <= 6.0 && score >= 98);
        assert!(aspects.iter().all(|&value| value >= 98));
        let (_, bright_score, _) = AudioProfile::new(&clip, (6.0, 12.0))
            .best_match(reference, MatchBy::Balanced)
            .unwrap();
        assert!(bright_score + 30 < score);
        let suggestions =
            AudioProfile::new(&clip, (0.0, 12.0)).suggestions(4.0, 12.0, Emphasis::Balanced);
        assert!(suggestions.len() >= 2);
        for (index, cut) in suggestions.iter().enumerate() {
            assert!(cut.start >= 0.0 && cut.end <= 12.0 && cut.end > cut.start);
            assert!(
                suggestions[..index]
                    .iter()
                    .all(|other| (cut.start - other.start).abs() >= 3.0)
            );
        }
    }
}
