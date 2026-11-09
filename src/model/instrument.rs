use super::{PIANO_HIGH, PIANO_LOW, STEPS, SampleControls};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_LAYERS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glide {
    pub mono: bool,
    pub milliseconds: u16,
    pub retrigger: bool,
}

impl Glide {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HitSound {
    pub pitch_cents: i16,
    pub cutoff_hz: Option<u16>,
    pub start_percent: u8,
}

impl HitSound {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&mut self) {
        self.pitch_cents = self.pitch_cents.clamp(-2400, 2400);
        self.cutoff_hz = self.cutoff_hz.map(|hz| hz.clamp(20, 20_000));
        self.start_percent = self.start_percent.min(99);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VelocityRange {
    pub min: u8,
    pub max: u8,
}

impl Default for VelocityRange {
    fn default() -> Self {
        Self { min: 1, max: 127 }
    }
}

impl VelocityRange {
    pub fn contains(self, velocity: u8) -> bool {
        (self.min..=self.max).contains(&velocity)
    }

    fn validate(&mut self) {
        self.min = self.min.clamp(1, 127);
        self.max = self.max.clamp(self.min, 127);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub source: usize,
    pub start: f64,
    pub end: f64,
    pub controls: SampleControls,
    pub velocity: VelocityRange,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layers {
    pub base: VelocityRange,
    pub extra: [Option<Layer>; MAX_LAYERS - 1],
}

impl Layers {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn preview_velocity(self) -> u8 {
        self.extra
            .iter()
            .flatten()
            .map(|layer| layer.velocity.max)
            .fold(self.base.max, u8::max)
    }

    fn validate(&mut self, duration: &impl Fn(usize) -> f64, max_source: usize) {
        self.base.validate();
        for entry in &mut self.extra {
            *entry = entry
                .filter(|layer| {
                    layer.source <= max_source && layer.start.is_finite() && layer.end.is_finite()
                })
                .and_then(|mut layer| {
                    let length = duration(layer.source);
                    layer.start = layer.start.clamp(0.0, length);
                    layer.end = layer.end.clamp(layer.start, length);
                    layer.controls.validate();
                    layer.velocity.validate();
                    (layer.end > layer.start).then_some(layer)
                });
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Instruments {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pads: BTreeMap<usize, Layers>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<u8, Layers>,
    #[serde(skip_serializing_if = "Layers::is_default")]
    pub chromatic: Layers,
    #[serde(skip_serializing_if = "Glide::is_default")]
    pub glide: Glide,
}

impl Instruments {
    pub fn is_default(&self) -> bool {
        self.pads.is_empty()
            && self.keys.is_empty()
            && self.chromatic.is_default()
            && self.glide.is_default()
    }

    pub(super) fn validate(&mut self, duration: &impl Fn(usize) -> f64, max_source: usize) {
        self.glide.milliseconds = self.glide.milliseconds.min(2_000);
        self.pads.retain(|&pad, layers| {
            layers.validate(duration, max_source);
            pad < STEPS && !layers.is_default()
        });
        self.keys.retain(|&key, layers| {
            layers.validate(duration, max_source);
            (PIANO_LOW..=PIANO_HIGH).contains(&key)
        });
        self.chromatic.validate(duration, max_source);
    }

    pub fn remap_sources(&mut self, index: impl Fn(usize) -> usize) {
        for layers in self
            .pads
            .values_mut()
            .chain(self.keys.values_mut())
            .chain(std::iter::once(&mut self.chromatic))
        {
            for layer in layers.extra.iter_mut().flatten() {
                layer.source = index(layer.source);
            }
        }
    }
}
