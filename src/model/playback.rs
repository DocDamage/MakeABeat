use super::{
    ChokeGroups, ChopInput, ChromaticSample, DRUMS, DelayDivision, DrumMix, KeyChop, MAX_BARS,
    MIN_LOOP_SECONDS, PlayedChop, STEPS, SampleControls, Source, StepIds, StepRule, Studio,
    TICKS_PER_BAR,
    events::StepTiming,
    grid_edit::LANES,
    groove::HitFeels,
    instrument::{HitSound, Instruments, Layers},
};
use std::collections::BTreeMap;

// Keep the playback projection and the allocation-free change check in step.
// Record titles and other project metadata have no representation here.
macro_rules! playback_fields {
    ($($field:ident: $ty:ty),+ $(,)?) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct PlaybackSettings {
            $(pub(crate) $field: $ty,)+
            sources: Vec<Source>,
        }

        impl From<&Studio> for PlaybackSettings {
            fn from(studio: &Studio) -> Self {
                Self {
                    $($field: Clone::clone(&studio.$field),)+
                    sources: studio.sources.iter().map(|record| record.source.clone()).collect(),
                }
            }
        }

        impl PlaybackSettings {
            pub(crate) fn matches(&self, studio: &Studio) -> bool {
                $(self.$field == studio.$field &&)+ self.same_sources(studio)
            }
        }
    };
}

playback_fields! {
    bpm: u16,
    bars: usize,
    kit: usize,
    pads: [Option<f64>; STEPS],
    pad_ends: [Option<f64>; STEPS],
    pad_sources: [usize; STEPS],
    drum_sources: [Option<usize>; DRUMS.len()],
    drum_mix: [DrumMix; DRUMS.len()],
    choke_groups: ChokeGroups,
    instruments: Instruments,
    key_sources: BTreeMap<u8, usize>,
    loop_source: usize,
    chromatic_enabled: bool,
    chromatic: Option<ChromaticSample>,
    pad_controls: [SampleControls; STEPS],
    key_chops: BTreeMap<u8, KeyChop>,
    key_controls: BTreeMap<u8, SampleControls>,
    loop_in: Option<f64>,
    loop_out: Option<f64>,
    looping: bool,
    speed: f64,
    chops: [[Option<usize>; STEPS]; MAX_BARS],
    key_steps: [[Option<u8>; STEPS]; MAX_BARS],
    performance: Vec<PlayedChop>,
    hits: [[[u8; STEPS]; DRUMS.len()]; MAX_BARS],
    step_rules: [BTreeMap<u8, StepRule>; MAX_BARS],
    step_sounds: [BTreeMap<u8, HitSound>; MAX_BARS],
    step_ids: [StepIds; MAX_BARS],
    step_feel: [HitFeels; MAX_BARS],
    drum_level: f32,
    sample_level: f32,
    swing: f32,
    lane_offsets: [i8; LANES],
    reverb: f32,
    delay: f32,
    delay_division: DelayDivision,
    drum_fx_bypass: bool,
    mix_fx_bypass: bool,
    filter: f32,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self::from(&Studio::default())
    }
}

impl PlaybackSettings {
    fn same_sources(&self, studio: &Studio) -> bool {
        self.sources
            .iter()
            .eq(studio.sources.iter().map(|record| &record.source))
    }

    pub(crate) fn capture_matches(&self, studio: &Studio) -> bool {
        self.instrument_view()
            .same_capture(studio.instrument_view())
            && self.choke_groups == studio.choke_groups
            && self.speed == studio.speed
            && self.same_sources(studio)
    }

    pub(crate) fn same_capture_instruments(&self, other: &Self) -> bool {
        self.instrument_view().same_capture(other.instrument_view())
            && self.choke_groups == other.choke_groups
            && self.speed == other.speed
            && self.sources == other.sources
    }

    pub(crate) fn active_loop(&self) -> Option<(f64, f64)> {
        loop_range(self.loop_in, self.loop_out).filter(|_| self.looping)
    }

    pub(crate) fn pattern_ticks(&self) -> u32 {
        self.bars as u32 * TICKS_PER_BAR
    }

    pub(crate) fn step_timing(&self) -> StepTiming<'_> {
        StepTiming {
            swing: self.swing,
            lane_offsets: &self.lane_offsets,
        }
    }

    pub(crate) fn has_chops(&self) -> bool {
        self.instrument_view().has_chops(
            &self.chops[..self.bars],
            &self.key_steps[..self.bars],
            &self.performance,
        )
    }
}

pub(super) fn loop_range(start: Option<f64>, end: Option<f64>) -> Option<(f64, f64)> {
    let (start, end) = (start?, end?);
    (end - start >= MIN_LOOP_SECONDS - 1e-9).then_some((start, end))
}

#[derive(Clone, Copy)]
pub(crate) struct InstrumentView<'a> {
    pub(crate) pads: &'a [Option<f64>; STEPS],
    pub(crate) pad_ends: &'a [Option<f64>; STEPS],
    pub(crate) pad_sources: &'a [usize; STEPS],
    pub(crate) pad_controls: &'a [SampleControls; STEPS],
    instruments: &'a Instruments,
    key_sources: &'a BTreeMap<u8, usize>,
    key_chops: &'a BTreeMap<u8, KeyChop>,
    key_controls: &'a BTreeMap<u8, SampleControls>,
    loop_source: usize,
    loop_out: Option<f64>,
    pub(crate) chromatic_enabled: bool,
    pub(crate) chromatic: Option<ChromaticSample>,
}

macro_rules! instrument_view {
    ($owner:ty) => {
        impl $owner {
            pub(crate) fn instrument_view(&self) -> InstrumentView<'_> {
                InstrumentView {
                    pads: &self.pads,
                    pad_ends: &self.pad_ends,
                    pad_sources: &self.pad_sources,
                    pad_controls: &self.pad_controls,
                    instruments: &self.instruments,
                    key_sources: &self.key_sources,
                    key_chops: &self.key_chops,
                    key_controls: &self.key_controls,
                    loop_source: self.loop_source,
                    loop_out: self.loop_out,
                    chromatic_enabled: self.chromatic_enabled,
                    chromatic: self.chromatic,
                }
            }
        }
    };
}

instrument_view!(Studio);
instrument_view!(PlaybackSettings);

impl InstrumentView<'_> {
    fn same_capture(self, other: Self) -> bool {
        self.pads == other.pads
            && self.pad_ends == other.pad_ends
            && self.pad_sources == other.pad_sources
            && self.pad_controls == other.pad_controls
            && self.instruments == other.instruments
            && self.key_sources == other.key_sources
            && self.key_chops == other.key_chops
            && self.key_controls == other.key_controls
            && self.chromatic_enabled == other.chromatic_enabled
            && self.chromatic == other.chromatic
    }

    pub(crate) fn input_is_assigned(self, input: ChopInput) -> bool {
        match input {
            ChopInput::Pad(pad) => self.pads.get(usize::from(pad)).is_some_and(Option::is_some),
            ChopInput::Key(note) => self.key_range(note, f64::INFINITY).is_some(),
        }
    }

    pub(super) fn has_chops(
        self,
        pads: &[[Option<usize>; STEPS]],
        keys: &[[Option<u8>; STEPS]],
        notes: &[PlayedChop],
    ) -> bool {
        let ticks = pads.len() as u32 * TICKS_PER_BAR;
        pads.iter()
            .flatten()
            .flatten()
            .any(|&pad| self.pads[pad].is_some())
            || keys
                .iter()
                .flatten()
                .flatten()
                .any(|&note| self.key_range(note, f64::INFINITY).is_some())
            || notes
                .iter()
                .any(|event| event.tick < ticks && self.input_is_assigned(event.input))
    }

    pub(crate) fn slice(self, pad: usize, duration: f64) -> Option<(f64, f64)> {
        let start = self.pads[pad]?;
        let end = self.pad_ends[pad]
            .map(|end| end.min(duration))
            .unwrap_or_else(|| {
                self.pads
                    .iter()
                    .zip(*self.pad_sources)
                    .filter_map(|(&time, source)| {
                        (source == self.pad_sources[pad]).then_some(time).flatten()
                    })
                    .chain(
                        self.loop_out
                            .filter(|_| self.loop_source == self.pad_sources[pad]),
                    )
                    .filter(|time| *time > start + 0.001)
                    .fold(duration, f64::min)
            });
        (end > start).then_some((start, end))
    }

    pub(crate) fn key_source(self, note: u8) -> usize {
        if self.chromatic_enabled {
            return self.chromatic.map_or(0, |sample| sample.source);
        }
        if self.key_chops.contains_key(&note) {
            self.key_sources.get(&note).copied().unwrap_or(0)
        } else {
            Studio::pad_for_key(note).map_or(0, |pad| self.pad_sources[pad])
        }
    }

    pub(crate) fn key_layers(self, note: u8) -> Layers {
        if self.chromatic_enabled {
            return self.instruments.chromatic;
        }
        self.instruments
            .keys
            .get(&note)
            .copied()
            .unwrap_or_else(|| {
                if self.key_chops.contains_key(&note) {
                    Layers::default()
                } else {
                    Studio::pad_for_key(note)
                        .map_or_else(Layers::default, |pad| self.pad_layers(pad))
                }
            })
    }

    pub(crate) fn pad_layers(self, pad: usize) -> Layers {
        self.instruments.pads.get(&pad).copied().unwrap_or_default()
    }

    pub(crate) fn key_range(self, note: u8, duration: f64) -> Option<(f64, f64)> {
        if self.chromatic_enabled {
            let sample = self.chromatic?;
            let end = sample.end.min(duration);
            return (sample.start < end).then_some((sample.start, end));
        }
        match self.key_chops.get(&note) {
            Some(KeyChop::Cut(start, end)) => Some((*start, *end)),
            Some(KeyChop::Muted) => None,
            None => self.slice(Studio::pad_for_key(note)?, duration),
        }
    }

    pub(crate) fn controls_for_key(self, note: u8) -> SampleControls {
        if self.chromatic_enabled
            && let Some(sample) = self.chromatic
        {
            let mut controls = sample.controls;
            controls.tune += f32::from(note) - f32::from(sample.root);
            return controls;
        }
        self.key_controls.get(&note).copied().unwrap_or_else(|| {
            if self.key_chops.contains_key(&note) {
                SampleControls::default()
            } else {
                Studio::pad_for_key(note)
                    .map_or_else(SampleControls::default, |pad| self.pad_controls[pad])
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_metadata_does_not_change_playback_but_source_identity_does() {
        let mut studio = Studio::default();
        studio
            .sources
            .push(crate::files::local_record("slice.wav".into()));
        studio.save_pattern("First pattern", false);
        let settings = PlaybackSettings::from(&studio);

        studio.sources[0].title = "Renamed source".into();
        studio.patterns[0].name = "Renamed pattern".into();
        studio.pad_names[0] = "Renamed pad".into();
        studio.pad_colors[0] = 2;
        assert!(settings.matches(&studio));
        assert!(settings.capture_matches(&studio));

        studio.sample_level = 0.5;
        assert!(!settings.matches(&studio));
        assert!(settings.capture_matches(&studio));

        studio.sources[0].source = Source::File("different.wav".into());
        assert!(!settings.capture_matches(&studio));
        assert!(!settings.same_capture_instruments(&PlaybackSettings::from(&studio)));
    }
}
