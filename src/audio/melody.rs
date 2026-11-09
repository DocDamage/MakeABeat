use super::{Clip, Frame, SampleAssignment, SampleStack};
use crate::model::{
    SampleControls,
    events::ResolvedEvent,
    melody::{Expression, Instrument, cents_at},
};
use std::sync::Arc;

pub(super) struct Voice {
    start: u64,
    duration: u64,
    expression: Arc<Expression>,
    sound: Sound,
    previous_bend: f64,
}

// Inline sampler storage avoids allocating when the mixer starts a note.
#[allow(clippy::large_enum_variant)]
enum Sound {
    Sine {
        phase: f64,
        frequency: f64,
        gain: f32,
        age: u32,
        released: u32,
    },
    Sample(SampleStack),
}

impl Voice {
    pub fn new(
        event: &ResolvedEvent,
        key: u8,
        expression: Arc<Expression>,
        clip: Option<&Clip>,
        rate: f64,
    ) -> Option<Self> {
        let bend = cents_at(&expression.bends, 0.0) / 100.0;
        let duration = event.midi_ticks.max(1);
        let sound = match expression.instrument {
            Instrument::Sine => Sound::Sine {
                phase: 0.0,
                frequency: 440.0
                    * 2.0_f64.powf(
                        (f64::from(key) - 69.0 + f64::from(event.sound.pitch_cents) / 100.0) / 12.0,
                    ),
                gain: 0.28 * (f32::from(event.velocity) / 127.0).powf(1.4),
                age: 0,
                released: 0,
            },
            Instrument::Sample {
                source,
                start,
                end,
                root,
            } => {
                let clip = clip?.source(source)?;
                let source_rate = f64::from(clip.rate);
                let end = end.min(clip.frames.len() as u32);
                if start >= end {
                    return None;
                }
                let range = (f64::from(start) / source_rate, f64::from(end) / source_rate);
                let transpose = f32::from(key) - f32::from(root);
                let controls = SampleControls {
                    tune: transpose,
                    ..SampleControls::default()
                };
                let assignment = SampleAssignment {
                    source,
                    range,
                    controls,
                    layers: Default::default(),
                    transpose,
                };
                let mut voices = SampleStack::new(
                    assignment,
                    1.0 / rate,
                    event.velocity,
                    bend,
                    event.sound,
                    rate,
                );
                voices.release_at = Some(event.tick + duration);
                Sound::Sample(voices)
            }
        };
        Some(Self {
            start: event.tick,
            duration,
            expression,
            sound,
            previous_bend: bend,
        })
    }

    pub fn frame(&mut self, clip: Option<&Clip>, rate: f64, tick: f64) -> Option<Frame> {
        let elapsed = (tick - self.start as f64).clamp(0.0, self.duration as f64);
        let bend = cents_at(&self.expression.bends, elapsed) / 100.0;
        let sample = match &mut self.sound {
            Sound::Sample(voices) => {
                if self.previous_bend != bend {
                    voices.bend(bend);
                }
                voices.advance_gate(tick as u64);
                voices.frame(clip?, rate)
            }
            Sound::Sine {
                phase,
                frequency,
                gain,
                age,
                released,
            } => {
                let attack = (f64::from(*age) / (0.004 * rate)).min(1.0) as f32;
                let release = 1.0 - (f64::from(*released) / (0.012 * rate)).min(1.0) as f32;
                if release <= 0.0 {
                    return None;
                }
                let value = phase.sin() as f32 * *gain * attack * release;
                *phase = (*phase
                    + std::f64::consts::TAU * *frequency * 2.0_f64.powf(bend / 12.0) / rate)
                    % std::f64::consts::TAU;
                *age += 1;
                if elapsed >= self.duration as f64 {
                    *released += 1;
                }
                Some([value; 2])
            }
        };
        self.previous_bend = bend;
        sample
    }
}
