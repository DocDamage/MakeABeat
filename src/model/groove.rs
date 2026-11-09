use super::{
    ChopInput, DRUMS, MAX_LANE_OFFSET, MAX_SAMPLE_NAME, STEPS, Sequence, TICKS_PER_BAR,
    TICKS_PER_STEP, drum_velocity, encode_drum_velocity, events::LANES,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_OFFSET: i8 = 30;
pub const EARLY_TICKS: u64 = (MAX_LANE_OFFSET + MAX_OFFSET) as u64;
pub const MAX_BARS: usize = 8;
pub const MAX_TEMPLATES: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HitFeel {
    pub offset: i8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
}

pub type HitFeels = BTreeMap<u8, HitFeel>;

pub fn set_feel(feel: &mut HitFeels, key: u8, value: HitFeel) {
    if value == HitFeel::default() {
        feel.remove(&key);
    } else {
        feel.insert(key, value);
    }
}

pub fn validate_feel(
    feel: &mut HitFeels,
    pads: &[Option<usize>; STEPS],
    keys: &[Option<u8>; STEPS],
    drums: &[[u8; STEPS]; DRUMS.len()],
) {
    feel.retain(|&key, value| {
        let (lane, step) = (usize::from(key) / STEPS, usize::from(key) % STEPS);
        value.offset = value.offset.clamp(-MAX_OFFSET, MAX_OFFSET);
        value.velocity = value.velocity.filter(|_| lane < 2).map(|v| v.clamp(1, 127));
        *value != HitFeel::default()
            && match lane {
                0 => pads[step].is_some(),
                1 => keys[step].is_some(),
                2..LANES => drums[lane - 2][step] > 0,
                _ => false,
            }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub offset: i8,
    pub accent: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub title: String,
    pub fingerprint: String,
    pub range: (f64, f64),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub bars: usize,
    pub slots: Vec<Slot>,
    pub source: Provenance,
}

impl Template {
    pub fn validate(&mut self) -> bool {
        if !(1..=MAX_BARS).contains(&self.bars) || self.slots.len() != self.bars * STEPS {
            return false;
        }
        self.name = self.name.trim().chars().take(MAX_SAMPLE_NAME).collect();
        self.source.title = self.source.title.chars().take(256).collect();
        self.source
            .fingerprint
            .truncate(self.source.fingerprint.floor_char_boundary(64));
        for slot in &mut self.slots {
            slot.offset = slot.offset.clamp(-MAX_OFFSET, MAX_OFFSET);
            slot.accent = if slot.accent.is_finite() {
                slot.accent.clamp(0.0, 4.0)
            } else {
                1.0
            };
        }
        !self.name.is_empty()
            && self.source.range.0.is_finite()
            && self.source.range.1.is_finite()
            && self.source.range.0 >= 0.0
            && self.source.range.0 < self.source.range.1
    }
}

#[derive(Clone)]
pub struct Transfer {
    pub timing: u8,
    pub velocity: u8,
    pub lanes: [bool; LANES],
    pub recorded: bool,
}

impl Default for Transfer {
    fn default() -> Self {
        Self {
            timing: 100,
            velocity: 0,
            lanes: std::array::from_fn(|lane| lane >= 2),
            recorded: false,
        }
    }
}

impl Transfer {
    pub fn bake(&self, mut sequence: Sequence, template: &Template) -> Sequence {
        if (self.timing == 0 && self.velocity == 0) || template.slots.is_empty() {
            return sequence;
        }
        let timing = f32::from(self.timing) / 100.0;
        let velocity = f32::from(self.velocity) / 100.0;
        let accent = |original: u8, weight: f32| {
            (f32::from(original) * (1.0 + velocity * (weight - 1.0)))
                .round()
                .clamp(1.0, 127.0) as u8
        };
        for (bar_index, bar) in sequence.bars.iter_mut().enumerate() {
            for (lane, selected) in self.lanes.iter().enumerate() {
                if !selected {
                    continue;
                }
                for step in 0..STEPS {
                    let base = match lane {
                        0 if bar.pads[step].is_some() => 127,
                        1 if bar.keys[step].is_some() => 112,
                        2..LANES if bar.drums[lane - 2][step] > 0 => {
                            drum_velocity(bar.drums[lane - 2][step])
                        }
                        _ => continue,
                    };
                    let key = (lane * STEPS + step) as u8;
                    let slot = template.slots[(bar_index * STEPS + step) % template.slots.len()];
                    let mut feel = bar.feel.get(&key).copied().unwrap_or_default();
                    feel.offset = (f32::from(feel.offset)
                        + timing * f32::from(slot.offset - feel.offset))
                    .round() as i8;
                    if self.velocity > 0 {
                        let adjusted = accent(feel.velocity.unwrap_or(base), slot.accent);
                        if lane < 2 {
                            feel.velocity = (adjusted != base).then_some(adjusted);
                        } else if adjusted != base {
                            bar.drums[lane - 2][step] = encode_drum_velocity(adjusted);
                        }
                    }
                    set_feel(&mut bar.feel, key, feel);
                }
            }
        }
        if self.recorded {
            for note in &mut sequence.performance {
                let lane = usize::from(matches!(note.input, ChopInput::Key(_)));
                if !self.lanes[lane] {
                    continue;
                }
                let bar = note.tick / TICKS_PER_BAR;
                let within = note.tick % TICKS_PER_BAR;
                let step = ((within + TICKS_PER_STEP / 2) / TICKS_PER_STEP).min(STEPS as u32 - 1);
                let slot =
                    template.slots[(bar as usize * STEPS + step as usize) % template.slots.len()];
                let shift = (f32::from(slot.offset) * timing).round() as i32;
                note.tick = bar * TICKS_PER_BAR
                    + within.saturating_add_signed(shift).min(TICKS_PER_BAR - 1);
                if self.velocity > 0 {
                    note.velocity = accent(note.velocity, slot.accent);
                }
            }
            sequence
                .performance
                .sort_by_key(|note| (note.tick, note.input));
        }
        sequence
    }
}
