use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range, path::PathBuf};

mod automation;
pub mod beat_map;
pub mod drum_templates;
pub mod events;
pub mod grid_edit;
pub mod groove;
pub mod instrument;
pub mod melody;
pub mod project_version;
pub mod snapshot;
mod step_rule;
pub mod variation;
pub use step_rule::{FillCondition, StepRule};
pub use automation::{Automation, AutomationPoint, AutomationTarget, Curve};

pub const STEPS: usize = 16;
pub const TICKS_PER_STEP: u32 = 120;
pub const MAX_LANE_OFFSET: i8 = 30;
pub const TICKS_PER_BAR: u32 = TICKS_PER_STEP * STEPS as u32;
pub const MIN_LOOP_SECONDS: f64 = 0.01;
pub const MAX_NAMED_SAMPLES: usize = 256;
pub const MAX_SAMPLE_NAME: usize = 64;
pub const MAX_BARS: usize = 16;
pub const MAX_PATTERNS: usize = 32;
pub const MAX_SOURCES: usize = 24;
pub const MAX_SECTIONS: usize = 32;
pub const MAX_REPEATS: u8 = 16;
pub const MAX_SONG_BARS: usize = 256;
pub const PIANO_LOW: u8 = 21;
pub const PIANO_HIGH: u8 = 108;
pub const PIANO_PAD_BASE: u8 = 60;
const MAX_PERSISTED_EVENT_ID: u64 = 1 << 62;
pub const DRUMS: [&str; 11] = [
    "Kick", "Snare", "Low tom", "Mid tom", "High tom", "Rim", "Clap", "Hat", "Open hat", "Crash",
    "Ride",
];
pub const KITS: [&str; 4] = ["Studio", "Boom", "Punch", "Dusty"];
pub const GENRES: [&str; 23] = [
    "All",
    "Jazz",
    "Rock",
    "Funk",
    "Soul",
    "Fusion",
    "Psych",
    "Pop",
    "Disco",
    "Lounge",
    "Ambient",
    "Library",
    "Soundtrack",
    "World",
    "Latin",
    "Synth",
    "Folk",
    "Classical",
    "Blues",
    "Gospel",
    "Hip Hop",
    "Country",
    "Other",
];
pub const PAD_LABELS: [&str; STEPS] = [
    "Z", "X", "C", "V", "A", "S", "D", "F", "Q", "W", "E", "R", "1", "2", "3", "4",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DelayDivision {
    Sixteenth,
    Eighth,
    #[default]
    DottedEighth,
    Quarter,
}

impl DelayDivision {
    pub const ALL: [(Self, &'static str); 4] = [
        (Self::Sixteenth, "1/16"),
        (Self::Eighth, "1/8"),
        (Self::DottedEighth, "Dotted 1/8"),
        (Self::Quarter, "1/4"),
    ];

    pub fn beats(self) -> f64 {
        match self {
            Self::Sixteenth => 0.25,
            Self::Eighth => 0.5,
            Self::DottedEighth => 0.75,
            Self::Quarter => 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    Demo(usize),
    File(PathBuf),
    YouTube { id: String },
    Commons { media: String, page: String },
    Generated(u64),
    Extracted(Box<ExtractionOrigin>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExtractionOrigin {
    pub id: u64,
    pub parent_id: u64,
    pub parent_title: String,
    pub start_frame: usize,
    pub end_frame: usize,
    pub sample_rate: u32,
    pub prompt: String,
    pub model: String,
    pub revision: String,
    pub residual: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub title: String,
    pub artist: String,
    pub genre: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Box<ReleaseInfo>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub id: String,
    pub title: String,
    pub year: u16,
    pub country: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    #[default]
    Pending,
    Keep,
    Skip,
    Later,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueEntry {
    pub record: Record,
    pub views: Option<u64>,
    #[serde(default)]
    pub verdict: Verdict,
    #[serde(default)]
    pub auditioned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub taste_heard: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_range: Option<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity_aspects: Option<[u8; 4]>,
}

impl QueueEntry {
    pub fn new(record: Record, views: Option<u64>) -> Self {
        Self {
            record,
            views,
            verdict: Verdict::Pending,
            auditioned: false,
            taste_heard: false,
            match_range: None,
            similarity: None,
            similarity_aspects: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SampleControls {
    pub gain: f32,
    pub pan: f32,
    pub tune: f32,
    pub reverse: bool,
    pub attack_ms: u16,
    pub release_ms: u16,
}

impl Default for SampleControls {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            tune: 0.0,
            reverse: false,
            attack_ms: 2,
            release_ms: 40,
        }
    }
}

impl SampleControls {
    fn validate(&mut self) {
        self.gain = self.gain.clamp(0.0, 2.0);
        self.pan = self.pan.clamp(-1.0, 1.0);
        self.tune = self.tune.clamp(-24.0, 24.0);
        self.attack_ms = self.attack_ms.min(1000);
        self.release_ms = self.release_ms.min(1000);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeyChop {
    Cut(f64, f64),
    Muted,
}

pub fn piano_note(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(note % 12)],
        i16::from(note / 12) - 1
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChopInput {
    Pad(u8),
    Key(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayedChop {
    #[serde(default, skip_serializing_if = "events::NoteId::is_empty")]
    pub id: events::NoteId,
    pub tick: u32,
    pub input: ChopInput,
    pub velocity: u8,
    pub mode: crate::audio::PadMode,
    pub length: Option<u32>,
    #[serde(default, skip_serializing_if = "instrument::HitSound::is_default")]
    pub sound: instrument::HitSound,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<std::sync::Arc<melody::Expression>>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Quantize {
    #[default]
    Off,
    Eighth,
    Sixteenth,
    ThirtySecond,
    EighthTriplet,
    SixteenthTriplet,
}

impl Quantize {
    pub const ALL: [Self; 6] = [
        Self::Off, Self::Eighth, Self::Sixteenth, Self::ThirtySecond,
        Self::EighthTriplet, Self::SixteenthTriplet,
    ];

    pub fn triplet_unit(self) -> Option<u32> {
        match self {
            Self::EighthTriplet => Some(160),
            Self::SixteenthTriplet => Some(80),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::EighthTriplet => "1/8 triplet",
            Self::SixteenthTriplet => "1/16 triplet",
        }
    }

    pub fn snap(self, position: f64, swing: f32) -> u64 {
        let unit = match self {
            Self::Off => return position.round() as u64,
            Self::Eighth => 240,
            Self::Sixteenth => 120,
            Self::ThirtySecond => 60,
            Self::EighthTriplet | Self::SixteenthTriplet => {
                let unit = f64::from(self.triplet_unit().unwrap());
                return ((position / unit).round() * unit) as u64;
            }
        };
        let index = (position / unit as f64).floor() as u64;
        (index.saturating_sub(2)..=index + 2)
            .map(|index| {
                let straight = index * unit;
                let step = straight / u64::from(TICKS_PER_STEP);
                let start = swung_step(step, swing);
                if straight.is_multiple_of(u64::from(TICKS_PER_STEP)) {
                    start
                } else {
                    (start + swung_step(step + 1, swing)) / 2
                }
            })
            .min_by(|&a, &b| {
                (a as f64 - position)
                    .abs()
                    .total_cmp(&(b as f64 - position).abs())
            })
            .unwrap_or(0)
    }
}

pub fn swung_step(step: u64, swing: f32) -> u64 {
    step * u64::from(TICKS_PER_STEP)
        + if step.is_multiple_of(2) {
            0
        } else {
            (60.0 * swing).round() as u64
        }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceBar {
    pub drums: [[u8; STEPS]; DRUMS.len()],
    pub pads: [Option<usize>; STEPS],
    pub keys: [Option<u8>; STEPS],
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<u8, StepRule>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sounds: BTreeMap<u8, instrument::HitSound>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ids: StepIds,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub feel: groove::HitFeels,
}

pub type StepIds = BTreeMap<u8, events::NoteId>;

impl SequenceBar {
    pub fn metadata(&self) -> events::StepMetadata<'_> {
        events::StepMetadata { rules: &self.rules, sounds: &self.sounds, ids: &self.ids, feel: &self.feel }
    }
}

#[derive(Clone, Default)]
pub struct BarContent {
    steps: SequenceBar,
    notes: Vec<PlayedChop>,
}

// Values 1 and 2 keep the normal/accent levels in older project files.
pub fn drum_velocity(hit: u8) -> u8 {
    match hit {
        0 => 0,
        1 => 89,
        2 => 127,
        hit => hit.saturating_sub(2).min(127),
    }
}

pub fn encode_drum_velocity(velocity: u8) -> u8 {
    if velocity == 0 { 0 } else { velocity.min(127) + 2 }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sequence {
    pub bars: Vec<SequenceBar>,
    pub performance: Vec<PlayedChop>,
    #[serde(skip)]
    pub bar_cycles: Vec<u64>,
    #[serde(skip)]
    pub automation: Automation,
    #[serde(skip)]
    pub section_automation: Vec<(Range<u32>, Automation)>,
}

impl Sequence {
    pub fn clear_event_ids(&mut self) {
        for bar in &mut self.bars {
            bar.ids.clear();
        }
        for note in &mut self.performance {
            note.id = events::NoteId::default();
        }
    }

    pub fn section_automation_at(&self, tick: f64) -> Option<(&Automation, f64)> {
        let index = self
            .section_automation
            .partition_point(|(range, _)| f64::from(range.end) <= tick);
        self.section_automation
            .get(index)
            .filter(|(range, _)| f64::from(range.start) <= tick)
            .map(|(range, automation)| (automation, tick - f64::from(range.start)))
    }

    pub fn ticks(&self) -> u32 {
        self.bars.len() as u32 * TICKS_PER_BAR
    }

    pub fn has_key_steps(&self, studio: &Studio) -> bool {
        self.bars
            .iter()
            .flat_map(|bar| bar.keys.iter().flatten())
            .any(|&note| studio.key_range(note, f64::INFINITY).is_some())
            || self.performance.iter().any(|event| {
                matches!(event.input, ChopInput::Key(_)) && (event.expression.is_some() || studio.input_is_assigned(event.input))
            })
    }

    pub fn has_chops(&self, studio: &Studio) -> bool {
        self.has_pad_steps(studio) || self.has_key_steps(studio)
    }

    pub fn has_pad_steps(&self, studio: &Studio) -> bool {
        self.bars
            .iter()
            .flat_map(|bar| bar.pads.iter().flatten())
            .any(|&pad| studio.pads[pad].is_some())
            || self
                .performance
                .iter()
                .any(|event| matches!(event.input, ChopInput::Pad(_)) && studio.input_is_assigned(event.input))
    }

    pub fn has_hits(&self, studio: &Studio) -> bool {
        self.has_chops(studio)
            || self
                .bars
                .iter()
                .any(|bar| bar.drums.iter().flatten().any(|&hit| hit != 0))
    }
}

fn validate_step_rules(rules: &mut BTreeMap<u8, StepRule>) {
    rules.retain(|key, rule| {
        rule.validate();
        usize::from(*key) < (DRUMS.len() + 2) * STEPS && *rule != StepRule::default()
    });
}

fn validate_bar(
    pads: &mut [Option<usize>; STEPS],
    keys: &mut [Option<u8>; STEPS],
    drums: &mut [[u8; STEPS]; DRUMS.len()],
) {
    for pad in pads {
        *pad = pad.filter(|&pad| pad < STEPS);
    }
    for key in keys {
        *key = key.filter(|key| (PIANO_LOW..=PIANO_HIGH).contains(key));
    }
    for hit in drums.iter_mut().flatten() {
        *hit = (*hit).min(129);
    }
}

fn validate_performance(performance: &mut Vec<PlayedChop>) {
    performance.retain_mut(|event| {
        event.sound.validate();
        if event.expression.as_ref().is_some_and(|expression| !matches!(event.input, ChopInput::Key(_)) || !expression.valid()) {
            event.expression = None;
        }
        if event.id.0 >= MAX_PERSISTED_EVENT_ID {
            event.id = events::NoteId::default();
        }
        event.velocity = event.velocity.clamp(1, 127);
        event.length = event
            .length
            .map(|length| length.clamp(1, MAX_BARS as u32 * TICKS_PER_BAR));
        event.tick < MAX_BARS as u32 * TICKS_PER_BAR
            && match event.input {
                ChopInput::Pad(pad) => usize::from(pad) < STEPS,
                ChopInput::Key(note) => (PIANO_LOW..=PIANO_HIGH).contains(&note),
            }
    });
    if !performance.is_sorted_by_key(|event| (event.tick, event.input)) {
        performance.sort_by_key(|event| (event.tick, event.input));
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedPattern {
    pub name: String,
    pub length: usize,
    pub notes: Sequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionKind {
    Intro,
    Verse,
    Hook,
    Bridge,
    Outro,
}

impl SectionKind {
    pub const ALL: [Self; 5] = [
        Self::Intro,
        Self::Verse,
        Self::Hook,
        Self::Bridge,
        Self::Outro,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Intro => "Intro",
            Self::Verse => "Verse",
            Self::Hook => "Hook",
            Self::Bridge => "Bridge",
            Self::Outro => "Outro",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub kind: SectionKind,
    pub pattern: usize,
    pub repeats: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SectionSpan>,
    #[serde(default, skip_serializing_if = "Automation::is_empty")]
    pub automation: Automation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionSpan {
    pub offset: usize,
    pub bars: usize,
}

impl Section {
    pub fn new(kind: SectionKind, pattern: usize, repeats: u8) -> Self {
        Self {
            kind,
            pattern,
            repeats,
            span: None,
            automation: Automation::default(),
        }
    }

    pub fn bars(&self, pattern_length: usize) -> usize {
        self.span
            .map_or(pattern_length * usize::from(self.repeats), |span| span.bars)
    }

    pub fn offset(&self, pattern_length: usize) -> usize {
        self.span.map_or(0, |span| span.offset % pattern_length)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumMix {
    pub gain: f32,
    pub pan: f32,
    pub tune: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for DrumMix {
    fn default() -> Self {
        Self { gain: 1.0, pan: 0.0, tune: 0.0, mute: false, solo: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChokeGroups {
    pub pads: [u8; STEPS],
    pub drums: [u8; DRUMS.len()],
}

impl ChokeGroups {
    pub const MAX: u8 = 8;

    pub fn is_empty(&self) -> bool {
        self.pads.iter().chain(&self.drums).all(|&group| group == 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromaticSample {
    pub source: usize,
    pub start: f64,
    pub end: f64,
    pub root: u8,
    pub controls: SampleControls,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Studio {
    pub bpm: u16,
    pub bars: usize,
    pub kit: usize,
    pub pads: [Option<f64>; STEPS],
    #[serde(default)]
    pub pad_ends: [Option<f64>; STEPS],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<Record>,
    #[serde(default)]
    pub pad_sources: [usize; STEPS],
    #[serde(default)]
    pub drum_sources: [Option<usize>; DRUMS.len()],
    #[serde(default)]
    pub drum_mix: [DrumMix; DRUMS.len()],
    #[serde(default, skip_serializing_if = "ChokeGroups::is_empty")]
    pub choke_groups: ChokeGroups,
    #[serde(default, skip_serializing_if = "instrument::Instruments::is_default")]
    pub instruments: instrument::Instruments,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub key_sources: BTreeMap<u8, usize>,
    #[serde(default)]
    pub loop_source: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub warp_markers: BTreeMap<usize, Vec<crate::stretch::Marker>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub beat_maps: BTreeMap<usize, std::sync::Arc<beat_map::BeatMap>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub warp_sources: BTreeMap<usize, beat_map::WarpSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grooves: Vec<groove::Template>,
    #[serde(default)]
    pub chromatic_enabled: bool,
    #[serde(default)]
    pub harmony: crate::harmony::Harmony,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chromatic: Option<ChromaticSample>,
    #[serde(default)]
    pub pad_names: [String; STEPS],
    #[serde(default)]
    pub pad_colors: [u8; STEPS],
    #[serde(default)]
    pub pad_controls: [SampleControls; STEPS],
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub key_chops: BTreeMap<u8, KeyChop>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub key_controls: BTreeMap<u8, SampleControls>,
    pub loop_in: Option<f64>,
    pub loop_out: Option<f64>,
    pub looping: bool,
    pub speed: f64,
    pub chops: [[Option<usize>; STEPS]; MAX_BARS],
    #[serde(default)]
    pub key_steps: [[Option<u8>; STEPS]; MAX_BARS],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub performance: Vec<PlayedChop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patterns: Vec<NamedPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<Section>,
    #[serde(default, skip_serializing_if = "Automation::is_empty")]
    pub automation: Automation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editing_pattern: Option<usize>,
    pub hits: [[[u8; STEPS]; DRUMS.len()]; MAX_BARS],
    #[serde(default)]
    pub step_rules: [BTreeMap<u8, StepRule>; MAX_BARS],
    #[serde(default, skip_serializing_if = "empty_step_sounds")]
    pub step_sounds: [BTreeMap<u8, instrument::HitSound>; MAX_BARS],
    #[serde(default, skip_serializing_if = "empty_step_ids")]
    pub step_ids: [StepIds; MAX_BARS],
    #[serde(default, skip_serializing_if = "empty_step_feel")]
    pub step_feel: [groove::HitFeels; MAX_BARS],
    #[serde(default, skip_serializing_if = "zero_id")]
    pub event_id_next: u64,
    pub drum_level: f32,
    pub sample_level: f32,
    pub swing: f32,
    #[serde(default)]
    pub lane_offsets: [i8; grid_edit::LANES],
    pub reverb: f32,
    pub delay: f32,
    #[serde(default)]
    pub delay_division: DelayDivision,
    #[serde(default)]
    pub drum_fx_bypass: bool,
    #[serde(default)]
    pub mix_fx_bypass: bool,
    pub filter: f32,
}

impl Default for Studio {
    fn default() -> Self {
        Self {
            bpm: 90,
            bars: 1,
            kit: 0,
            pads: [None; STEPS],
            pad_ends: [None; STEPS],
            sources: Vec::new(),
            pad_sources: [0; STEPS],
            drum_sources: [None; DRUMS.len()],
            drum_mix: [DrumMix::default(); DRUMS.len()],
            choke_groups: ChokeGroups::default(),
            instruments: instrument::Instruments::default(),
            key_sources: BTreeMap::new(),
            loop_source: 0,
            warp_markers: BTreeMap::new(),
            beat_maps: BTreeMap::new(),
            warp_sources: BTreeMap::new(),
            grooves: Vec::new(),
            chromatic_enabled: false,
            harmony: Default::default(),
            chromatic: None,
            pad_names: std::array::from_fn(|_| String::new()),
            pad_colors: [0; STEPS],
            pad_controls: [SampleControls::default(); STEPS],
            key_chops: BTreeMap::new(),
            key_controls: BTreeMap::new(),
            loop_in: None,
            loop_out: None,
            looping: false,
            speed: 1.0,
            chops: [[None; STEPS]; MAX_BARS],
            key_steps: [[None; STEPS]; MAX_BARS],
            performance: Vec::new(),
            patterns: Vec::new(),
            sections: Vec::new(),
            automation: Automation::default(),
            editing_pattern: None,
            hits: [[[0; STEPS]; DRUMS.len()]; MAX_BARS],
            step_rules: Default::default(),
            step_sounds: Default::default(),
            step_ids: Default::default(),
            step_feel: Default::default(),
            event_id_next: 0,
            drum_level: 0.8,
            sample_level: 1.0,
            swing: 0.0,
            lane_offsets: [0; grid_edit::LANES],
            reverb: 0.0,
            delay: 0.0,
            delay_division: DelayDivision::default(),
            drum_fx_bypass: false,
            mix_fx_bypass: false,
            filter: 0.0,
        }
    }
}

fn empty_step_sounds(sounds: &[BTreeMap<u8, instrument::HitSound>; MAX_BARS]) -> bool {
    sounds.iter().all(BTreeMap::is_empty)
}

fn empty_step_feel(feel: &[groove::HitFeels; MAX_BARS]) -> bool {
    feel.iter().all(BTreeMap::is_empty)
}

fn empty_step_ids(ids: &[StepIds; MAX_BARS]) -> bool {
    ids.iter().all(BTreeMap::is_empty)
}

fn zero_id(id: &u64) -> bool {
    *id == 0
}

fn validate_step_sounds(sounds: &mut BTreeMap<u8, instrument::HitSound>) {
    sounds.retain(|&key, sound| {
        sound.validate();
        usize::from(key) < grid_edit::LANES * STEPS && !sound.is_default()
    });
}

impl Studio {
    pub fn uses_transcription(&self) -> bool {
        self.performance.iter().chain(self.patterns.iter().flat_map(|pattern| &pattern.notes.performance))
            .any(|note| note.expression.is_some())
    }

    pub fn uses_grooves(&self) -> bool {
        !self.grooves.is_empty() || !empty_step_feel(&self.step_feel)
            || self.patterns.iter().any(|pattern| pattern.notes.bars.iter().any(|bar| !bar.feel.is_empty()))
    }

    pub fn uses_beat_maps(&self) -> bool {
        !self.beat_maps.is_empty() || !self.warp_sources.is_empty()
    }

    pub fn validate_source_analysis(&mut self, media: Option<&crate::audio::Clip>) {
        self.beat_maps.retain(|&source, map| {
            let Some(clip) = media.and_then(|root| root.source(source)) else { return false };
            std::sync::Arc::make_mut(map).validate(clip.duration());
            true
        });
        self.warp_sources.retain(|&source, warp| {
            let Some(parent) = media.and_then(|root| root.source(warp.parent)) else { return false };
            media.is_some_and(|root| root.source(source).is_some())
                && source != warp.parent && warp.range.0 >= 0.0
                && warp.range.1 <= parent.duration() && warp.range.1 > warp.range.0
                && (40..=240).contains(&warp.target_bpm)
                && (0.0..=128.0).contains(&warp.target_beats)
                && warp.markers.len() <= crate::stretch::Marker::MAX
                && warp.markers.iter().all(|marker| {
                    (warp.range.0..warp.range.1).contains(&marker.source_seconds)
                        && (0.0..warp.target_beats).contains(&marker.target_beat)
                })
        });
    }
    fn next_event_id(&self) -> u64 {
        let working = self.step_ids.iter().flat_map(|bar| bar.values()).copied()
            .chain(self.performance.iter().map(|note| note.id));
        let saved = self.patterns.iter().flat_map(|pattern| {
            pattern.notes.bars.iter().flat_map(|bar| bar.ids.values()).copied()
                .chain(pattern.notes.performance.iter().map(|note| note.id))
        });
        let largest = working.chain(saved).map(|id| id.0)
            .filter(|&id| id < MAX_PERSISTED_EVENT_ID).max().unwrap_or(0);
        (largest + 1).max(if self.event_id_next < MAX_PERSISTED_EVENT_ID {
            self.event_id_next
        } else {
            1
        })
    }

    pub fn identify_working_events(&mut self) {
        let mut missing = Vec::new();
        for bar in 0..MAX_BARS {
            for lane in 0..grid_edit::LANES {
                for step in 0..STEPS {
                    let key = (lane * STEPS + step) as u8;
                    if self.step_has_hit(bar, lane, step) && !self.step_ids[bar].contains_key(&key) {
                        missing.push((bar, key));
                    }
                }
            }
        }
        if missing.is_empty() && self.performance.iter().all(|note| !note.id.is_empty()) {
            return;
        }
        let mut next = self.next_event_id();
        for (bar, key) in missing {
            self.step_ids[bar].insert(key, events::NoteId(next));
            next += 1;
        }
        for note in self.performance.iter_mut().filter(|note| note.id.is_empty()) {
            note.id = events::NoteId(next);
            next += 1;
        }
        self.event_id_next = next;
    }

    pub fn identify_saved_events(&mut self) {
        let mut next = self.next_event_id();
        let start = next;
        for (index, pattern) in self.patterns.iter_mut().enumerate() {
            if self.editing_pattern == Some(index) {
                continue;
            }
            for bar in &mut pattern.notes.bars {
                for lane in 0..grid_edit::LANES {
                    for step in 0..STEPS {
                        let key = (lane * STEPS + step) as u8;
                        let hit = match lane {
                            0 => bar.pads[step].is_some(),
                            1 => bar.keys[step].is_some(),
                            drum => bar.drums[drum - 2][step] != 0,
                        };
                        if hit {
                            bar.ids.entry(key).or_insert_with(|| {
                                let id = events::NoteId(next);
                                next += 1;
                                id
                            });
                        }
                    }
                }
            }
            for note in pattern.notes.performance.iter_mut().filter(|note| note.id.is_empty()) {
                note.id = events::NoteId(next);
                next += 1;
            }
        }
        if next != start {
            self.event_id_next = next;
        }
    }

    pub fn uses_instruments(&self) -> bool {
        !self.instruments.is_default()
            || !empty_step_sounds(&self.step_sounds)
            || !empty_step_ids(&self.step_ids)
            || self.event_id_next != 0
            || self.performance.iter().any(|note| !note.sound.is_default() || !note.id.is_empty())
            || self.patterns.iter().any(|pattern| {
                pattern.notes.bars.iter().any(|bar| !bar.sounds.is_empty() || !bar.ids.is_empty())
                    || pattern.notes.performance.iter().any(|note| !note.sound.is_default() || !note.id.is_empty())
            })
    }

    pub fn set_step_rule(&mut self, bar: usize, row: usize, step: usize, rule: StepRule) {
        let key = (row * STEPS + step) as u8;
        if rule == StepRule::default() {
            self.step_rules[bar].remove(&key);
        } else {
            self.step_rules[bar].insert(key, rule);
        }
    }

    pub fn step_has_hit(&self, bar: usize, lane: usize, step: usize) -> bool {
        match lane {
            0 => self.chops[bar][step].is_some(),
            1 => self.key_steps[bar][step].is_some(),
            _ => self.hits[bar][lane - 2][step] != 0,
        }
    }

    pub fn set_step_sound(&mut self, bar: usize, lane: usize, step: usize, sound: instrument::HitSound) {
        let key = (lane * STEPS + step) as u8;
        if sound.is_default() {
            self.step_sounds[bar].remove(&key);
        } else if self.step_has_hit(bar, lane, step) {
            self.step_sounds[bar].insert(key, sound);
        }
    }

    pub fn set_roll_sound(&mut self, tick: u32, input: ChopInput, sound: instrument::HitSound) {
        let key = (tick, input);
        if let Ok(index) = self.performance.binary_search_by_key(&key, |note| (note.tick, note.input)) {
            self.performance[index].sound = sound;
        } else {
            let step = self.roll_step_notes().find(|(_, _, note)| (note.tick, note.input) == key);
            if let Some((bar, step, _)) = step {
                self.set_step_sound(bar, usize::from(matches!(input, ChopInput::Key(_))), step, sound);
            }
        }
    }

    pub fn bar_notes(&self, index: usize) -> SequenceBar {
        SequenceBar {
            drums: self.hits[index],
            pads: self.chops[index],
            keys: self.key_steps[index],
            rules: self.step_rules[index].clone(),
            sounds: self.step_sounds[index].clone(),
            ids: self.step_ids[index].clone(),
            feel: self.step_feel[index].clone(),
        }
    }

    pub fn step_hit_ticks(
        &self, rule: StepRule, row: usize, absolute: u64, cycle: u64, fill: bool, feel: groove::HitFeel,
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

    fn roll_step_notes(&self) -> impl Iterator<Item = (usize, usize, PlayedChop)> + '_ {
        (0..self.bars).flat_map(move |bar| {
            (0..STEPS).flat_map(move |step| {
                let absolute = (bar * STEPS + step) as u64;
                [
                    self.chops[bar][step].map(|pad| (0, ChopInput::Pad(pad as u8), 127)),
                    self.key_steps[bar][step].map(|note| (1, ChopInput::Key(note), 112)),
                ].into_iter().flatten().map(move |(row, input, velocity)| {
                    // Show the first hit even when its condition only plays on some passes.
                    let repeats = self.step_rules[bar].get(&((row * STEPS + step) as u8))
                        .map_or(1, |rule| rule.repeats);
                    let rule = StepRule { repeats, ..StepRule::default() };
                    let feel = self.step_feel[bar].get(&((row * STEPS + step) as u8)).copied().unwrap_or_default();
                    let tick = self.step_hit_ticks(rule, row, absolute, 0, false, feel).next().unwrap().0 as u32;
                    let velocity = feel.velocity.unwrap_or(velocity);
                    (bar, step, PlayedChop {
                        id: self.step_ids[bar].get(&((row * STEPS + step) as u8)).copied().unwrap_or_default(),
                        tick, input, velocity, mode: crate::audio::PadMode::Slice, length: None,
                        sound: self.step_sounds[bar].get(&((row * STEPS + step) as u8)).copied().unwrap_or_default(),
                        expression: None,
                    })
                })
            })
        })
    }

    pub fn roll_notes(&self) -> Vec<PlayedChop> {
        let mut notes = self.performance.clone();
        notes.extend(self.roll_step_notes().map(|(_, _, note)| note).filter(|note| {
            self.performance.binary_search_by_key(&(note.tick, note.input), |event| (event.tick, event.input)).is_err()
        }));
        notes.sort_by_key(|note| (note.tick, note.input));
        notes
    }

    pub fn edit_roll_notes<T>(&mut self, edit: impl FnOnce(&mut Vec<PlayedChop>) -> T) -> T {
        let before = self.roll_notes();
        let mut after = before.clone();
        let result = edit(&mut after);
        if before == after {
            return result;
        }
        let step_notes: Vec<_> = self.roll_step_notes().collect();
        for (bar, step, note) in step_notes {
            let key = (note.tick, note.input);
            let old = before.binary_search_by_key(&key, |event| (event.tick, event.input))
                .ok().map(|index| &before[index]);
            let new = after.binary_search_by_key(&key, |event| (event.tick, event.input)).ok();
            if let Some(index) = new && Some(&after[index]) == old {
                // Leave untouched step entries on the grid so they still follow swing.
                if self.performance.binary_search_by_key(&key, |event| (event.tick, event.input)).is_err() {
                    after.remove(index);
                }
            } else {
                match note.input {
                    ChopInput::Pad(_) => self.chops[bar][step] = None,
                    ChopInput::Key(_) => self.key_steps[bar][step] = None,
                }
                let row = usize::from(matches!(note.input, ChopInput::Key(_)));
                self.step_sounds[bar].remove(&((row * STEPS + step) as u8));
                self.step_ids[bar].remove(&((row * STEPS + step) as u8));
                self.step_feel[bar].remove(&((row * STEPS + step) as u8));
            }
        }
        self.performance = after;
        result
    }

    pub fn active_sequence(&self) -> Sequence {
        Sequence {
            bars: (0..self.bars).map(|bar| self.bar_notes(bar)).collect(),
            performance: self
                .performance
                .iter()
                .take_while(|event| event.tick < self.pattern_ticks())
                .cloned()
                .collect(),
            ..Sequence::default()
        }
    }

    pub fn save_pattern(&mut self, name: &str, blank: bool) {
        if self.patterns.len() >= MAX_PATTERNS || name.trim().is_empty() {
            return;
        }
        self.sync_pattern();
        let index = self.patterns.len();
        let duplicate = self.editing_pattern.is_some();
        self.patterns.push(NamedPattern {
            name: name.trim().chars().take(MAX_SAMPLE_NAME).collect(),
            length: if blank { 1 } else { self.bars },
            notes: if blank {
                Sequence::default()
            } else {
                Sequence {
                    bars: (0..MAX_BARS).map(|bar| self.bar_notes(bar)).collect(),
                    performance: self.performance.clone(),
                    ..Sequence::default()
                }
            },
        });
        if duplicate {
            self.patterns[index].notes.clear_event_ids();
        }
        self.load_pattern(index);
    }

    pub fn save_captured_pattern(&mut self, bars: usize, notes: Vec<PlayedChop>) -> Option<String> {
        if notes.is_empty() {
            return None;
        }
        let name = (1..)
            .map(|number| format!("Capture {number}"))
            .find(|name| !self.patterns.iter().any(|pattern| pattern.name.eq_ignore_ascii_case(name)))
            .unwrap();
        let sequence = Sequence { bars: vec![SequenceBar::default(); bars], performance: notes, ..Sequence::default() };
        self.save_generated_pattern(&name, sequence)
    }

    pub fn save_generated_pattern(&mut self, name: &str, mut sequence: Sequence) -> Option<String> {
        let save_draft = self.has_draft();
        let name: String = name.trim().chars().take(MAX_SAMPLE_NAME).collect();
        if !(1..=MAX_BARS).contains(&sequence.bars.len())
            || self.patterns.len() + usize::from(save_draft) >= MAX_PATTERNS
            || name.is_empty()
            || self.patterns.iter().any(|pattern| pattern.name.eq_ignore_ascii_case(&name))
        {
            return None;
        }
        if save_draft {
            self.save_pattern(&self.next_pattern_name_except(&name), false);
        }
        let index = self.patterns.len();
        sequence.clear_event_ids();
        self.patterns.push(NamedPattern {
            name: name.clone(),
            length: sequence.bars.len(),
            notes: sequence,
        });
        self.load_pattern(index);
        self.sync_pattern();
        Some(name)
    }

    pub fn sync_pattern(&mut self) {
        let Some(index) = self.editing_pattern else {
            return;
        };
        let pattern = &self.patterns[index];
        let changed = pattern.length != self.bars
            || pattern.notes.performance != self.performance
            || pattern.notes.bars.len() != MAX_BARS
            || pattern
                .notes
                .bars
                .iter()
                .enumerate()
                .any(|(index, bar)| {
                    bar.drums != self.hits[index]
                        || bar.pads != self.chops[index]
                        || bar.keys != self.key_steps[index]
                        || bar.rules != self.step_rules[index]
                        || bar.sounds != self.step_sounds[index]
                        || bar.ids != self.step_ids[index]
                        || bar.feel != self.step_feel[index]
                });
        if changed {
            let bars = (0..MAX_BARS).map(|bar| self.bar_notes(bar)).collect();
            self.patterns[index].length = self.bars;
            self.patterns[index].notes = Sequence {
                bars,
                performance: self.performance.clone(),
                ..Sequence::default()
            };
        }
    }

    pub fn load_pattern(&mut self, index: usize) {
        self.sync_pattern();
        let pattern = &self.patterns[index];
        self.bars = pattern.length;
        self.performance.clone_from(&pattern.notes.performance);
        for bar in 0..MAX_BARS {
            let notes = pattern.notes.bars.get(bar).cloned().unwrap_or_default();
            self.hits[bar] = notes.drums;
            self.chops[bar] = notes.pads;
            self.key_steps[bar] = notes.keys;
            self.step_rules[bar] = notes.rules;
            self.step_sounds[bar] = notes.sounds;
            self.step_ids[bar] = notes.ids;
            self.step_feel[bar] = notes.feel;
        }
        self.editing_pattern = Some(index);
    }

    pub fn song_bars(&self) -> usize {
        self.sections
            .iter()
            .map(|section| section.bars(self.patterns[section.pattern].length))
            .sum()
    }

    pub fn section_range(&self, index: usize) -> Option<Range<u32>> {
        let section = self.sections.get(index)?;
        let bars = |section: &Section| section.bars(self.patterns[section.pattern].length);
        let start = self.sections[..index].iter().map(bars).sum::<usize>() as u32 * TICKS_PER_BAR;
        Some(start..start + bars(section) as u32 * TICKS_PER_BAR)
    }

    pub fn song_sequence(&self) -> Option<Sequence> {
        if !(1..=MAX_SONG_BARS).contains(&self.song_bars()) {
            return None;
        }
        let mut song = Sequence {
            automation: self.automation.clone(),
            ..Sequence::default()
        };
        for section in &self.sections {
            let pattern = &self.patterns[section.pattern];
            let source_offset = section.offset(pattern.length);
            let bars = section.bars(pattern.length);
            if !section.automation.is_empty() {
                let start = song.ticks();
                song.section_automation.push((
                    start..start + bars as u32 * TICKS_PER_BAR,
                    section.automation.clone(),
                ));
            }
            for bar in 0..bars {
                let source = (source_offset + bar) % pattern.length;
                let start = source as u32 * TICKS_PER_BAR;
                let offset = song.ticks();
                let events = &pattern.notes.performance;
                let first_event = events.partition_point(|event| event.tick < start);
                song.bar_cycles.push(((source_offset + bar) / pattern.length) as u64);
                song.bars
                    .push(pattern.notes.bars.get(source).cloned().unwrap_or_default());
                song.performance.extend(
                    events[first_event..]
                        .iter()
                        .take_while(|event| event.tick < start + TICKS_PER_BAR)
                        .cloned()
                        .map(|event| PlayedChop {
                            tick: offset + (event.tick - start),
                            ..event
                        }),
                );
            }
        }
        melody::join_continuations(&mut song.performance);
        Some(song)
    }

    pub fn render_sequence(&self) -> Option<Sequence> {
        if self.sections.is_empty() {
            Some(self.active_sequence())
        } else {
            self.song_sequence()
        }
    }

    pub fn has_draft(&self) -> bool {
        self.editing_pattern.is_none()
            && (self.bars > 1
                || !self.performance.is_empty()
                || (0..MAX_BARS).any(|bar| self.bar_notes(bar) != SequenceBar::default()))
    }

    pub fn next_pattern_name(&self) -> String {
        self.next_pattern_name_except("")
    }

    pub fn next_pattern_name_except(&self, excluded: &str) -> String {
        (1..)
            .map(|number| format!("Pattern {number}"))
            .find(|name| {
                !name.eq_ignore_ascii_case(excluded)
                    && !self
                        .patterns
                        .iter()
                        .any(|pattern| pattern.name.eq_ignore_ascii_case(name))
            })
            .unwrap()
    }

    pub fn remove_pattern(&mut self, index: usize) {
        if self.editing_pattern == Some(index)
            || self.sections.iter().any(|section| section.pattern == index)
        {
            return;
        }
        self.patterns.remove(index);
        for section in &mut self.sections {
            if section.pattern > index {
                section.pattern -= 1;
            }
        }
        if let Some(current) = &mut self.editing_pattern
            && *current > index
        {
            *current -= 1;
        }
    }

    pub fn validate_song(&mut self) {
        self.automation.validate();
        self.grooves.truncate(groove::MAX_TEMPLATES);
        self.grooves.retain_mut(groove::Template::validate);
        self.patterns.truncate(MAX_PATTERNS);
        for pattern in &mut self.patterns {
            pattern.name = pattern.name.trim().chars().take(MAX_SAMPLE_NAME).collect();
            if pattern.name.is_empty() {
                pattern.name = "Pattern".into();
            }
            pattern.length = pattern.length.clamp(1, MAX_BARS);
            pattern.notes.bars.truncate(MAX_BARS);
            for bar in &mut pattern.notes.bars {
                validate_bar(&mut bar.pads, &mut bar.keys, &mut bar.drums);
                validate_step_rules(&mut bar.rules);
                validate_step_sounds(&mut bar.sounds);
                groove::validate_feel(&mut bar.feel, &bar.pads, &bar.keys, &bar.drums);
                bar.ids.retain(|&key, id| {
                    usize::from(key) < grid_edit::LANES * STEPS
                        && !id.is_empty() && id.0 < MAX_PERSISTED_EVENT_ID
                });
            }
            validate_performance(&mut pattern.notes.performance);
        }
        self.editing_pattern = self
            .editing_pattern
            .filter(|&index| index < self.patterns.len());
        self.sections.truncate(MAX_SECTIONS);
        self.sections.retain_mut(|section| {
            section.automation.validate();
            section.repeats = section.repeats.clamp(1, MAX_REPEATS);
            if let Some(span) = &mut section.span {
                span.bars = span.bars.clamp(1, MAX_SONG_BARS);
                span.offset = span.offset.min(MAX_BARS - 1);
            }
            section.pattern < self.patterns.len()
        });
        self.sync_pattern();
    }

    pub fn validate(&mut self, duration: f64) {
        self.validate_sources(|_| duration, 0);
    }

    pub fn validate_media(&mut self, media: Option<&crate::audio::Clip>) {
        self.validate_sources(
            |index| media.and_then(|clip| clip.source(index)).map_or(0.0, |clip| clip.duration()),
            media.map_or(0, |clip| clip.sources.len()),
        );
    }

    fn validate_sources(&mut self, duration: impl Fn(usize) -> f64, extra_count: usize) {
        for group in self.choke_groups.pads.iter_mut().chain(&mut self.choke_groups.drums) {
            if *group > ChokeGroups::MAX {
                *group = 0;
            }
        }
        self.sources.truncate(extra_count.min(MAX_SOURCES - 1));
        let max_index = self.sources.len();
        self.instruments.validate(&duration, max_index);
        self.loop_source = self.loop_source.min(max_index);
        for index in &mut self.pad_sources {
            *index = (*index).min(max_index);
        }
        for source in &mut self.drum_sources {
            *source = source.filter(|index| *index <= max_index && duration(*index) > 0.0);
        }
        for strip in &mut self.drum_mix {
            strip.gain = if strip.gain.is_finite() { strip.gain.clamp(0.0, 2.0) } else { 1.0 };
            strip.pan = if strip.pan.is_finite() { strip.pan.clamp(-1.0, 1.0) } else { 0.0 };
            strip.tune = if strip.tune.is_finite() { strip.tune.clamp(-24.0, 24.0) } else { 0.0 };
        }
        self.chromatic = self.chromatic.filter(|sample| {
            sample.source <= max_index && sample.start.is_finite() && sample.end.is_finite()
        }).and_then(|mut sample| {
            let length = duration(sample.source);
            sample.start = sample.start.clamp(0.0, length);
            sample.end = sample.end.clamp(0.0, length);
            sample.root = sample.root.clamp(PIANO_LOW, PIANO_HIGH);
            sample.controls.validate();
            (sample.end > sample.start).then_some(sample)
        });
        self.key_sources.retain(|note, source| self.key_chops.contains_key(note) && *source <= max_index && *source != 0);
        for rules in &mut self.step_rules {
            validate_step_rules(rules);
        }
        for (bar, sounds) in self.step_sounds.iter_mut().enumerate() {
            validate_step_sounds(sounds);
            sounds.retain(|&key, _| {
                let step = usize::from(key) % STEPS;
                match usize::from(key) / STEPS {
                    0 => self.chops[bar][step].is_some(),
                    1 => self.key_steps[bar][step].is_some(),
                    drum => self.hits[bar][drum - 2][step] != 0,
                }
            });
        }
        for (bar, ids) in self.step_ids.iter_mut().enumerate() {
            ids.retain(|&key, id| {
                if usize::from(key) >= grid_edit::LANES * STEPS || id.is_empty()
                    || id.0 >= MAX_PERSISTED_EVENT_ID
                {
                    return false;
                }
                let step = usize::from(key) % STEPS;
                match usize::from(key) / STEPS {
                    0 => self.chops[bar][step].is_some(),
                    1 => self.key_steps[bar][step].is_some(),
                    drum => self.hits[bar][drum - 2][step] != 0,
                }
            });
        }
        self.warp_markers.retain(|source, markers| {
            if *source > max_index { return false; }
            let seconds = duration(*source);
            markers.retain(|marker| marker.source_seconds.is_finite() && marker.target_beat.is_finite()
                && (0.0..=seconds).contains(&marker.source_seconds) && (0.0..=128.0).contains(&marker.target_beat));
            markers.truncate(crate::stretch::Marker::MAX);
            !markers.is_empty()
        });
        self.harmony.root %= 12;
        self.bpm = self.bpm.clamp(40, 240);
        self.bars = self.bars.clamp(1, MAX_BARS);
        self.kit = self.kit.min(KITS.len() - 1);
        self.speed = self.speed.clamp(0.5, 2.0);
        let clamp_point = |point: &mut Option<f64>, length| {
            *point = point.filter(|time| time.is_finite()).map(|time| time.clamp(0.0, length));
        };
        clamp_point(&mut self.loop_in, duration(self.loop_source));
        clamp_point(&mut self.loop_out, duration(self.loop_source));
        for (index, (start, end)) in self.pads.iter_mut().zip(&mut self.pad_ends).enumerate() {
            let length = duration(self.pad_sources[index]);
            clamp_point(start, length);
            *end = end.filter(|end| end.is_finite() && *end > 0.0 && start.is_some() && length > 0.0)
                .map(|end| end.min(length));
            if let (Some(start), Some(end)) = (start, *end) {
                let minimum = 0.001_f64.min(end);
                *start = start.min(end - minimum);
            }
        }
        self.looping &= self.loop_range().is_some();
        self.key_chops.retain(|note, chop| {
            if !(PIANO_LOW..=PIANO_HIGH).contains(note) {
                return false;
            }
            match chop {
                KeyChop::Cut(start, end) if start.is_finite() && end.is_finite() => {
                    let length = duration(self.key_sources.get(note).copied().unwrap_or(0));
                    *start = start.clamp(0.0, length);
                    *end = end.clamp(0.0, length);
                    *end > *start
                }
                KeyChop::Cut(..) => false,
                KeyChop::Muted => true,
            }
        });
        self.key_controls
            .retain(|note, _| (PIANO_LOW..=PIANO_HIGH).contains(note));
        for control in self
            .pad_controls
            .iter_mut()
            .chain(self.key_controls.values_mut())
        {
            control.validate();
        }
        for value in [
            &mut self.drum_level,
            &mut self.sample_level,
            &mut self.swing,
            &mut self.reverb,
            &mut self.delay,
            &mut self.filter,
        ] {
            *value = value.clamp(0.0, 1.0);
        }
        for bar in 0..MAX_BARS {
            validate_bar(
                &mut self.chops[bar],
                &mut self.key_steps[bar],
                &mut self.hits[bar],
            );
            groove::validate_feel(&mut self.step_feel[bar], &self.chops[bar], &self.key_steps[bar], &self.hits[bar]);
        }
        for offset in &mut self.lane_offsets {
            *offset = (*offset).clamp(-MAX_LANE_OFFSET, MAX_LANE_OFFSET);
        }
        validate_performance(&mut self.performance);
    }

    pub fn loop_range(&self) -> Option<(f64, f64)> {
        let start = self.loop_in?;
        let end = self.loop_out?;
        (end - start >= MIN_LOOP_SECONDS - 1e-9).then_some((start, end))
    }

    pub fn slice(&self, pad: usize, duration: f64) -> Option<(f64, f64)> {
        let start = self.pads[pad]?;
        let end = self.pad_ends[pad].map(|end| end.min(duration)).unwrap_or_else(|| self
            .pads
            .iter()
            .zip(self.pad_sources)
            .filter_map(|(&time, source)| (source == self.pad_sources[pad]).then_some(time).flatten())
            .chain(self.loop_out.filter(|_| self.loop_source == self.pad_sources[pad]))
            .filter(|time| *time > start + 0.001)
            .fold(duration, f64::min));
        (end > start).then_some((start, end))
    }

    pub fn pad_for_key(note: u8) -> Option<usize> {
        note.checked_sub(PIANO_PAD_BASE)
            .map(usize::from)
            .filter(|&pad| pad < STEPS)
    }

    pub fn key_source(&self, note: u8) -> usize {
        if self.chromatic_enabled {
            return self.chromatic.map_or(0, |sample| sample.source);
        }
        if self.key_chops.contains_key(&note) {
            self.key_sources.get(&note).copied().unwrap_or(0)
        } else {
            Self::pad_for_key(note).map_or(0, |pad| self.pad_sources[pad])
        }
    }

    pub fn key_layers(&self, note: u8) -> instrument::Layers {
        if self.chromatic_enabled {
            return self.instruments.chromatic;
        }
        self.instruments.keys.get(&note).copied().unwrap_or_else(|| {
            if self.key_chops.contains_key(&note) {
                instrument::Layers::default()
            } else {
                Self::pad_for_key(note).map_or_else(instrument::Layers::default, |pad| {
                    self.pad_layers(pad)
                })
            }
        })
    }

    pub fn pad_layers(&self, pad: usize) -> instrument::Layers {
        self.instruments.pads.get(&pad).copied().unwrap_or_default()
    }

    pub fn key_range(&self, note: u8, duration: f64) -> Option<(f64, f64)> {
        if self.chromatic_enabled {
            let sample = self.chromatic?;
            let end = sample.end.min(duration);
            return (sample.start < end).then_some((sample.start, end));
        }
        match self.key_chops.get(&note) {
            Some(KeyChop::Cut(start, end)) => Some((*start, *end)),
            Some(KeyChop::Muted) => None,
            None => self.slice(Self::pad_for_key(note)?, duration),
        }
    }

    pub fn controls_for_key(&self, note: u8) -> SampleControls {
        if self.chromatic_enabled && let Some(sample) = self.chromatic {
            let mut controls = sample.controls;
            controls.tune += f32::from(note) - f32::from(sample.root);
            return controls;
        }
        self.key_controls.get(&note).copied().unwrap_or_else(|| {
            if self.key_chops.contains_key(&note) {
                SampleControls::default()
            } else {
                Self::pad_for_key(note)
                    .map_or_else(SampleControls::default, |pad| self.pad_controls[pad])
            }
        })
    }

    #[cfg(test)]
    pub fn auto_chop(&mut self) {
        if let Some((start, end)) = self.loop_range() {
            let boundaries: Vec<_> = (0..=STEPS)
                .map(|index| start + (end - start) * index as f64 / STEPS as f64)
                .collect();
            self.apply_slices(&boundaries, 0);
        }
    }

    pub fn apply_slices(&mut self, boundaries: &[f64], source: usize) {
        for (index, (start, end)) in self.pads.iter_mut().zip(&mut self.pad_ends).enumerate() {
            *start = boundaries.get(index..=index + 1).map(|pair| pair[0]);
            *end = boundaries.get(index + 1).copied();
            self.pad_sources[index] = if start.is_some() { source } else { 0 };
        }
    }

    pub fn copy_bar(&self, index: usize) -> BarContent {
        let start = index as u32 * TICKS_PER_BAR;
        BarContent {
            steps: self.bar_notes(index),
            notes: self.performance.iter()
                .filter(|event| (start..start + TICKS_PER_BAR).contains(&event.tick))
                .cloned()
                .map(|event| PlayedChop { tick: event.tick - start, ..event })
                .collect(),
        }
    }

    pub fn paste_bar(&mut self, index: usize, content: &BarContent) {
        let mut content = content.clone();
        content.steps.ids.clear();
        for note in &mut content.notes {
            note.id = events::NoteId::default();
        }
        self.replace_bar(index, &content);
    }

    fn replace_bar(&mut self, index: usize, content: &BarContent) {
        let start = index as u32 * TICKS_PER_BAR;
        self.hits[index] = content.steps.drums;
        self.chops[index] = content.steps.pads;
        self.key_steps[index] = content.steps.keys;
        self.step_rules[index].clone_from(&content.steps.rules);
        self.step_sounds[index].clone_from(&content.steps.sounds);
        self.step_ids[index].clone_from(&content.steps.ids);
        self.step_feel[index].clone_from(&content.steps.feel);
        self.performance.retain(|event| !(start..start + TICKS_PER_BAR).contains(&event.tick));
        self.performance.extend(content.notes.iter().map(|event| PlayedChop {
            tick: event.tick + start, ..event.clone()
        }));
        self.performance.sort_by_key(|event| (event.tick, event.input));
    }

    pub fn insert_bar(&mut self, index: usize) {
        if self.bars == MAX_BARS || index > self.bars {
            return;
        }
        self.hits.copy_within(index..self.bars, index + 1);
        self.chops.copy_within(index..self.bars, index + 1);
        self.key_steps.copy_within(index..self.bars, index + 1);
        self.step_rules[index..=self.bars].rotate_right(1);
        self.step_sounds[index..=self.bars].rotate_right(1);
        self.step_ids[index..=self.bars].rotate_right(1);
        self.step_feel[index..=self.bars].rotate_right(1);
        let start = index as u32 * TICKS_PER_BAR;
        let end = self.pattern_ticks();
        self.performance.retain(|event| event.tick < end);
        for event in &mut self.performance {
            if event.tick >= start {
                event.tick += TICKS_PER_BAR;
            }
        }
        self.bars += 1;
        self.paste_bar(index, &BarContent::default());
    }

    pub fn remove_bar(&mut self, index: usize) {
        if self.bars <= 1 || index >= self.bars {
            return;
        }
        let start = index as u32 * TICKS_PER_BAR;
        let end = start + TICKS_PER_BAR;
        self.performance.retain(|event| !(start..end).contains(&event.tick));
        for event in &mut self.performance {
            if event.tick >= end {
                event.tick -= TICKS_PER_BAR;
            }
        }
        self.hits.copy_within(index + 1..self.bars, index);
        self.chops.copy_within(index + 1..self.bars, index);
        self.key_steps.copy_within(index + 1..self.bars, index);
        self.step_rules[index..self.bars].rotate_left(1);
        self.step_sounds[index..self.bars].rotate_left(1);
        self.step_ids[index..self.bars].rotate_left(1);
        self.step_feel[index..self.bars].rotate_left(1);
        self.bars -= 1;
        self.paste_bar(self.bars, &BarContent::default());
    }

    pub fn pattern_ticks(&self) -> u32 {
        self.bars as u32 * TICKS_PER_BAR
    }

    pub fn same_capture_instruments(&self, other: &Self) -> bool {
        self.pads == other.pads
            && self.pad_ends == other.pad_ends
            && self.pad_sources == other.pad_sources
            && self.pad_controls == other.pad_controls
            && self.key_chops == other.key_chops
            && self.key_sources == other.key_sources
            && self.key_controls == other.key_controls
            && self.chromatic_enabled == other.chromatic_enabled
            && self.chromatic == other.chromatic
            && self.choke_groups == other.choke_groups
            && self.instruments == other.instruments
            && self.sources.iter().map(|record| &record.source)
                .eq(other.sources.iter().map(|record| &record.source))
            && self.speed == other.speed
    }

    pub fn performance_is_playable(&self, event: &PlayedChop) -> bool {
        event.tick < self.pattern_ticks() && self.input_is_assigned(event.input)
    }

    pub fn input_is_assigned(&self, input: ChopInput) -> bool {
        match input {
            ChopInput::Pad(pad) => self.pads.get(usize::from(pad)).is_some_and(Option::is_some),
            ChopInput::Key(note) => self.key_range(note, f64::INFINITY).is_some(),
        }
    }

    pub fn has_pattern(&self) -> bool {
        self.has_chops()
            || self.hits[..self.bars]
                .iter()
                .flatten()
                .flatten()
                .any(|&hit| hit != 0)
    }

    pub fn has_chops(&self) -> bool {
        self.chops[..self.bars]
            .iter()
            .flatten()
            .flatten()
            .any(|&pad| self.pads[pad].is_some())
            || self.has_key_steps()
            || self
                .performance
                .iter()
                .any(|event| self.performance_is_playable(event))
    }

    pub fn has_key_steps(&self) -> bool {
        self.key_steps[..self.bars]
            .iter()
            .flatten()
            .flatten()
            .any(|&note| self.key_range(note, f64::INFINITY).is_some())
            || self.performance.iter().any(|event| {
                matches!(event.input, ChopInput::Key(_)) && self.performance_is_playable(event)
            })
    }

    pub fn active_loop(&self) -> Option<(f64, f64)> {
        self.loop_range().filter(|_| self.looping)
    }

    pub fn apply_drum_template(
        &mut self,
        start: usize,
        template: &drum_templates::Template,
        use_groove: bool,
    ) -> bool {
        let Some(end) = template.end_bar(start).filter(|_| start < self.bars) else {
            return false;
        };
        for (bar, template_bar) in (start..end).zip(template.bars) {
            let pattern = template_bar.steps();
            self.hits[bar] = pattern.drums;
            self.step_rules[bar].retain(|&cell, _| usize::from(cell) < 2 * STEPS);
            self.step_sounds[bar].retain(|&cell, _| usize::from(cell) < 2 * STEPS);
            self.step_ids[bar].retain(|&cell, _| usize::from(cell) < 2 * STEPS);
            self.step_feel[bar].retain(|&cell, _| usize::from(cell) < 2 * STEPS);
            self.step_rules[bar].extend(pattern.rules);
        }
        self.bars = self.bars.max(end);
        if use_groove {
            self.bpm = template.bpm;
            self.swing = template.swing;
        }
        true
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedRecord {
    pub record: Record,
    pub studio: Studio,
    pub collection: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_collections: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tags: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default)]
    pub favorite: bool,
}

impl SavedRecord {
    pub fn in_collection(&self, collection: Option<&str>) -> bool {
        collection.is_none_or(|name| {
            self.collection.as_deref() == Some(name)
                || self.other_collections.iter().any(|other| other == name)
        })
    }

    pub fn toggle_collection(&mut self, collection: &str) {
        if self.collection.as_deref() == Some(collection) {
            self.collection = None;
        } else if self.other_collections.iter().any(|name| name == collection) {
            self.other_collections.retain(|name| name != collection);
        } else if self.collection.is_none() {
            self.collection = Some(collection.into());
        } else {
            self.other_collections.push(collection.into());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedSample {
    #[serde(default, skip_serializing_if = "is_primary_source")]
    pub source: usize,
    pub name: String,
    pub start: f64,
    pub end: f64,
}

fn is_primary_source(source: &usize) -> bool {
    *source == 0
}

impl NamedSample {
    pub fn range(&self) -> (f64, f64) {
        (self.start, self.end)
    }

    pub fn validate_list(samples: &mut Vec<Self>, duration: f64) {
        Self::validate_sources(samples, |source| (source == 0).then_some(duration));
    }

    pub fn validate_sources(samples: &mut Vec<Self>, duration: impl Fn(usize) -> Option<f64>) {
        samples.retain_mut(|sample| {
            let Some(duration) = duration(sample.source) else { return false };
            sample.name = sample.name.trim().chars().take(MAX_SAMPLE_NAME).collect();
            sample.start = sample.start.clamp(0.0, duration);
            sample.end = sample.end.clamp(0.0, duration);
            !sample.name.is_empty() && sample.end - sample.start >= MIN_LOOP_SECONDS - 1e-9
        });
        samples.truncate(MAX_NAMED_SAMPLES);
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SampleBank {
    pub source: Source,
    pub samples: Vec<NamedSample>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DawTarget {
    pub path: PathBuf,
    pub enabled: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct DigPreset {
    pub name: String,
    pub genres: [bool; GENRES.len()],
    pub decade: Option<u16>,
    pub max_views: String,
    pub year: String,
    pub country: String,
    pub label: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default, skip_serializing_if = "crate::audio::OutputSettings::is_default")]
    pub audio: crate::audio::OutputSettings,
    #[serde(default)]
    pub interface: crate::ui::preferences::Preferences,
    pub records: Vec<SavedRecord>,
    pub collections: Vec<String>,
    pub heard: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_projects: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dig_presets: Vec<DigPreset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_banks: Vec<SampleBank>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<QueueEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heard_sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daw_targets: Vec<DawTarget>,
    #[serde(default, skip_serializing_if = "crate::midi::Settings::is_empty")]
    pub midi: crate::midi::Settings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_python: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_python: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_python: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sam_python: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sam_ffmpeg: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "crate::recommendations::Profile::is_default")]
    pub taste: crate::recommendations::Profile,
}
