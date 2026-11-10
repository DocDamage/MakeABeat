use super::instrument::HitSound;
use super::{ChopInput, DRUMS, PlayedChop, STEPS, StepRule, Studio, TICKS_PER_BAR, drum_velocity, groove::HitFeel, swung_step};
use crate::{audio::PadMode, midi_out::DRUM_NOTES};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteId(pub u64);

impl NoteId {
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy)]
pub enum Trigger {
    Pad(u8),
    Key(u8),
    Drum(u8),
}

impl Trigger {
    pub fn order(self) -> u8 {
        match self {
            Self::Pad(_) => 0,
            Self::Key(_) => 1,
            Self::Drum(drum) => drum + 2,
        }
    }
}

/// A scheduled hit on the absolute playback timeline. Recorded positions
/// already contain the performed timing, so only grid cells resolve swing.
#[derive(Clone)]
pub struct ResolvedEvent {
    pub id: NoteId,
    pub trigger: Trigger,
    pub tick: u64,
    pub velocity: u8,
    pub mode: PadMode,
    pub gate_ticks: Option<u32>,
    pub midi_ticks: u64,
    pub grid: bool,
    midi_velocity: u8,
    pub sound: HitSound,
    pub expression: Option<std::sync::Arc<super::melody::Expression>>,
}

impl ResolvedEvent {
    pub fn recorded(note: &PlayedChop, tick: u64) -> Self {
        Self {
            id: note.id,
            trigger: match note.input {
                ChopInput::Pad(pad) => Trigger::Pad(pad),
                ChopInput::Key(key) => Trigger::Key(key),
            },
            tick,
            velocity: note.velocity,
            mode: note.mode,
            gate_ticks: note.length,
            midi_ticks: u64::from(note.length.unwrap_or(55)),
            grid: false,
            midi_velocity: note.velocity,
            sound: note.sound,
            expression: note.expression.clone(),
        }
    }

    pub fn midi(&self) -> (usize, u8, u8) {
        match self.trigger {
            // The original grid plays pad audio at full velocity and sends MIDI at 112.
            Trigger::Pad(pad) => (0, 24 + pad, self.midi_velocity),
            Trigger::Key(key) => (1, key, self.velocity),
            Trigger::Drum(drum) => (2, DRUM_NOTES[usize::from(drum)], self.velocity),
        }
    }

    pub fn assigned(&self, studio: &Studio) -> bool {
        match self.trigger {
            Trigger::Pad(pad) => studio.input_is_assigned(ChopInput::Pad(pad)),
            Trigger::Key(key) => self.expression.is_some() || studio.input_is_assigned(ChopInput::Key(key)),
            Trigger::Drum(_) => true,
        }
    }
}

pub const LANES: usize = DRUMS.len() + 2;

#[derive(Clone, Copy)]
pub struct Cell {
    id: NoteId,
    trigger: Trigger,
    velocity: u8,
    rule: StepRule,
    sound: HitSound,
    feel: super::groove::HitFeel,
}

impl Cell {
    pub fn column(
        drums: &[[u8; STEPS]; DRUMS.len()],
        pad: Option<usize>,
        key: Option<u8>,
        metadata: StepMetadata<'_>,
        step: usize,
    ) -> [Option<Self>; LANES] {
        std::array::from_fn(|row| {
            let (trigger, velocity) = match row {
                0 => (Trigger::Pad(pad? as u8), 127),
                1 => (Trigger::Key(key?), 112),
                _ => {
                    let drum = row - 2;
                    let velocity = drum_velocity(drums[drum][step]);
                    if velocity == 0 {
                        return None;
                    }
                    (Trigger::Drum(drum as u8), velocity)
                }
            };
            let key = (row * STEPS + step) as u8;
            Some(Self {
                id: metadata.ids.get(&key).copied().unwrap_or_default(),
                trigger,
                velocity,
                rule: metadata.rules.get(&key).copied().unwrap_or_default(),
                sound: metadata.sounds.get(&key).copied().unwrap_or_default(),
                feel: metadata.feel.get(&key).copied().unwrap_or_default(),
            })
        })
    }

    pub fn plays_early(self, lane_offset: i8) -> bool {
        i16::from(lane_offset) + i16::from(self.feel.offset) < 0
    }

    pub fn resolve(
        self,
        timing: StepTiming<'_>,
        row: usize,
        step: u64,
        cycle: u64,
        fill: bool,
    ) -> impl Iterator<Item = ResolvedEvent> + use<> {
        let velocity = self.feel.velocity.unwrap_or(self.velocity);
        let midi_velocity = if row == 0 {
            (u16::from(velocity) * 112).div_ceil(127) as u8
        } else {
            velocity
        };
        timing
            .hit_ticks(self.rule, row, step, cycle, fill, self.feel)
            .map(move |(tick, midi_ticks)| ResolvedEvent {
                id: self.id,
                trigger: self.trigger,
                tick,
                velocity,
                midi_velocity,
                mode: PadMode::Slice,
                gate_ticks: None,
                midi_ticks,
                grid: true,
                sound: self.sound,
                expression: None,
            })
    }
}

pub struct StepMetadata<'a> {
    pub rules: &'a std::collections::BTreeMap<u8, StepRule>,
    pub sounds: &'a std::collections::BTreeMap<u8, HitSound>,
    pub ids: &'a super::StepIds,
    pub feel: &'a super::groove::HitFeels,
}

pub(crate) struct StepTiming<'a> {
    pub swing: f32,
    pub lane_offsets: &'a [i8; LANES],
}

impl StepTiming<'_> {
    pub(crate) fn hit_ticks(
        self, rule: StepRule, row: usize, absolute: u64, cycle: u64, fill: bool, feel: HitFeel,
    ) -> impl Iterator<Item = (u64, u64)> + use<> {
        let start = swung_step(absolute, self.swing);
        let next = swung_step(absolute + 1, self.swing);
        let bar_ticks = u64::from(TICKS_PER_BAR);
        let bar_start = (start / bar_ticks) * bar_ticks;
        let bar_end = bar_start + bar_ticks;
        let repeats = u64::from(rule.repeats.clamp(1, 4));
        let last_hit = start + (next - start) * (repeats - 1) / repeats;
        let offset = (i64::from(self.lane_offsets[row]) + i64::from(feel.offset)).clamp(
            -((start - bar_start) as i64), (bar_end - last_hit - 1) as i64,
        );
        rule.hits(start, next, row, cycle, fill).map(move |(tick, length)| {
            (tick.saturating_add_signed(offset), length)
        })
    }
}
