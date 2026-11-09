use super::{Clip, Frame, HitSound, SampleAssignment, SampleStack};
use crate::model::instrument::Glide;

#[derive(Clone, Copy)]
pub(super) struct Note {
    pub source: u64,
    pub key: u8,
    pub velocity: u8,
    pub bend: f64,
    pub gate: bool,
    pub release_at: Option<u64>,
    pub assignment: SampleAssignment,
    pub sound: HitSound,
    pub speed: f64,
}

impl Note {
    fn identity(self) -> (u64, u8) {
        (self.source, self.key)
    }

    fn pitch(self) -> f64 {
        f64::from(self.key) + f64::from(self.sound.pitch_cents) / 100.0
    }
}

struct Held {
    note: Note,
    pressed: bool,
}

struct Active {
    note: Note,
    voices: SampleStack,
    pitch: Pitch,
}

struct Pitch {
    current: f64,
    ratio: f64,
    increment: f64,
    ratio_step: f64,
    frames: u32,
}

impl Pitch {
    fn new(from: f64, to: f64, frames: u32) -> Self {
        let increment = (to - from) / f64::from(frames.max(1));
        Self {
            current: if frames == 0 { to } else { from },
            ratio: if frames == 0 {
                1.0
            } else {
                2.0_f64.powf((from - to) / 12.0)
            },
            increment,
            ratio_step: 2.0_f64.powf(increment / 12.0),
            frames,
        }
    }

    fn advance(&mut self) {
        if self.frames > 0 {
            self.frames -= 1;
            self.current += self.increment;
            self.ratio = if self.frames == 0 {
                1.0
            } else {
                self.ratio * self.ratio_step
            };
        }
    }
}

pub(super) struct Mono {
    held: Vec<Held>,
    active: Option<Active>,
    next_release: Option<u64>,
}

impl Mono {
    pub fn new() -> Self {
        Self {
            held: Vec::with_capacity(128),
            active: None,
            next_release: None,
        }
    }

    pub fn clear(&mut self) {
        self.held.clear();
        self.active = None;
        self.next_release = None;
    }

    pub fn on(&mut self, note: Note, voices: SampleStack, settings: Glide, rate: f64) {
        let overlap = !self.held.is_empty();
        self.held
            .retain(|held| held.note.identity() != note.identity());
        if self.held.len() == self.held.capacity() {
            self.held.remove(0);
        }
        self.held.push(Held {
            note,
            pressed: true,
        });
        self.update_next_release();
        self.switch(note, voices, settings, rate, overlap);
    }

    pub fn off(&mut self, source: u64, key: u8, sustained: bool, settings: Glide, rate: f64) {
        if sustained {
            if let Some(held) = self
                .held
                .iter_mut()
                .find(|held| held.note.identity() == (source, key))
            {
                held.pressed = false;
            }
            return;
        }
        self.remove(|held| held.note.identity() == (source, key), settings, rate);
    }

    pub fn pedal_up(&mut self, source: u64, settings: Glide, rate: f64) {
        self.remove(
            |held| !held.pressed && held.note.source == source,
            settings,
            rate,
        );
    }

    pub fn silence(&mut self, source: Option<u64>, settings: Glide, rate: f64) {
        self.remove(
            |held| source.is_none_or(|id| id == held.note.source),
            settings,
            rate,
        );
        if let Some(active) = &mut self.active
            && source.is_none_or(|id| id == active.note.source)
        {
            active.voices.releasing(true);
        }
    }

    pub fn bend(&mut self, source: u64, bend: f64) {
        for held in self
            .held
            .iter_mut()
            .filter(|held| held.note.source == source)
        {
            held.note.bend = bend;
        }
        if let Some(active) = &mut self.active
            && active.note.source == source
        {
            active.voices.bend(bend);
        }
    }

    fn remove(&mut self, mut remove: impl FnMut(&Held) -> bool, settings: Glide, rate: f64) {
        let before = self.held.len();
        self.held.retain(|held| !remove(held));
        if before == self.held.len() {
            return;
        }
        self.update_next_release();
        if let Some(held) = self.held.last() {
            let note = held.note;
            if self
                .active
                .as_ref()
                .is_none_or(|active| active.note.identity() != note.identity())
            {
                let voices = SampleStack::new(
                    note.assignment,
                    note.speed / rate,
                    note.velocity,
                    note.bend,
                    note.sound,
                    rate,
                );
                self.switch(note, voices, settings, rate, true);
            }
        } else if let Some(active) = &mut self.active
            && active.note.gate
        {
            active.voices.releasing(true);
        }
    }

    fn update_next_release(&mut self) {
        self.next_release = self
            .held
            .iter()
            .filter_map(|held| held.note.release_at)
            .min();
    }

    fn switch(
        &mut self,
        note: Note,
        mut voices: SampleStack,
        settings: Glide,
        rate: f64,
        overlap: bool,
    ) {
        let to = note.pitch();
        let pitch = match self
            .active
            .as_ref()
            .filter(|old| overlap && old.voices.started)
        {
            Some(old) => {
                if !settings.retrigger && old.note.sound.start_percent == note.sound.start_percent {
                    voices.continue_from(&old.voices);
                }
                let frames = (rate * f64::from(settings.milliseconds) / 1_000.0).round() as u32;
                Pitch::new(old.pitch.current, to, frames)
            }
            None => Pitch::new(to, to, 0),
        };
        self.active = Some(Active {
            note,
            voices,
            pitch,
        });
    }

    pub fn frame(&mut self, clip: &Clip, tick: u64, settings: Glide, rate: f64) -> Option<Frame> {
        self.advance(tick, settings, rate);
        let active = self.active.as_mut()?;
        let frame = active.voices.frame_at(clip, rate, active.pitch.ratio);
        active.pitch.advance();
        frame
    }

    pub fn advance(&mut self, tick: u64, settings: Glide, rate: f64) {
        if self.next_release.is_some_and(|end| tick >= end) {
            self.remove(
                |held| held.note.release_at.is_some_and(|end| tick >= end),
                settings,
                rate,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_reaches_target_in_the_same_time_at_different_sample_rates() {
        for rate in [44_100, 48_000, 96_000] {
            let mut pitch = Pitch::new(36.0, 48.0, rate / 10);
            for _ in 0..rate / 20 {
                pitch.advance();
            }
            assert!((pitch.current - 42.0).abs() < 1e-8);
            for _ in 0..rate / 20 {
                pitch.advance();
            }
            assert!((pitch.current - 48.0).abs() < 1e-8);
            assert_eq!(pitch.ratio, 1.0);
        }
    }

    #[test]
    fn sustained_note_resumes_when_the_newer_note_is_released() {
        let mut mono = Mono::new();
        let settings = Glide {
            mono: true,
            milliseconds: 100,
            retrigger: false,
        };
        let rate = 1_000.0;
        let assignment = SampleAssignment {
            source: 0,
            range: (0.0, 1.0),
            controls: Default::default(),
            layers: Default::default(),
            transpose: 0.0,
        };
        let note = |key| Note {
            source: 1,
            key,
            velocity: 127,
            bend: 0.0,
            gate: true,
            release_at: None,
            assignment,
            sound: HitSound::default(),
            speed: 1.0,
        };
        let voices =
            || SampleStack::new(assignment, 1.0 / rate, 127, 0.0, HitSound::default(), rate);
        let clip = Clip::new(1_000, vec![[0.4; 2]; 1_000]);
        mono.on(note(36), voices(), settings, rate);
        for _ in 0..10 {
            mono.frame(&clip, 0, settings, rate);
        }
        mono.off(1, 36, true, settings, rate);
        mono.on(note(48), voices(), settings, rate);
        let active = mono.active.as_ref().unwrap();
        assert_eq!(active.note.key, 48);
        assert!(active.voices.head().unwrap().voice.position > 0.009);
        assert_eq!(active.pitch.ratio, 0.5);
        mono.off(1, 48, false, settings, rate);
        assert_eq!(mono.active.as_ref().unwrap().note.key, 36);
        mono.pedal_up(1, settings, rate);
        assert!(
            mono.active
                .as_ref()
                .unwrap()
                .voices
                .head()
                .unwrap()
                .voice
                .releasing
        );
    }
}
