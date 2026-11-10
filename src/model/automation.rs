use super::{MAX_SONG_BARS, TICKS_PER_BAR};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AutomationTarget {
    MasterVolume,
    SampleVolume,
    DrumVolume,
    LowPass,
    Delay,
    Reverb,
}

impl AutomationTarget {
    pub const ALL: [Self; 6] = [
        Self::MasterVolume,
        Self::SampleVolume,
        Self::DrumVolume,
        Self::LowPass,
        Self::Delay,
        Self::Reverb,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MasterVolume => "Master volume",
            Self::SampleVolume => "Sample volume",
            Self::DrumVolume => "Drum volume",
            Self::LowPass => "Mix low-pass",
            Self::Delay => "Mix delay",
            Self::Reverb => "Mix reverb",
        }
    }

    pub fn baseline(self, sample_level: f32, drum_level: f32) -> f32 {
        match self {
            Self::MasterVolume | Self::LowPass => 1.0,
            Self::SampleVolume => sample_level,
            Self::DrumVolume => drum_level,
            Self::Delay | Self::Reverb => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    #[default]
    Linear,
    Smooth,
    Hold,
}

impl Curve {
    pub const ALL: [Self; 3] = [Self::Linear, Self::Smooth, Self::Hold];

    pub fn label(self) -> &'static str {
        match self { Self::Linear => "Linear", Self::Smooth => "Smooth", Self::Hold => "Hold" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationPoint {
    pub tick: u32,
    pub percent: u8,
    #[serde(default)]
    pub curve: Curve,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Automation(BTreeMap<AutomationTarget, Vec<AutomationPoint>>);

impl Automation {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn has_mix_fx(&self) -> bool {
        [
            AutomationTarget::LowPass,
            AutomationTarget::Delay,
            AutomationTarget::Reverb,
        ]
        .iter()
        .any(|target| self.0.contains_key(target))
    }

    pub fn points(&self, target: AutomationTarget) -> &[AutomationPoint] {
        self.0.get(&target).map_or(&[], Vec::as_slice)
    }

    pub fn set_points(&mut self, target: AutomationTarget, points: Vec<AutomationPoint>) {
        if points.is_empty() {
            self.0.remove(&target);
        } else {
            self.0.insert(target, points);
        }
    }

    pub fn value(&self, target: AutomationTarget, tick: f64, baseline: f32) -> f32 {
        Self::interpolate(self.points(target), tick, baseline)
    }

    pub fn interpolate(points: &[AutomationPoint], tick: f64, baseline: f32) -> f32 {
        if points.is_empty() {
            return baseline;
        }
        let next = points.partition_point(|point| f64::from(point.tick) <= tick);
        let percent = match (
            next.checked_sub(1).map(|index| points[index]),
            points.get(next),
        ) {
            (Some(left), Some(right)) => {
                let position =
                    ((tick - f64::from(left.tick)) / f64::from(right.tick - left.tick)) as f32;
                let position = match left.curve {
                    Curve::Linear => position,
                    Curve::Smooth => position * position * (3.0 - 2.0 * position),
                    Curve::Hold => 0.0,
                };
                f32::from(left.percent)
                    + position * (f32::from(right.percent) - f32::from(left.percent))
            }
            (Some(point), None) => f32::from(point.percent),
            (None, Some(point)) => f32::from(point.percent),
            (None, None) => unreachable!(),
        };
        percent * 0.01
    }

    pub(super) fn validate(&mut self) {
        self.0.retain(|_, points| {
            points.retain_mut(|point| {
                point.percent = point.percent.min(100);
                point.tick <= MAX_SONG_BARS as u32 * TICKS_PER_BAR
            });
            points.sort_by_key(|point| point.tick);
            points.dedup_by_key(|point| point.tick);
            !points.is_empty()
        });
    }
}
