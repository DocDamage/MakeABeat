use crate::model::events::{Cell, NoteId, ResolvedEvent, StepMetadata, Trigger};
use crate::model::instrument::{HitSound, Layers, MAX_LAYERS};
use crate::model::playback::{InstrumentView, PlaybackSettings};
use crate::model::{
    AutomationTarget, ChopInput, DRUMS, KITS, PlayedChop, Quantize, STEPS, SampleControls,
    Sequence, Studio, TICKS_PER_BAR, TICKS_PER_STEP, swung_step,
};
use crate::performance_capture::{self, InputEvent};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering::Relaxed},
    mpsc::{self, Receiver, Sender},
};

pub(crate) mod commands;
mod melody;
mod mono;
#[cfg(all(test, windows))]
mod soak;

pub type Frame = [f32; 2];
pub const RENDER_RATE: u32 = 48_000;
const PEAK_BLOCK: usize = 128;
const SEQUENCED_KEY: u64 = u64::MAX;

#[derive(Clone)]
pub struct Clip {
    pub rate: u32,
    pub frames: Arc<[Frame]>,
    audio_id: Arc<()>,
    pub sources: Vec<Arc<Clip>>,
    pub peaks: Vec<f32>,
    detail_peaks: Vec<Frame>,
}

impl Clip {
    pub fn new(rate: u32, frames: Vec<Frame>) -> Self {
        let bucket = frames.len().div_ceil(960).max(1);
        let peaks = frames.chunks(bucket).map(peak).collect();
        let detail_peaks = frames.chunks(PEAK_BLOCK).map(stereo_peak).collect();
        Self {
            rate,
            frames: frames.into(),
            audio_id: Arc::new(()),
            sources: Vec::new(),
            peaks,
            detail_peaks,
        }
    }

    pub(crate) fn audio_id(&self) -> std::sync::Weak<()> {
        Arc::downgrade(&self.audio_id)
    }

    pub fn source(&self, index: usize) -> Option<&Clip> {
        if index == 0 { Some(self) } else { self.sources.get(index - 1).map(Arc::as_ref) }
    }

    pub fn source_arc(root: &Arc<Self>, index: usize) -> Option<Arc<Clip>> {
        if index == 0 { Some(root.clone()) } else { root.sources.get(index - 1).cloned() }
    }

    pub fn duration(&self) -> f64 {
        self.frames.len() as f64 / f64::from(self.rate)
    }

    pub fn region(&self, (start, end): (f64, f64)) -> Option<&[Frame]> {
        if !start.is_finite() || !end.is_finite() {
            return None;
        }
        let index = |time: f64| {
            (time * f64::from(self.rate))
                .round()
                .clamp(0.0, self.frames.len() as f64) as usize
        };
        let (start, end) = (index(start), index(end));
        (start < end).then(|| &self.frames[start..end])
    }

    pub fn peak_between(&self, start: f64, end: f64) -> f32 {
        let [left, right] = self.stereo_peak_between(start, end);
        left.max(right)
    }

    pub fn stereo_peak_between(&self, start: f64, end: f64) -> Frame {
        let rate = f64::from(self.rate);
        let start = ((start * rate) as usize).min(self.frames.len());
        let end = ((end * rate).ceil() as usize).clamp(start, self.frames.len());
        let first_block = start.div_ceil(PEAK_BLOCK);
        let last_block = end / PEAK_BLOCK;
        if first_block >= last_block {
            return stereo_peak(&self.frames[start..end]);
        }
        let middle = self.detail_peaks[first_block..last_block]
            .iter()
            .copied()
            .fold([0.0; 2], max_channels);
        let left = stereo_peak(&self.frames[start..first_block * PEAK_BLOCK]);
        let right = stereo_peak(&self.frames[last_block * PEAK_BLOCK..end]);
        max_channels(max_channels(middle, left), right)
    }

    pub(crate) fn read(&self, position: f64) -> Frame {
        interpolate(&self.frames, position)
    }
}

fn peak(frames: &[Frame]) -> f32 {
    let [left, right] = stereo_peak(frames);
    left.max(right)
}

fn stereo_peak(frames: &[Frame]) -> Frame {
    frames.iter().fold([0.0; 2], |peaks, frame| max_channels(peaks, frame.map(f32::abs)))
}

fn max_channels(a: Frame, b: Frame) -> Frame {
    [a[0].max(b[0]), a[1].max(b[1])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrospective_capture_keeps_off_grid_positions_across_pattern_wrap() {
        let mut mixer = Mixer::new(960);
        mixer.settings.bpm = 120;
        mixer.settings.pads[0] = Some(0.0);
        mixer.settings.pad_ends[0] = Some(0.9);
        let (input, mut history) = performance_capture::channel();
        mixer.retrospective = Some(input);
        mixer.command(Command::Clip(Some(Arc::new(Clip::new(960, vec![[0.25; 2]; 960])))));
        mixer.command(Command::Pattern(true));
        for _ in 0..1_909 {
            mixer.next_frame();
        }
        mixer.command(Command::Pad(0, PadMode::Gate, 85));
        for _ in 0..24 {
            mixer.next_frame();
        }
        mixer.command(Command::Release(0));
        mixer.command(Command::Pad(0, PadMode::Slice, 110));
        mixer.command(Command::PreviewPerformance(4, 0));
        history.poll();
        let preview = history.preview().unwrap();
        assert!(preview.synced);
        let notes = preview.notes(Quantize::Off, 0.0);
        assert_eq!(notes[0].tick, 2 * TICKS_PER_BAR + 1_909);
        assert_eq!(notes[0].length, Some(24));
        assert_eq!(notes[0].velocity, 85);
        assert_eq!(notes[1].tick, 3 * TICKS_PER_BAR + 13);
        assert_eq!(notes[1].velocity, 110);
        assert!(mixer.settings.performance.is_empty());
    }

    #[test]
    fn lane_timing_advances_and_delays_live_hits_across_the_swung_step() {
        let mut mixer = Mixer::new(1_000);
        mixer.settings.swing = 0.5;
        mixer.settings.hits[0][0][1] = 1;
        mixer.settings.hits[0][1][1] = 1;
        mixer.settings.lane_offsets[2] = -21;
        mixer.settings.lane_offsets[3] = 25;
        mixer.command(Command::Pattern(true));
        let at = |mixer: &mut Mixer, tick| {
            mixer.pattern_tick = tick;
            mixer.advance_pattern();
            mixer.voices.len()
        };
        assert_eq!(at(&mut mixer, 0.0), 0);
        assert_eq!(at(&mut mixer, 128.0), 0);
        assert_eq!(at(&mut mixer, 129.0), 1);
        assert_eq!(mixer.current_step, 0);
        assert_eq!(at(&mut mixer, 150.0), 1);
        assert_eq!(mixer.current_step, 1);
        assert_eq!(at(&mut mixer, 174.0), 1);
        assert_eq!(at(&mut mixer, 175.0), 2);

        let mut studio = mixer.settings;
        studio.lane_offsets[2] = 30;
        studio.swing = 1.0;
        let rule = crate::model::StepRule { repeats: 4, ..Default::default() };
        let times: Vec<_> = studio.step_timing().hit_ticks(rule, 2, 15, 0, false, Default::default()).map(|(tick, _)| tick).collect();
        assert_eq!(times, [1874, 1889, 1904, 1919]);
        studio.lane_offsets[2] = -30;
        assert_eq!(studio.step_timing().hit_ticks(rule, 2, 16, 0, false, Default::default()).next().unwrap().0, 1920);
    }

    #[test]
    fn beat_audition_joins_the_next_bar_and_preserves_playback() {
        let mut mixer = Mixer::new(1_000);
        mixer.settings.bpm = 120;
        mixer.settings.drum_level = 0.2;
        mixer.settings.hits[0][0][0] = 1;
        let settings = mixer.settings.clone();
        let mut reference = Mixer::new(1_000);
        reference.settings = settings.clone();
        for mixer in [&mut mixer, &mut reference] {
            mixer.command(Command::Pattern(true));
        }
        let clip = Arc::new(Clip::new(1_000, vec![[0.4; 2]; 2_000]));
        mixer.command(Command::BeatAuditionGain(0.5));
        for frame in 0..2_000 {
            if frame == 500 {
                mixer.command(Command::BeatAudition(crate::audition::Voice::new(
                    1, clip.clone(), (0.0, 2.0), Some(1),
                )));
            }
            assert_eq!(mixer.next_frame(), reference.next_frame(), "frame {frame}");
        }
        let joined = mixer.next_frame();
        let beat = reference.next_frame();
        assert!((joined[0] - beat[0] - 0.2).abs() < 1e-6);
        assert_eq!(mixer.current_step, 0);
        assert_eq!(mixer.settings, settings);
        for _ in 0..1_999 {
            mixer.next_frame();
            reference.next_frame();
        }
        assert!((mixer.next_frame()[0] - reference.next_frame()[0] - 0.2).abs() < 1e-6);
        let mut faster = settings;
        faster.bpm = 150;
        mixer.command(Command::Settings(Box::new(faster)));
        assert!(mixer.audition_active.is_none() && mixer.pattern);
        mixer.command(Command::BeatAudition(crate::audition::Voice::new(2, clip.clone(), (0.0, 2.0), None)));
        assert!(mixer.audition_pending.is_some());
        mixer.command(Command::ClockPosition(16));
        assert!(mixer.audition_pending.is_none() && mixer.audition_active.is_none());

        mixer.command(Command::ClockMode(true));
        mixer.command(Command::BeatAudition(crate::audition::Voice::new(3, clip.clone(), (0.0, 2.0), Some(1))));
        assert!(mixer.audition_pending.is_none());
        mixer.command(Command::BeatAudition(crate::audition::Voice::new(4, clip, (0.0, 2.0), None)));
        mixer.command(Command::ClockStart(false));
        mixer.command(Command::ClockPulse(0.02));
        mixer.next_frame();
        assert_eq!(mixer.audition_active.as_ref().map(|voice| voice.serial), Some(4));
        mixer.command(Command::ClockPosition(16));
        assert!(mixer.audition_active.is_none());
    }

    #[test]
    fn scene_switch_waits_for_bar_and_captures_completed_scene() {
        use crate::model::SequenceBar;
        let mut mixer = Mixer::new(1_000);
        let (capture, received) = mpsc::channel();
        mixer.capture = Some(capture);
        let sequence = || Arc::new(Sequence {
            bars: vec![SequenceBar::default(); 2],
            ..Sequence::default()
        });
        mixer.command(Command::LaunchScene(2, sequence(), 1));
        mixer.tick();
        assert_eq!(mixer.scene.as_ref().map(|scene| scene.pattern), Some(2));

        mixer.command(Command::LaunchScene(3, sequence(), 1));
        for _ in 1..STEPS {
            mixer.tick();
        }
        assert_eq!(mixer.scene.as_ref().map(|scene| scene.pattern), Some(2));
        assert!(received.try_recv().is_err());

        mixer.tick();
        assert_eq!(mixer.scene.as_ref().map(|scene| scene.pattern), Some(3));
        assert!(matches!(
            received.try_recv().unwrap(),
            CapturedChop::SceneBar { pattern: 2, offset: 0 }
        ));
    }

    #[test]
    fn stereo_peaks_keep_channels_separate_across_cached_blocks() {
        let mut frames = vec![[0.1, 0.2]; PEAK_BLOCK * 3];
        frames[PEAK_BLOCK] = [-0.9, 0.2];
        frames[PEAK_BLOCK * 2 + 1] = [0.1, -0.7];
        let clip = Clip::new(1024, frames);
        assert_eq!(clip.stereo_peak_between(1.0 / 1024.0, 0.30), [0.9, 0.7]);
        assert_eq!(clip.peak_between(0.0, 0.1), 0.2);
    }

    #[test]
    fn automated_mix_effects_filter_samples_and_leave_delay_and_reverb_tails() {
        use crate::model::{AutomationPoint, Section, SectionKind};
        let mut studio = Studio {
            bpm: 240,
            ..Studio::default()
        };
        studio.pads[0] = Some(0.0);
        studio.chops[0][0] = Some(0);
        studio.save_pattern("Hook", false);
        studio.sections.push(Section::new(SectionKind::Hook, 0, 1));
        for (target, percent) in [
            (AutomationTarget::LowPass, 0),
            (AutomationTarget::Delay, 100),
            (AutomationTarget::Reverb, 100),
        ] {
            studio
                .automation
                .set_points(target, vec![AutomationPoint { tick: 0, percent, curve: Default::default() }]);
        }
        let rate = 8_000;
        let clip = Arc::new(Clip::new(
            rate,
            (0..160)
                .map(|i| [if i % 2 == 0 { 0.4 } else { -0.4 }; 2])
                .collect(),
        ));
        let render = |arranged| {
            let mut mixer = Mixer::new(rate);
            mixer.settings = (&studio).into();
            mixer.command(Command::Clip(Some(clip.clone())));
            if arranged {
                mixer.command(Command::Song(Arc::new(studio.song_sequence().unwrap()), 1));
            } else {
                mixer.command(Command::Pattern(true));
            }
            (0..1_700)
                .map(|_| mixer.next_stems().0[0])
                .collect::<Vec<_>>()
        };
        let dry = render(false);
        let wet = render(true);
        assert!(dry[100].abs() > 0.35 && wet[100].abs() < 0.01);
        assert!(dry[160..].iter().all(|&value| value == 0.0));
        assert!(wet[320..430].iter().any(|value| value.abs() > 0.001));
        assert!(wet[1_530..1_640].iter().any(|value| value.abs() > 0.001));
    }

    #[test]
    fn song_plays_past_the_pattern_limit_and_stops_without_wrapping() {
        use crate::model::{MAX_BARS, SequenceBar};
        let mut mixer = Mixer::new(1_000);
        mixer.settings.bpm = 240;
        let song = Sequence {
            bars: vec![SequenceBar::default(); MAX_BARS + 1],
            ..Sequence::default()
        };
        mixer.command(Command::Song(Arc::new(song), 23));
        for _ in 0..MAX_BARS * 1_000 + 1 {
            mixer.next_frame();
        }
        assert_eq!(mixer.current_step, (MAX_BARS * STEPS) as i32);
        assert!(mixer.pattern);
        for _ in 0..1_000 {
            mixer.next_frame();
        }
        assert!(!mixer.pattern);
        assert_eq!(mixer.current_step, -1);
        assert_eq!(mixer.finished_song, 23);
    }

    #[test]
    fn audition_stops_at_its_end_without_changing_the_loop() {
        let rate = 8_000;
        let mut mixer = Mixer::new(rate);
        mixer.command(Command::Clip(Some(Arc::new(Clip::new(
            rate,
            vec![[0.4; 2]; 8_000],
        )))));
        mixer.settings.loop_in = Some(0.1);
        mixer.settings.loop_out = Some(0.9);
        mixer.settings.looping = true;
        mixer.command(Command::Audition(0.25, 0.375));
        let mut heard_audio = false;
        for _ in 0..1_000 {
            heard_audio |= mixer.next_frame()[0] > 0.3;
        }
        assert!(heard_audio);
        for _ in 0..3 {
            assert!(mixer.next_frame()[0].abs() < 1e-9);
        }
        assert_eq!(mixer.settings.active_loop(), Some((0.1, 0.9)));
    }

    #[test]
    fn independent_chops_play_together_and_gate_release_only_stops_one() {
        let rate = 8_000;
        let mut mixer = Mixer::new(rate);
        let mut frames = vec![[0.4, 0.4]; 8_000];
        frames.extend(vec![[0.25, 0.25]; 8_000]);
        mixer.command(Command::Clip(Some(Arc::new(Clip::new(rate, frames)))));
        for (note, range, pan) in [(21, (0.0, 0.95), -1.0), (108, (1.01, 2.0), 1.0)] {
            mixer.command(Command::PianoOn {
                source: 18,
                note,
                velocity: 127,
                bend: 0.0,
                range,
                controls: SampleControls {
                    pan,
                    release_ms: 15,
                    ..SampleControls::default()
                },
                gate: true,
            });
        }
        for _ in 0..30 {
            mixer.next_frame();
        }
        assert_eq!(mixer.next_frame(), [0.4, 0.25]);
        mixer.command(Command::PianoOff(18, 21, true));
        for _ in 0..200 {
            mixer.next_frame();
        }
        assert_eq!(mixer.next_frame(), [0.4, 0.25]);
        mixer.command(Command::PianoPedalUp(18));
        for _ in 0..200 {
            mixer.next_frame();
        }
        assert_eq!(mixer.next_frame(), [0.0, 0.25]);

        mixer.command(Command::PianoSilence(None));
        mixer.settings.pads[0] = Some(0.0);
        mixer.settings.pad_controls[0] = SampleControls {
            reverse: true,
            tune: 12.0,
            pan: -1.0,
            ..SampleControls::default()
        };
        mixer.command(Command::Pad(0, PadMode::Slice, 127));
        for _ in 0..500 {
            mixer.next_frame();
        }
        assert!((mixer.position() - 1.875).abs() < 0.001);
        assert_eq!(mixer.next_frame(), [0.25, 0.0]);
    }

    #[test]
    fn midi_clock_waits_for_pulses_and_continues_from_song_position() {
        let mut mixer = Mixer::new(8_000);
        mixer.command(Command::ClockMode(true));
        mixer.command(Command::ClockStart(false));
        for _ in 0..2_000 {
            mixer.next_frame();
        }
        assert_eq!(mixer.current_step, -1);
        for index in 0..7 {
            mixer.command(Command::ClockPulse(0.0208));
            mixer.next_frame();
            assert_eq!(mixer.current_step, if index < 6 { 0 } else { 1 });
        }
        mixer.command(Command::Pattern(false));
        mixer.command(Command::ClockPosition(21));
        mixer.command(Command::ClockStart(true));
        mixer.command(Command::ClockPulse(0.0208));
        mixer.next_frame();
        assert_eq!(mixer.current_step, 5);
    }

    #[test]
    fn records_off_grid_gate_length_and_polyphonic_key_velocity() {
        let mut mixer = Mixer::new(8_000);
        mixer.settings.bpm = 120;
        mixer.settings.pads[0] = Some(0.0);
        mixer.settings.pad_controls[0].attack_ms = 0;
        let (send, take) = mpsc::channel();
        mixer.capture = Some(send);
        mixer.command(Command::Clip(Some(Arc::new(Clip::new(
            8_000,
            vec![[0.4; 2]; 8_000],
        )))));
        mixer.command(Command::Record(Some((Quantize::Off, 1))));
        mixer.command(Command::Pattern(true));
        for _ in 0..725 {
            mixer.next_frame();
        }
        mixer.command(Command::Pad(0, PadMode::Gate, 88));
        let CapturedChop::Start(1, id, pad) = take.try_recv().unwrap() else {
            panic!("missing pad onset")
        };
        assert_eq!(
            (pad.tick, pad.input, pad.velocity),
            (87, ChopInput::Pad(0), 88)
        );
        for _ in 0..275 {
            mixer.next_frame();
        }
        mixer.command(Command::Release(0));
        let CapturedChop::End(1, released, pad) = take.try_recv().unwrap() else {
            panic!("missing pad release")
        };
        assert_eq!(released, id);
        assert_eq!(pad.length, Some(33));
        for _ in 0..130 {
            mixer.next_frame();
        }
        for (note, velocity) in [(60, 71), (64, 109)] {
            mixer.command(Command::PianoOn {
                source: 18,
                note,
                velocity,
                bend: 0.0,
                range: (0.0, 1.0),
                controls: SampleControls::default(),
                gate: false,
            });
            let CapturedChop::Start(1, _, event) = take.try_recv().unwrap() else {
                panic!("missing key onset")
            };
            assert_eq!(
                (event.tick, event.input, event.velocity),
                (136, ChopInput::Key(note), velocity)
            );
        }
        mixer.command(Command::Pattern(false));
        mixer.command(Command::Clip(mixer.clip.clone()));
        mixer.settings.performance = vec![pad];
        mixer.command(Command::Pattern(true));
        assert!((0..725).all(|_| mixer.next_frame() == [0.0; 2]));
        assert!(mixer.next_frame()[0] > 0.1);
        for _ in 0..280 {
            mixer.next_frame();
        }
        assert!(mixer.chop.as_ref().is_some_and(|chop| chop.voices.head().is_some_and(|layer| layer.voice.releasing)));
    }

    #[test]
    fn quantized_overdub_does_not_retrigger_before_the_next_loop() {
        let mut mixer = Mixer::new(8_000);
        mixer.settings.bpm = 120;
        mixer.settings.swing = 1.0;
        mixer.settings.pads[0] = Some(0.0);
        let (send, take) = mpsc::channel();
        mixer.capture = Some(send);
        mixer.command(Command::Clip(Some(Arc::new(Clip::new(
            8_000,
            vec![[0.4; 2]; 8_000],
        )))));
        mixer.command(Command::Record(Some((Quantize::Sixteenth, 1))));
        mixer.command(Command::Pattern(true));
        for _ in 0..1_150 {
            mixer.next_frame();
        }
        mixer.command(Command::Pad(0, PadMode::Slice, 127));
        let CapturedChop::Start(1, _, pad) = take.try_recv().unwrap() else {
            panic!("missing quantized pad")
        };
        assert_eq!(pad.tick, 180);
        mixer.settings.performance = vec![pad];
        for _ in 1_150..1_505 {
            mixer.next_frame();
        }
        assert!(mixer.position() > 0.04);
        for _ in 1_505..17_505 {
            mixer.next_frame();
        }
        assert!(mixer.chop.is_some() && mixer.position() < 0.001);
    }
}

fn interpolate(frames: &[Frame], position: f64) -> Frame {
    let index = position as usize;
    let Some(&a) = frames.get(index) else {
        return [0.0; 2];
    };
    let b = frames.get(index + 1).copied().unwrap_or(a);
    let fraction = position.fract() as f32;
    [
        a[0] + (b[0] - a[0]) * fraction,
        a[1] + (b[1] - a[1]) * fraction,
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadMode {
    #[default]
    Slice,
    Gate,
    Jump,
}

pub enum Command {
    Clip(Option<Arc<Clip>>),
    PreviewSource(usize),
    InputMonitor(Option<crate::input_audio::Monitor>),
    MidiOutput(Option<Box<crate::midi_out::Sink>>),
    Settings(Box<PlaybackSettings>),
    Play(bool),
    Seek(f64),
    Audition(f64, f64),
    Listen(Arc<Clip>, (f64, f64), f32, Option<(f64, f64)>),
    StopListening,
    BeatAudition(crate::audition::Voice),
    BeatAuditionGain(f32),
    StopBeatAudition,
    StopAll,
    Pattern(bool),
    Song(Arc<Sequence>, u64),
    SongAutomation(u64, Arc<Sequence>),
    SongWindow(Arc<Sequence>, u64, std::ops::Range<usize>, bool),
    LaunchScene(usize, Arc<Sequence>, usize),
    Record(Option<(Quantize, u64)>),
    RecordRange(Option<std::ops::Range<u32>>),
    PreviewPerformance(usize, u64),
    CountIn(u8),
    ClickGain(f32),
    ClickTrack(bool),
    Fill(bool),
    ClockMode(bool),
    ClockStart(bool),
    ClockPosition(u16),
    ClockPulse(f64),
    Pad(usize, PadMode, u8),
    Release(usize),
    Drum(usize),
    MidiDrum(usize, u8),
    PianoOn {
        source: u64,
        note: u8,
        velocity: u8,
        bend: f64,
        range: (f64, f64),
        controls: SampleControls,
        gate: bool,
    },
    PianoOff(u64, u8, bool),
    PianoPedalUp(u64),
    PianoBend(u64, f64),
    PianoSilence(Option<u64>),
}

#[derive(Default)]
pub struct Meters {
    #[cfg(all(test, windows))]
    pub callback_frames: AtomicUsize,
    #[cfg(all(test, windows))]
    pub callback_peak_ns: AtomicU64,
    #[cfg(all(test, windows))]
    callback_over_budget: AtomicU64,
    #[cfg(all(test, windows))]
    soak_probe: std::sync::OnceLock<soak::RenderProbe>,
    position: AtomicU64,
    step: AtomicI32,
    preview: AtomicBool,
    listening: AtomicBool,
    beat_audition: AtomicUsize,
    listening_frames: AtomicU64,
    beat_audition_frames: AtomicU64,
    audition_watermark: AtomicU64,
    failed: AtomicBool,
    finished_song: AtomicU64,
    scene: AtomicUsize,
    count_in: AtomicUsize,
    peak: AtomicU64,
}

impl Meters {
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Relaxed))
    }
    pub fn step(&self) -> Option<usize> {
        usize::try_from(self.step.load(Relaxed)).ok()
    }
    pub fn preview(&self) -> bool {
        self.preview.load(Relaxed)
    }
    pub fn listening(&self) -> bool {
        self.listening.load(Relaxed)
    }
    pub fn audition_frames(&self, with_beat: bool) -> u64 {
        if with_beat { self.beat_audition_frames.load(Relaxed) } else { self.listening_frames.load(Relaxed) }
    }
    pub fn beat_audition(&self) -> usize {
        self.beat_audition.load(Relaxed)
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Relaxed)
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Relaxed) as u32)
    }

    pub fn count_in_beats(&self) -> usize {
        self.count_in.load(Relaxed)
    }

    pub fn scene(&self) -> Option<usize> {
        self.scene.load(Relaxed).checked_sub(1)
    }

    pub fn finished_song(&self) -> u64 {
        self.finished_song.load(Relaxed)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    pub device: Option<String>,
    pub rate: Option<u32>,
    pub buffer: Option<u32>,
}

impl OutputSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

pub fn output_devices() -> Vec<String> {
    let mut names = cpal::default_host().output_devices()
        .map(|devices| devices.filter_map(|device| device.name().ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

pub struct Output {
    _stream: Stream,
    tx: commands::CommandSender,
    retired_settings: Receiver<Box<PlaybackSettings>>,
    recorded: Receiver<CapturedChop>,
    pub performance: performance_capture::History,
    pub meters: Arc<Meters>,
    pub device_name: String,
    pub sample_rate: u32,
    pub requested_buffer: Option<u32>,
    audition_serial: std::cell::Cell<u64>,
    audition_clips: std::cell::RefCell<Vec<(u64, Arc<Clip>)>>,
}

impl Output {
    pub fn open(settings: &OutputSettings) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = if let Some(name) = &settings.device {
            host.output_devices().map_err(|error| error.to_string())?
                .find(|device| device.name().ok().as_ref() == Some(name))
                .ok_or_else(|| format!("Output device {name} is unavailable."))?
        } else {
            host.default_output_device().ok_or("No audio output device")?
        };
        let device_name = device.name().unwrap_or_else(|_| "Audio output".into());
        let default = device.default_output_config().map_err(|error| error.to_string())?;
        let format = if let Some(rate) = settings.rate.filter(|&rate| rate != default.sample_rate().0) {
            device.supported_output_configs().map_err(|error| error.to_string())?
                .filter_map(|range| range.try_with_sample_rate(cpal::SampleRate(rate)))
                .find(|config| config.sample_format() == default.sample_format() && config.channels() == default.channels())
                .ok_or_else(|| format!("{device_name} does not support {rate} Hz in its default channel format."))?
        } else {
            default
        };
        let mut config = format.config();
        if let Some(frames) = settings.buffer {
            if frames == 0 {
                return Err("Audio buffer must contain at least one frame.".into());
            }
            if let cpal::SupportedBufferSize::Range { min, max } = format.buffer_size()
                && !(*min..=*max).contains(&frames)
            {
                return Err(format!("{device_name} supports buffers from {min} to {max} frames."));
            }
            config.buffer_size = cpal::BufferSize::Fixed(frames);
        }
        let (tx, rx, retired_settings) = commands::channel();
        let (capture, recorded) = mpsc::channel();
        let (performance_input, performance) = performance_capture::channel();
        let meters = Arc::new(Meters::default());
        meters.step.store(-1, Relaxed);
        let stream = match format.sample_format() {
            SampleFormat::F32 => output_stream::<f32>(&device, &config, rx, &meters, capture, performance_input),
            SampleFormat::F64 => output_stream::<f64>(&device, &config, rx, &meters, capture, performance_input),
            SampleFormat::I16 => output_stream::<i16>(&device, &config, rx, &meters, capture, performance_input),
            SampleFormat::U16 => output_stream::<u16>(&device, &config, rx, &meters, capture, performance_input),
            SampleFormat::I32 => output_stream::<i32>(&device, &config, rx, &meters, capture, performance_input),
            other => return Err(format!("Unsupported audio output format: {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            _stream: stream,
            tx,
            retired_settings,
            recorded,
            performance,
            meters,
            device_name,
            sample_rate: config.sample_rate.0,
            requested_buffer: settings.buffer,
            audition_serial: Default::default(),
            audition_clips: Default::default(),
        })
    }

    pub fn send(&self, command: Command) {
        if matches!(&command, Command::Clip(_)) {
            self.performance.invalidate_preview();
        }
        self.tx.send(command);
    }

    pub fn queue_beat_audition(&self, clip: Arc<Clip>, range: (f64, f64), bars: Option<usize>) {
        let serial = self.audition_serial.get() + 1;
        self.audition_serial.set(serial);
        self.audition_clips.borrow_mut().push((serial, clip.clone()));
        self.send(Command::BeatAudition(crate::audition::Voice::new(serial, clip, range, bars)));
    }

    pub fn retire_beat_auditions(&self) {
        // Keep large buffers alive until the callback has released its references.
        let watermark = self.meters.audition_watermark.load(std::sync::atomic::Ordering::Acquire);
        self.audition_clips.borrow_mut().retain(|(serial, _)| *serial >= watermark);
    }

    pub(crate) fn sender(&self) -> commands::CommandSender {
        self.tx.clone()
    }

    pub fn retire_settings(&self) {
        self.retired_settings.try_iter().for_each(drop);
    }

    pub fn recorded(&self) -> Option<CapturedChop> {
        self.recorded.try_recv().ok()
    }
}

fn output_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut rx: commands::CommandReceiver,
    meters: &Arc<Meters>,
    capture: Sender<CapturedChop>,
    performance: performance_capture::Input,
) -> Result<Stream, cpal::BuildStreamError> {
    let channels = usize::from(config.channels);
    #[cfg(all(test, windows))]
    let config_rate = config.sample_rate.0;
    let mut mixer = Mixer::new(config.sample_rate.0);
    mixer.capture = Some(capture);
    mixer.retrospective = Some(performance);
    let status = Arc::clone(meters);
    let error_status = Arc::clone(meters);
    #[cfg(all(test, windows))]
    let mut diagnostic = soak::RenderState::new(config.sample_rate.0);
    #[cfg(windows)]
    let mut promote = true;
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            #[cfg(windows)]
            if std::mem::take(&mut promote) {
                crate::platform::promote_audio_thread();
            }
            #[cfg(all(test, windows))]
            let started = std::time::Instant::now();
            #[cfg(all(test, windows))]
            let probe = status.soak_probe.get();
            #[cfg(all(test, windows))]
            let cycles = probe.map(|_| soak::thread_cycles());
            rx.apply(&mut mixer);
            #[cfg(all(test, windows))]
            let commands_elapsed = started.elapsed();
            let mut peak = 0.0_f32;
            for output in data.chunks_exact_mut(channels) {
                let [left, right] = mixer.next_frame();
                #[cfg(all(test, windows))]
                let (left, right) = if let Some(probe) = probe {
                    diagnostic.add_reference(probe, left, right)
                } else {
                    (left, right)
                };
                peak = peak.max(mixer.unlimited_peak);
                output[0] = T::from_sample(if channels == 1 {
                    (left + right) * 0.5
                } else {
                    left
                });
                if channels > 1 {
                    output[1] = T::from_sample(right);
                }
                for channel in &mut output[2.min(channels)..] {
                    *channel = T::from_sample(0.0);
                }
            }
            #[cfg(all(test, windows))]
            let mix_elapsed = started.elapsed();
            if let Some(capture) = &mixer.retrospective {
                capture.publish();
            }
            status.peak.store(u64::from(peak.to_bits()), Relaxed);
            status.position.store(mixer.position().to_bits(), Relaxed);
            status.step.store(mixer.current_step, Relaxed);
            status.finished_song.store(mixer.finished_song, Relaxed);
            status.scene.store(mixer.scene.as_ref().map_or(0, |scene| scene.pattern + 1), Relaxed);
            status.count_in.store(mixer.count_in.div_ceil(mixer.count_in_beat.max(1)), Relaxed);
            let auditioning = mixer.chop.as_ref().is_some_and(|chop| chop.pad.is_none());
            status.preview.store((mixer.preview && !mixer.pattern_preview) || auditioning, Relaxed);
            status.listening.store(mixer.listening.is_some(), Relaxed);
            let pending = mixer.audition_pending.as_ref().map(|voice| voice.serial);
            let active = mixer.audition_active.as_ref().map(|voice| voice.serial);
            let watermark = pending.into_iter().chain(active).min().unwrap_or(mixer.audition_serial + 1);
            status.beat_audition.store(if pending.is_some() { 1 } else { usize::from(active.is_some()) * 2 }, Relaxed);
            status.listening_frames.store(mixer.listening_frames, Relaxed);
            status.beat_audition_frames.store(mixer.beat_audition_frames, Relaxed);
            status.audition_watermark.store(watermark, std::sync::atomic::Ordering::Release);
            #[cfg(all(test, windows))]
            {
                let frames = data.len() / channels;
                let elapsed_ns = started.elapsed().as_nanos() as u64;
                status.callback_frames.store(frames, Relaxed);
                status.callback_peak_ns.fetch_max(elapsed_ns, Relaxed);
                if elapsed_ns * u64::from(config_rate) > frames as u64 * 1_000_000_000 {
                    status.callback_over_budget.fetch_add(1, Relaxed);
                }
                if let (Some(probe), Some(cycles)) = (probe, cycles) {
                    diagnostic.callback(probe, soak::CallbackMeasurement {
                        started,
                        frames,
                        rate: config_rate,
                        commands_elapsed,
                        mix_elapsed,
                        elapsed_ns,
                        cycles_start: cycles,
                    });
                }
            }
        },
        move |_| error_status.failed.store(true, Relaxed),
        None,
    )
}

struct Chop {
    pad: Option<usize>,
    mode: PadMode,
    voices: SampleStack,
    choke: Choke,
}

impl Chop {
    fn frame(&mut self, clip: &Clip, rate: f64, choke_frames: u32) -> Option<Frame> {
        let gain = self.choke.gain(choke_frames);
        if gain == 0.0 {
            return None;
        }
        self.voices.frame(clip, rate).map(|frame| frame.map(|sample| sample * gain))
    }
}

struct DrumVoice {
    clip_source: Option<usize>,
    rate_multiplier: f64,
    track: usize,
    kit: usize,
    position: f64,
    gain: f32,
    choke: Choke,
    filter: Option<HitFilter>,
    started: bool,
}

struct Choke {
    group: u8,
    remaining: Option<u32>,
}

impl Choke {
    fn new(group: u8) -> Self {
        Self { group, remaining: None }
    }

    fn release(&mut self, group: u8, frames: u32, started: bool) -> bool {
        if group != self.group || self.remaining.is_some() {
            return false;
        }
        self.remaining = Some(if started { frames } else { 0 });
        true
    }

    fn gain(&mut self, frames: u32) -> f32 {
        self.remaining.as_mut().map_or(1.0, |remaining| {
            let gain = *remaining as f32 / frames as f32;
            *remaining = remaining.saturating_sub(1);
            gain
        })
    }
}

struct PianoVoice {
    source: u64,
    note: u8,
    held: bool,
    gate: bool,
    voices: SampleStack,
}

struct SourceVoice {
    source: usize,
    voice: SampleVoice,
}

#[derive(Clone, Copy)]
struct SampleAssignment {
    source: usize,
    range: (f64, f64),
    controls: SampleControls,
    layers: Layers,
    transpose: f32,
}

struct SampleStack {
    layers: [Option<SourceVoice>; MAX_LAYERS],
    release_at: Option<u64>,
    started: bool,
}

impl SampleStack {
    fn new(
        assignment: SampleAssignment, speed: f64, velocity: u8, bend: f64,
        sound: HitSound, rate: f64,
    ) -> Self {
        let SampleAssignment { source, range, controls, layers, transpose } = assignment;
        let mut voices = std::array::from_fn(|_| None);
        let make = |source, range, mut controls: SampleControls| {
            controls.tune += f32::from(sound.pitch_cents) / 100.0;
            let mut voice = SampleVoice::new(range, controls, speed, velocity, bend);
            let skip = (range.1 - range.0) * f64::from(sound.start_percent) / 100.0;
            voice.position = if controls.reverse { (voice.position - skip).max(range.0) }
                else { voice.position + skip };
            voice.filter = HitFilter::new(sound.cutoff_hz, rate);
            SourceVoice { source, voice }
        };
        if layers.base.contains(velocity) {
            voices[0] = Some(make(source, range, controls));
        }
        for (slot, layer) in voices[1..].iter_mut().zip(layers.extra) {
            if let Some(layer) = layer.filter(|layer| layer.velocity.contains(velocity)) {
                let mut controls = layer.controls;
                controls.tune += transpose;
                *slot = Some(make(layer.source, (layer.start, layer.end), controls));
            }
        }
        Self { layers: voices, release_at: None, started: false }
    }

    fn head(&self) -> Option<&SourceVoice> {
        self.layers.iter().flatten().next()
    }

    fn releasing(&mut self, value: bool) {
        for layer in self.layers.iter_mut().flatten() {
            layer.voice.releasing = value;
        }
    }

    fn bend(&mut self, bend: f64) {
        for layer in self.layers.iter_mut().flatten() {
            layer.voice.bend(bend);
        }
    }

    fn advance_gate(&mut self, tick: u64) {
        if self.release_at.is_some_and(|end| tick >= end) {
            self.releasing(true);
        }
    }

    fn frame(&mut self, clip: &Clip, rate: f64) -> Option<Frame> {
        self.frame_at(clip, rate, 1.0)
    }

    fn continue_from(&mut self, previous: &Self) {
        for (next, previous) in self.layers.iter_mut().zip(&previous.layers) {
            if let (Some(next), Some(previous)) = (next, previous)
                && next.source == previous.source
                && next.voice.range == previous.voice.range
                && next.voice.controls.reverse == previous.voice.controls.reverse
                && !previous.voice.releasing
                && (next.voice.range.0..next.voice.range.1).contains(&previous.voice.position)
            {
                next.voice.position = previous.voice.position;
                next.voice.age = previous.voice.age;
                self.started = true;
            }
        }
    }

    fn frame_at(&mut self, clip: &Clip, rate: f64, ratio: f64) -> Option<Frame> {
        let mut mix: Option<Frame> = None;
        for slot in &mut self.layers {
            let frame = slot.as_mut().and_then(|layer| {
                layer.voice.frame_at(clip.source(layer.source)?, rate, ratio)
            });
            if let Some(frame) = frame {
                mix = Some(mix.map_or(frame, |mix| [mix[0] + frame[0], mix[1] + frame[1]]));
                self.started = true;
            } else {
                *slot = None;
            }
        }
        mix
    }
}

struct SampleVoice {
    base_step: f64,
    step: f64,
    position: f64,
    range: (f64, f64),
    age: f32,
    gain: Frame,
    release: f32,
    releasing: bool,
    controls: SampleControls,
    filter: Option<HitFilter>,
}

struct HitFilter {
    coefficient: f32,
    state: Frame,
}

impl HitFilter {
    fn new(hz: Option<u16>, rate: f64) -> Option<Self> {
        let cutoff = f64::from(hz?).min(rate * 0.49);
        let coefficient = (1.0 - (-std::f64::consts::TAU * cutoff / rate).exp()) as f32;
        Some(Self { coefficient, state: [0.0; 2] })
    }

    fn apply(&mut self, frame: Frame) -> Frame {
        for (value, input) in self.state.iter_mut().zip(frame) {
            *value += self.coefficient * (input - *value);
        }
        self.state
    }
}

impl SampleVoice {
    fn new(
        range: (f64, f64),
        controls: SampleControls,
        base_step: f64,
        velocity: u8,
        bend: f64,
    ) -> Self {
        let step = base_step * 2.0_f64.powf((f64::from(controls.tune) + bend) / 12.0);
        let gain = (f32::from(velocity) / 127.0).powf(1.4) * controls.gain;
        Self {
            base_step,
            step,
            range,
            controls,
            filter: None,
            position: if controls.reverse {
                (range.1 - step).max(range.0)
            } else {
                range.0
            },
            age: 0.0,
            gain: [
                gain * (1.0 - controls.pan.max(0.0)),
                gain * (1.0 + controls.pan.min(0.0)),
            ],
            release: 1.0,
            releasing: false,
        }
    }

    fn bend(&mut self, semitones: f64) {
        self.step =
            self.base_step * 2.0_f64.powf((f64::from(self.controls.tune) + semitones) / 12.0);
    }

    fn frame(&mut self, clip: &Clip, rate: f64) -> Option<Frame> {
        self.frame_at(clip, rate, 1.0)
    }

    fn frame_at(&mut self, clip: &Clip, rate: f64, ratio: f64) -> Option<Frame> {
        let remaining = if self.controls.reverse {
            self.position - self.range.0
        } else {
            self.range.1 - self.position
        };
        if remaining <= 0.0 {
            return None;
        }
        let mut sample = clip.read(self.position * f64::from(clip.rate));
        if let Some(filter) = &mut self.filter {
            sample = filter.apply(sample);
        }
        let step = self.step * ratio;
        let end_fade = (remaining * 500.0 / (step * rate)).min(1.0) as f32;
        self.position += if self.controls.reverse {
            -step
        } else {
            step
        };
        let attack = if self.controls.attack_ms == 0 {
            1.0
        } else {
            (self.age * 1000.0 / f32::from(self.controls.attack_ms)).min(1.0)
        };
        self.age += (1.0 / rate) as f32;
        if self.releasing {
            if self.controls.release_ms == 0 {
                return None;
            }
            self.release -= (1000.0 / rate) as f32 / f32::from(self.controls.release_ms);
        }
        if self.release <= 0.0 {
            return None;
        }
        let envelope = attack * end_fade * self.release;
        Some([
            sample[0] * self.gain[0] * envelope,
            sample[1] * self.gain[1] * envelope,
        ])
    }
}

pub enum CapturedChop {
    SceneBar { pattern: usize, offset: usize },
    Start(u64, u64, PlayedChop),
    End(u64, u64, PlayedChop),
}

fn pad_assignment(clip: &Clip, studio: InstrumentView<'_>, pad: usize, mode: PadMode) -> Option<SampleAssignment> {
    let source = studio.pad_sources[pad];
    let duration = clip.source(source)?.duration();
    let start = studio.pads[pad].filter(|&time| time < duration)?;
    let controls = studio.pad_controls[pad];
    let end = if mode == PadMode::Slice
        || controls.reverse
        || (mode == PadMode::Gate && studio.pad_ends[pad].is_some())
    {
        studio.slice(pad, duration).map_or(duration, |range| range.1)
    } else {
        duration
    };
    Some(SampleAssignment {
        source, range: (start, end), controls, layers: studio.pad_layers(pad), transpose: 0.0,
    })
}

pub fn pad_frames<'a>(
    clip: &'a Clip, studio: &Studio, pad: usize, velocity: u8, rate: u32,
) -> Option<impl Iterator<Item = Frame> + 'a> {
    let assignment = pad_assignment(clip, studio.instrument_view(), pad, PadMode::Slice)?;
    let mut stack = SampleStack::new(assignment, studio.speed / f64::from(rate), velocity, 0.0, HitSound::default(), f64::from(rate));
    stack.head()?;
    Some(std::iter::from_fn(move || stack.frame(clip, f64::from(rate))))
}

fn key_assignment(
    studio: InstrumentView<'_>, note: u8, range: (f64, f64), controls: SampleControls,
) -> SampleAssignment {
    let transpose = studio.chromatic.filter(|_| studio.chromatic_enabled)
        .map_or(0.0, |instrument| f32::from(note) - f32::from(instrument.root));
    SampleAssignment {
        source: studio.key_source(note), range, controls,
        layers: studio.key_layers(note), transpose,
    }
}

pub fn key_frames<'a>(
    clip: &'a Clip, studio: &Studio, note: u8, velocity: u8, rate: u32,
) -> Option<impl Iterator<Item = Frame> + 'a> {
    let range = studio.key_range(note, clip.source(studio.key_source(note))?.duration())?;
    let assignment = key_assignment(studio.instrument_view(), note, range, studio.controls_for_key(note));
    let mut stack = SampleStack::new(assignment, studio.speed / f64::from(rate), velocity, 0.0, HitSound::default(), f64::from(rate));
    stack.head()?;
    Some(std::iter::from_fn(move || stack.frame(clip, f64::from(rate))))
}

pub fn cut_frames(
    clip: &Clip, range: (f64, f64), controls: SampleControls, speed: f64, rate: u32,
) -> impl Iterator<Item = Frame> + '_ {
    let mut voice = SampleVoice::new(range, controls, speed / f64::from(rate), 127, 0.0);
    std::iter::from_fn(move || voice.frame(clip, f64::from(rate)))
}

struct HeldChop {
    project: u64,
    id: u64,
    event: PlayedChop,
    source: u64,
    started: f64,
    sustained: bool,
}

struct Effects {
    echo: Vec<Frame>,
    index: usize,
    filtered: Frame,
}

#[derive(Clone, Copy)]
struct FxAmount {
    filter: f32,
    delay: f32,
    reverb: f32,
    delay_beats: f64,
}

impl Effects {
    fn new(rate: u32) -> Self {
        Self {
            echo: vec![[0.0; 2]; rate as usize * 2],
            index: 0,
            filtered: [0.0; 2],
        }
    }

    fn clear(&mut self) {
        self.echo.fill([0.0; 2]);
        self.filtered = [0.0; 2];
        self.index = 0;
    }

    fn process(&mut self, mut input: Frame, rate: f64, bpm: u16, amount: FxAmount) -> Frame {
        let len = self.echo.len();
        let delay = ((rate * 60.0 * amount.delay_beats / f64::from(bpm)) as usize).clamp(1, len - 1);
        let offset = |frames: usize| (self.index + len - frames) % len;
        let echo = self.echo[offset(delay)];
        let early = self.echo[offset((rate * 0.037) as usize)];
        let late = self.echo[offset((rate * 0.081) as usize)];
        let filter = 1.0 - 0.978 * amount.filter;
        for channel in 0..2 {
            self.filtered[channel] += filter * (input[channel] - self.filtered[channel]);
            self.echo[self.index][channel] =
                self.filtered[channel] + echo[channel] * amount.delay * 0.48;
            input[channel] = self.filtered[channel]
                + echo[channel] * amount.delay * 0.56
                + (early[channel] * 0.30 + late[1 - channel] * 0.23) * amount.reverb;
        }
        self.index = (self.index + 1) % len;
        input
    }
}

struct Scene {
    pattern: usize,
    sequence: Arc<Sequence>,
    first_step: u64,
}

struct SongWindow {
    first_step: u64,
    end_tick: u64,
    repeat: bool,
}

struct ListenVoice {
    clip: Arc<Clip>,
    start: f64,
    position: f64,
    end: f64,
    gain: f32,
    beat_grid: Option<(f64, f64)>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum StemRoute {
    #[default]
    All,
    Drum(usize),
    Pads,
    Keys,
}

pub struct Mixer {
    pub stem_route: StemRoute,
    input_monitor: Option<crate::input_audio::Monitor>,
    fill: bool,
    record_range: Option<std::ops::Range<u32>>,
    count_in: usize,
    count_in_beat: usize,
    click_gain: f32,
    pending_steps: Vec<ResolvedEvent>,
    midi_output: Option<Box<crate::midi_out::Sink>>,
    rate: f64,
    pub settings: PlaybackSettings,
    clip: Option<Arc<Clip>>,
    preview_source: usize,
    listening: Option<ListenVoice>,
    audition_pending: Option<crate::audition::Voice>,
    audition_active: Option<crate::audition::Voice>,
    audition_serial: u64,
    listening_frames: u64,
    beat_audition_frames: u64,
    audition_gain: f32,
    drums: [[Vec<Frame>; DRUMS.len()]; KITS.len()],
    voices: Vec<DrumVoice>,
    choke_frames: u32,
    pad_tails: Vec<Chop>,
    piano: Vec<PianoVoice>,
    melody: Vec<melody::Voice>,
    mono: mono::Mono,
    chop: Option<Chop>,
    cursor: f64,
    preview: bool,
    pattern_preview: bool,
    pattern: bool,
    song: Option<(Arc<Sequence>, u64)>,
    song_window: Option<SongWindow>,
    scene: Option<Scene>,
    next_scene: Option<Scene>,
    song_mix_fx: bool,
    finished_song: u64,
    current_step: i32,
    next_step: u64,
    early_step_queued: bool,
    pattern_tick: f64,
    next_performance_tick: u64,
    external_clock: bool,
    next_pulse: u64,
    pulse_seconds: Option<f64>,
    clock_waiting_start: bool,
    record: Option<(Quantize, u64)>,
    capture: Option<Sender<CapturedChop>>,
    retrospective: Option<performance_capture::Input>,
    next_capture_id: u64,
    held_chops: Vec<HeldChop>,
    skip_recorded: Vec<(u64, ChopInput)>,
    click_track: bool,
    click_frames: u32,
    click_accent: bool,
    drum_fx: Effects,
    mix_fx: [Effects; 2],
    unlimited_peak: f32,
}

impl Mixer {
    pub fn new(rate: u32) -> Self {
        Self {
            stem_route: StemRoute::All,
            rate: f64::from(rate),
            settings: PlaybackSettings::default(),
            clip: None,
            input_monitor: None,
            fill: false,
            record_range: None,
            count_in: 0,
            count_in_beat: 1,
            click_gain: 1.0,
            pending_steps: Vec::with_capacity(12 * (DRUMS.len() + 2)),
            midi_output: None,
            preview_source: 0,
            listening: None,
            audition_pending: None,
            audition_active: None,
            audition_serial: 0,
            listening_frames: 0,
            beat_audition_frames: 0,
            audition_gain: 0.45,
            drums: std::array::from_fn(|kit| std::array::from_fn(|track| drum(track, kit))),
            voices: Vec::with_capacity(48),
            choke_frames: rate.div_ceil(200).max(1),
            pad_tails: Vec::with_capacity(8),
            piano: Vec::with_capacity(64),
            melody: Vec::with_capacity(64),
            mono: mono::Mono::new(),
            chop: None,
            cursor: 0.0,
            preview: false,
            pattern_preview: false,
            pattern: false,
            song: None,
            song_window: None,
            scene: None,
            next_scene: None,
            song_mix_fx: false,
            finished_song: 0,
            current_step: -1,
            next_step: 0,
            early_step_queued: false,
            pattern_tick: 0.0,
            next_performance_tick: 0,
            external_clock: false,
            next_pulse: 0,
            pulse_seconds: None,
            clock_waiting_start: false,
            record: None,
            capture: None,
            retrospective: None,
            next_capture_id: 0,
            held_chops: Vec::with_capacity(32),
            skip_recorded: Vec::with_capacity(32),
            click_track: false,
            click_frames: 0,
            click_accent: false,
            drum_fx: Effects::new(rate),
            mix_fx: std::array::from_fn(|_| Effects::new(rate)),
            unlimited_peak: 0.0,
        }
    }

    fn replace_settings(&mut self, settings: &mut PlaybackSettings) {
        if self.settings.bpm != settings.bpm && !self.external_clock {
            self.stop_beat_audition();
        }
        let instruments_changed = !self.settings.same_capture_instruments(settings);
        if instruments_changed {
            if let Some(capture) = &mut self.retrospective {
                capture.reset();
            }
            if self.settings.instruments.glide.mono || settings.instruments.glide.mono {
                self.mono.clear();
                if settings.chromatic_enabled && settings.instruments.glide.mono {
                    self.piano.clear();
                }
            }
        }
        std::mem::swap(&mut self.settings, settings);
    }

    pub fn command(&mut self, command: Command) {
        match command {
            Command::Clip(clip) => {
                if let Some(capture) = &mut self.retrospective {
                    capture.reset();
                }
                self.listening = None;
                self.clip = clip;
                self.preview_source = 0;
                self.cursor = 0.0;
                self.preview = false;
                self.pattern_preview = false;
                self.chop = None;
                self.pad_tails.clear();
                self.piano.clear();
                self.melody.clear();
                self.mono.clear();
            }
            Command::InputMonitor(monitor) => self.input_monitor = monitor,
            Command::MidiOutput(output) => {
                if let Some(old) = &mut self.midi_output { old.stop(); }
                self.midi_output = output;
            }
            Command::PreviewSource(index) => {
                self.preview_source = index;
                if !self.pattern_preview {
                    self.preview = false;
                    self.cursor = 0.0;
                    self.chop = None;
                    self.pad_tails.clear();
                }
            }
            Command::Settings(mut settings) => self.replace_settings(&mut settings),
            Command::Play(play) => {
                self.pattern_preview = false;
                self.chop = None;
                self.pad_tails.clear();
                let clip = self.clip.as_deref().and_then(|clip| clip.source(self.preview_source));
                self.preview = play && clip.is_some();
                if self.preview && let Some(clip) = clip {
                    let range = self.settings.active_loop()
                        .filter(|_| self.settings.loop_source == self.preview_source)
                        .unwrap_or((0.0, clip.duration()));
                    if self.cursor < range.0 || self.cursor >= range.1 {
                        self.cursor = range.0;
                    }
                }
            }
            Command::Seek(time) => {
                self.chop = None;
                self.pad_tails.clear();
                let index = if self.pattern_preview { self.settings.loop_source } else { self.preview_source };
                if index == self.preview_source {
                    self.cursor = self.clip.as_deref().and_then(|clip| clip.source(index))
                        .map_or(0.0, |clip| time.clamp(0.0, clip.duration()));
                }
            }
            Command::Audition(start, end) => {
                if let Some(clip) = self.clip.as_deref().and_then(|clip| clip.source(self.preview_source)) {
                    let end = end.min(clip.duration());
                    if start >= 0.0 && start < end {
                        let assignment = SampleAssignment {
                            source: self.preview_source, range: (start, end),
                            controls: SampleControls::default(), layers: Layers::default(),
                            transpose: 0.0,
                        };
                        let voices = SampleStack::new(assignment, self.settings.speed / self.rate, 127, 0.0, HitSound::default(), self.rate);
                        self.start_chop(None, PadMode::Slice, voices);
                    }
                }
            }
            Command::Listen(clip, (start, end), gain, beat_grid) => {
                let start = start.clamp(0.0, clip.duration());
                let end = end.clamp(start, clip.duration());
                self.listening = (start < end).then_some(ListenVoice { clip, start, position: start, end, gain, beat_grid });
            }
            Command::StopListening => self.listening = None,
            Command::BeatAudition(voice) => {
                self.audition_serial = voice.serial;
                if !self.external_clock || !voice.is_fitted() {
                    self.audition_pending = Some(voice);
                }
            }
            Command::BeatAuditionGain(gain) => self.audition_gain = gain.clamp(0.0, 1.0),
            Command::StopBeatAudition => self.stop_beat_audition(),
            Command::StopAll => {
                self.song = None;
                self.song_window = None;
                self.set_pattern(false, false);
                self.preview = false;
                self.listening = None;
                self.piano.clear();
                self.melody.clear();
                self.mono.clear();
                self.capture_input(InputEvent::SilenceKeys(None));
            }
            Command::Pattern(play) => {
                self.song = None;
                self.song_window = None;
                self.set_pattern(play, play);
            }
            Command::Song(sequence, id) => self.start_song(sequence, id, None),
            Command::SongAutomation(id, sequence) => {
                if let Some((current, playing)) = &mut self.song && *playing == id {
                    self.song_mix_fx = sequence.automation.has_mix_fx()
                        || sequence.section_automation.iter().any(|(_, automation)| automation.has_mix_fx());
                    *current = sequence;
                }
            }
            Command::SongWindow(sequence, id, bars, repeat) => {
                if bars.start < bars.end && bars.end <= sequence.bars.len() {
                    let window = SongWindow {
                        first_step: (bars.start * STEPS) as u64,
                        end_tick: bars.end as u64 * u64::from(TICKS_PER_BAR),
                        repeat,
                    };
                    self.start_song(sequence, id, Some(window));
                }
            }
            Command::LaunchScene(pattern, sequence, bars) => {
                if sequence.bars.is_empty() || self.song.is_some() || self.external_clock { return; }
                let first_step = if self.pattern {
                    let quantize = (bars.clamp(1, 4) * STEPS) as u64;
                    self.next_step.div_ceil(quantize) * quantize
                } else {
                    self.set_pattern(true, true);
                    0
                };
                self.next_scene = Some(Scene { pattern, sequence, first_step });
            }
            Command::RecordRange(range) => self.record_range = range,
            Command::PreviewPerformance(bars, request) => {
                let phase = self.capture_follows_transport()
                    .then(|| self.pattern_tick.rem_euclid(f64::from(TICKS_PER_BAR)));
                self.capture_input(InputEvent::Preview { request, bars: bars.clamp(1, 16), phase });
            }
            Command::CountIn(bars) => {
                if self.pattern && !self.external_clock {
                    self.count_in_beat = (60.0 * self.rate / f64::from(self.settings.bpm)).round() as usize;
                    self.count_in = self.count_in_beat * usize::from(bars.min(2)) * 4;
                    if self.count_in > 0 && self.pattern_preview {
                        self.preview = false;
                    }
                }
            }
            Command::ClickGain(gain) => self.click_gain = gain.clamp(0.0, 1.0),
            Command::Record(mode) => {
                if mode.is_none() {
                    if std::mem::take(&mut self.count_in) > 0 {
                        self.prepare_pattern_sample();
                    }
                    self.finish_captures(|_| true);
                }
                self.record = mode;
            }
            Command::Fill(fill) => self.fill = fill,
            Command::ClickTrack(enabled) => {
                self.click_track = enabled;
                self.click_frames = 0;
            }
            Command::ClockMode(external) => {
                self.external_clock = external;
                self.song = None;
                self.set_pattern(false, true);
            }
            Command::ClockStart(resume) => {
                if self.pattern && !resume { self.stop_beat_audition(); }
                self.set_pattern(true, !resume);
            }
            Command::ClockPosition(step) => {
                self.stop_beat_audition();
                self.seek_pattern(u64::from(step));
            }
            Command::ClockPulse(seconds) => {
                if self.external_clock && self.pattern {
                    if let Some(capture) = &mut self.retrospective {
                        capture.pulse(seconds, self.rate, self.next_pulse as f64 - self.pattern_tick);
                    }
                    self.pattern_tick = self.next_pulse as f64;
                    self.next_pulse += 20;
                    self.pulse_seconds = Some(seconds);
                    if std::mem::take(&mut self.clock_waiting_start) {
                        self.prepare_pattern_sample();
                    }
                }
            }
            Command::Pad(pad, mode, velocity) => {
                if self.start_pad(pad, mode, velocity, HitSound::default()) {
                    self.capture_input(InputEvent::On(performance_capture::Note {
                        source: 0, input: ChopInput::Pad(pad as u8), velocity, mode, bent: false,
                    }));
                    self.finish_captures(|held| matches!(held.event.input, ChopInput::Pad(_)));
                    self.capture_chop(0, ChopInput::Pad(pad as u8), velocity, mode);
                }
            }
            Command::Release(pad) => {
                if let Some(chop) = &mut self.chop
                    && chop.pad == Some(pad)
                    && chop.mode == PadMode::Gate
                {
                    chop.voices.releasing(true);
                }
                self.finish_captures(|held| held.event.input == ChopInput::Pad(pad as u8));
                self.capture_input(InputEvent::Off(0, ChopInput::Pad(pad as u8), false));
            }
            Command::Drum(track) => self.trigger_drum(track, 1.0, HitSound::default()),
            Command::MidiDrum(track, velocity) => {
                self.trigger_drum(track, f32::from(velocity) / 127.0, HitSound::default())
            }
            Command::PianoOn {
                source,
                note,
                velocity,
                bend,
                range,
                controls,
                gate,
            } => {
                let assignment = key_assignment(self.settings.instrument_view(), note, range, controls);
                let voices = SampleStack::new(assignment, self.settings.speed / self.rate, velocity, bend, HitSound::default(), self.rate);
                if voices.head().is_none() {
                    return;
                }
                if self.mono_enabled() {
                    self.mono.on(mono::Note {
                        source, key: note, velocity, bend, gate, release_at: None,
                        assignment, sound: HitSound::default(), speed: self.settings.speed,
                    }, voices, self.settings.instruments.glide, self.rate);
                } else {
                    self.start_piano(PianoVoice { source, note, held: true, gate, voices });
                }
                if source != SEQUENCED_KEY {
                    self.finish_captures(|held| {
                        held.source == source && held.event.input == ChopInput::Key(note)
                    });
                    self.capture_chop(
                        source,
                        ChopInput::Key(note),
                        velocity,
                        if gate { PadMode::Gate } else { PadMode::Slice },
                    );
                    if self.clip.as_deref().and_then(|clip| clip.source(self.settings.instrument_view().key_source(note)))
                        .is_some_and(|clip| range.0 < range.1 && range.0 < clip.duration())
                    {
                        self.capture_input(InputEvent::On(performance_capture::Note {
                            source,
                            input: ChopInput::Key(note),
                            velocity,
                            mode: if gate { PadMode::Gate } else { PadMode::Slice },
                            bent: bend.abs() > f64::EPSILON,
                        }));
                    }
                }
            }
            Command::PianoOff(source, note, sustained) => {
                if self.mono_enabled() {
                    self.mono.off(source, note, sustained, self.settings.instruments.glide, self.rate);
                }
                self.capture_input(InputEvent::Off(source, ChopInput::Key(note), sustained));
                for voice in self
                    .piano
                    .iter_mut()
                    .filter(|voice| voice.source == source && voice.note == note && voice.held)
                {
                    voice.held = false;
                    voice.voices.releasing(voice.gate && !sustained);
                }
                let is_note = |held: &HeldChop| {
                    held.source == source && held.event.input == ChopInput::Key(note)
                };
                if sustained {
                    for held in self.held_chops.iter_mut().filter(|held| is_note(held)) {
                        held.sustained = true;
                    }
                } else {
                    self.finish_captures(is_note);
                }
            }
            Command::PianoPedalUp(source) => {
                if self.mono_enabled() {
                    self.mono.pedal_up(source, self.settings.instruments.glide, self.rate);
                }
                self.capture_input(InputEvent::PedalUp(source));
                for voice in self
                    .piano
                    .iter_mut()
                    .filter(|voice| voice.source == source && voice.gate && !voice.held)
                {
                    voice.voices.releasing(true);
                }
                self.finish_captures(|held| held.source == source && held.sustained);
            }
            Command::PianoBend(source, bend) => {
                if self.mono_enabled() {
                    self.mono.bend(source, bend);
                }
                if bend.abs() > f64::EPSILON {
                    self.capture_input(InputEvent::Bend(source));
                }
                for voice in self.piano.iter_mut().filter(|voice| voice.source == source) {
                    voice.voices.bend(bend);
                }
            }
            Command::PianoSilence(source) => {
                self.mono.silence(source, self.settings.instruments.glide, self.rate);
                self.capture_input(InputEvent::SilenceKeys(source));
                for voice in self
                    .piano
                    .iter_mut()
                    .filter(|voice| source.is_none_or(|id| voice.source == id))
                {
                    voice.held = false;
                    voice.voices.releasing(true);
                }
                self.finish_captures(|held| {
                    matches!(held.event.input, ChopInput::Key(_))
                        && source.is_none_or(|id| id == held.source)
                });
            }
        }
    }

    pub fn render_tail(&mut self) {
        self.pattern = false;
        self.preview = false;
        self.pattern_preview = false;
        self.mono.silence(None, self.settings.instruments.glide, self.rate);
        if let Some((sequence, _)) = &self.song {
            self.pattern_tick = self.pattern_tick.min(f64::from(sequence.ticks()) - 1e-3);
        }
        if let Some(chop) = &mut self.chop && chop.mode == PadMode::Gate {
            chop.voices.releasing(true);
        }
        for piano in &mut self.piano {
            if piano.gate || piano.voices.release_at.is_some() {
                piano.voices.releasing(true);
            }
        }
    }

    fn start_song(&mut self, sequence: Arc<Sequence>, id: u64, window: Option<SongWindow>) {
        self.set_pattern(false, false);
        self.preview = false;
        self.song_mix_fx = sequence.automation.has_mix_fx()
            || sequence.section_automation.iter().any(|(_, automation)| automation.has_mix_fx());
        self.song = Some((sequence, id));
        self.set_pattern(true, true);
        if let Some(window) = &window {
            self.seek_pattern(window.first_step);
        }
        self.song_window = window;
    }

    fn stop_beat_audition(&mut self) {
        self.audition_pending = None;
        self.audition_active = None;
    }

    fn set_pattern(&mut self, play: bool, restart: bool) {
        if !play {
            self.stop_beat_audition();
            self.mono.silence(Some(SEQUENCED_KEY), self.settings.instruments.glide, self.rate);
            if let Some(capture) = &mut self.retrospective {
                capture.stop_clock();
            }
            self.capture_input(InputEvent::SilencePads);
            self.finish_captures(|_| true);
            self.record = None;
        }
        self.pattern = play;
        self.current_step = -1;
        self.pulse_seconds = None;
        self.clock_waiting_start = play && self.external_clock;
        if restart {
            self.seek_pattern(0);
        }
        if play {
            if !self.clock_waiting_start {
                self.prepare_pattern_sample();
            }
        } else {
            if let Some(output) = &mut self.midi_output { output.stop(); }
            self.pending_steps.clear();
            self.count_in = 0;
            self.scene = None;
            self.next_scene = None;
            self.click_frames = 0;
            if self.pattern_preview {
                self.preview = false;
            }
            self.pattern_preview = false;
            self.voices.clear();
            self.piano.retain(|voice| voice.source != SEQUENCED_KEY);
            self.melody.clear();
            self.chop = None;
            self.pad_tails.clear();
            self.drum_fx.clear();
            for effects in &mut self.mix_fx {
                effects.clear();
            }
        }
    }

    fn seek_pattern(&mut self, step: u64) {
        self.melody.clear();
        self.mono.silence(Some(SEQUENCED_KEY), self.settings.instruments.glide, self.rate);
        if let Some(capture) = &mut self.retrospective {
            capture.reset_clock();
        }
        if let Some(output) = &mut self.midi_output { output.stop(); }
        self.finish_captures(|_| true);
        let tick = step * u64::from(TICKS_PER_STEP);
        self.next_step = step;
        self.early_step_queued = false;
        self.pattern_tick = tick as f64;
        self.next_performance_tick = tick;
        self.pending_steps.clear();
        self.next_pulse = tick;
        self.pulse_seconds = None;
        self.skip_recorded.clear();
    }

    fn capture_input(&mut self, event: InputEvent) {
        if let Some(capture) = &mut self.retrospective {
            capture.push(event);
        }
    }

    fn capture_follows_transport(&self) -> bool {
        self.pattern && self.count_in == 0 && !self.clock_waiting_start && self.listening.is_none()
            && (!self.external_clock || self.retrospective.as_ref()
                .is_some_and(performance_capture::Input::external_clock_running))
    }

    fn capture_chop(&mut self, source: u64, input: ChopInput, velocity: u8, mode: PadMode) {
        let Some((quantize, project)) = self.record else {
            return;
        };
        if !self.pattern || self.count_in > 0 || self.clock_waiting_start || self.song.is_some() || self.scene.is_some() {
            return;
        }
        let Some(capture) = &self.capture else { return };
        let snapped = quantize.snap(self.pattern_tick, self.settings.swing);
        let tick = (snapped % u64::from(self.settings.pattern_ticks())) as u32;
        if self.record_range.as_ref().is_some_and(|range| !range.contains(&tick)) { return; }
        let event = PlayedChop {
            id: Default::default(),
            tick,
            input,
            velocity,
            mode,
            length: None,
            sound: Default::default(),
            expression: None,
        };
        self.next_capture_id += 1;
        let id = self.next_capture_id;
        let _ = capture.send(CapturedChop::Start(project, id, event.clone()));
        if snapped >= self.next_performance_tick {
            self.skip_recorded.push((snapped, input));
        }
        if mode == PadMode::Gate {
            self.held_chops.push(HeldChop {
                project,
                id,
                event,
                source,
                started: self.pattern_tick,
                sustained: false,
            });
        }
    }

    fn finish_captures(&mut self, mut finish: impl FnMut(&HeldChop) -> bool) {
        let Some(capture) = &self.capture else { return };
        self.held_chops.retain(|held| {
            if finish(held) {
                let mut event = held.event.clone();
                event.length = Some(
                    ((self.pattern_tick - held.started).round() as u32)
                        .clamp(1, self.settings.pattern_ticks()),
                );
                let _ = capture.send(CapturedChop::End(held.project, held.id, event));
                false
            } else {
                true
            }
        });
    }

    fn playback_sequence(&self) -> Option<&Sequence> {
        self.song.as_ref().map(|(sequence, _)| sequence.as_ref())
            .or_else(|| self.scene.as_ref().map(|scene| scene.sequence.as_ref()))
    }

    fn prepare_pattern_sample(&mut self) {
        let chops = self.playback_sequence().map_or_else(
            || self.settings.has_chops(),
            |sequence| sequence.has_chops_with(self.settings.instrument_view()),
        );
        if chops {
            self.preview = false;
            self.chop = None;
            self.pad_tails.clear();
        } else if let Some((start, _)) = self.settings.active_loop()
            && self.clip.is_some()
        {
            self.cursor = start;
            self.preview = true;
            self.pattern_preview = true;
            self.chop = None;
            self.pad_tails.clear();
        }
    }

    fn start_piano(&mut self, piano: PianoVoice) {
        for voice in self
            .piano
            .iter_mut()
            .filter(|voice| voice.source == piano.source && voice.note == piano.note)
        {
            voice.held = false;
            voice.voices.releasing(true);
        }
        if self.piano.len() == self.piano.capacity() {
            self.piano.remove(0);
        }
        self.piano.push(piano);
    }

    fn mono_enabled(&self) -> bool {
        self.settings.chromatic_enabled && self.settings.instruments.glide.mono
    }

    fn position(&self) -> f64 {
        self.chop
            .as_ref()
            .and_then(|chop| chop.voices.head())
            .map_or(self.cursor, |head| head.voice.position)
    }

    fn start_pad(&mut self, pad: usize, mode: PadMode, velocity: u8, sound: HitSound) -> bool {
        let Some(assignment) = self.clip.as_deref()
            .and_then(|clip| pad_assignment(clip, self.settings.instrument_view(), pad, mode))
        else {
            return false;
        };
        let voices = SampleStack::new(assignment, self.settings.speed / self.rate, velocity, 0.0, sound, self.rate);
        if voices.head().is_none() {
            return false;
        }
        self.start_chop(Some(pad), mode, voices);
        true
    }

    fn start_chop(&mut self, pad: Option<usize>, mode: PadMode, voices: SampleStack) {
        self.preview = false;
        self.pattern_preview = false;
        if let Some(head) = voices.head().filter(|head| head.source == self.preview_source) {
            self.cursor = head.voice.position;
        }
        let group = pad.map_or(0, |pad| self.settings.choke_groups.pads[pad]);
        self.choke_group(group);
        if let Some(previous) = self.chop.take()
            && previous.choke.remaining.is_some_and(|frames| frames > 0)
        {
            if self.pad_tails.len() == self.pad_tails.capacity() {
                self.pad_tails.remove(0);
            }
            self.pad_tails.push(previous);
        }
        self.chop = Some(Chop { pad, mode, voices, choke: Choke::new(group) });
    }

    fn trigger_drum(&mut self, track: usize, gain: f32, sound: HitSound) {
        let group = self.settings.choke_groups.drums[track];
        self.choke_group(group);
        if self.voices.len() == self.voices.capacity() {
            self.voices.remove(0);
        }
        let source = self.settings.drum_sources[track];
        let kit = self.settings.kit;
        let position = if sound.start_percent == 0 {
            0.0
        } else {
            let frames = source.map_or(self.drums[kit][track].len(), |index| {
                self.clip.as_deref().and_then(|clip| clip.source(index)).map_or(0, |clip| clip.frames.len())
            });
            frames as f64 * f64::from(sound.start_percent) / 100.0
        };
        let tune = f64::from(self.settings.drum_mix[track].tune) + f64::from(sound.pitch_cents) / 100.0;
        self.voices.push(DrumVoice {
            clip_source: source,
            rate_multiplier: 2.0_f64.powf(tune / 12.0),
            track,
            kit,
            position,
            gain,
            choke: Choke::new(group),
            filter: HitFilter::new(sound.cutoff_hz, self.rate),
            started: false,
        });
    }

    fn choke_group(&mut self, group: u8) {
        if group == 0 {
            return;
        }
        for voice in &mut self.voices {
            voice.choke.release(group, self.choke_frames, voice.started);
        }
        if let Some(chop) = &mut self.chop
            && chop.choke.release(group, self.choke_frames, chop.voices.started)
        {
            self.capture_input(InputEvent::SilencePads);
        }
    }

    fn tick(&mut self) {
        let absolute = self.next_step;
        if absolute.is_multiple_of(STEPS as u64) {
            if let Some(voice) = self.audition_pending.take() {
                self.audition_active = Some(voice);
            } else if let Some(voice) = &mut self.audition_active {
                voice.on_bar();
            }
        }
        if absolute.is_multiple_of(STEPS as u64)
            && let Some(scene) = &self.scene
            && absolute > scene.first_step
            && let Some(capture) = &self.capture
        {
            let offset = ((absolute - scene.first_step) / STEPS as u64 - 1) as usize % scene.sequence.bars.len();
            let _ = capture.send(CapturedChop::SceneBar { pattern: scene.pattern, offset });
        }
        if self.next_scene.as_ref().is_some_and(|scene| scene.first_step <= absolute) {
            self.scene = self.next_scene.take();
            self.next_performance_tick = absolute * u64::from(TICKS_PER_STEP);
            self.pending_steps.clear();
            self.prepare_pattern_sample();
        }
        let index = self.schedule_step(absolute, false);
        self.play_pending_steps(swung_step(absolute, self.settings.swing));
        self.next_step += 1;
        self.early_step_queued = false;
        self.current_step = index as i32;
        let step = index % STEPS;
        if self.click_track && step.is_multiple_of(4) {
            self.click_frames = (self.rate * 0.025) as u32;
            self.click_accent = step == 0;
        }
    }

    fn schedule_step(&mut self, absolute: u64, early: bool) -> usize {
        let sequence = self.playback_sequence();
        let relative = absolute - self.scene.as_ref().map_or(0, |scene| scene.first_step);
        let bars = sequence.map_or(self.settings.bars, |sequence| sequence.bars.len());
        let index = (relative % (bars * STEPS) as u64) as usize;
        let (bar, step) = (index / STEPS, index % STEPS);
        let cycle = sequence.and_then(|sequence| sequence.bar_cycles.get(bar).copied())
            .unwrap_or(relative / (bars * STEPS) as u64);
        let (drums, pad, key, metadata) = match sequence {
            Some(sequence) => {
                let notes = &sequence.bars[bar];
                (&notes.drums, notes.pads[step], notes.keys[step], notes.metadata())
            }
            None => (
                &self.settings.hits[bar], self.settings.chops[bar][step],
                self.settings.key_steps[bar][step],
                StepMetadata {
                    rules: &self.settings.step_rules[bar],
                    sounds: &self.settings.step_sounds[bar],
                    ids: &self.settings.step_ids[bar],
                    feel: &self.settings.step_feel[bar],
                },
            ),
        };
        for (row, cell) in Cell::column(drums, pad, key, metadata, step).into_iter().enumerate() {
            let Some(cell) = cell else { continue };
            let schedule_early = step != 0 && cell.plays_early(self.settings.lane_offsets[row]);
            if early != schedule_early && (early || self.early_step_queued) {
                continue;
            }
            let timing = self.settings.step_timing();
            self.pending_steps.extend(cell.resolve(timing, row, absolute, cycle, self.fill));
        }
        index
    }

    fn play_pending_steps(&mut self, through: u64) {
        while let Some(index) = self.pending_steps.iter().enumerate()
            .filter(|(_, event)| event.tick <= through)
            .min_by_key(|(_, event)| {
                let (order, id) = if self.settings.choke_groups.is_empty() {
                    (0, NoteId::default())
                } else {
                    (event.trigger.order(), event.id)
                };
                (event.tick, order, id)
            })
            .map(|(index, _)| index)
        {
            let event = self.pending_steps.swap_remove(index);
            self.play_event(event);
        }
    }

    fn play_event(&mut self, event: ResolvedEvent) {
        let release_at = event.gate_ticks.map(|length| event.tick + u64::from(length));
        if let Some(output) = &mut self.midi_output {
            let (part, note, velocity) = event.midi();
            output.note(part, note, velocity, event.tick, event.midi_ticks);
        }
        match event.trigger {
            Trigger::Drum(track) => {
                self.trigger_drum(usize::from(track), f32::from(event.velocity) / 127.0, event.sound);
            }
            Trigger::Pad(pad) => {
                if self.start_pad(usize::from(pad), event.mode, event.velocity, event.sound)
                    && let Some(chop) = &mut self.chop
                {
                    chop.voices.release_at = release_at;
                    self.capture_input(InputEvent::SilencePads);
                }
            }
            Trigger::Key(note) => {
                if let Some(expression) = &event.expression {
                    if let Some(voice) = melody::Voice::new(&event, note, expression.clone(), self.clip.as_deref(), self.rate) {
                        if self.melody.len() == self.melody.capacity() { self.melody.remove(0); }
                        self.melody.push(voice);
                    }
                    return;
                }
                let instruments = self.settings.instrument_view();
                let clip_source = instruments.key_source(note);
                let Some(clip) = self.clip.as_deref().and_then(|clip| clip.source(clip_source)) else {
                    return;
                };
                let Some(range) = instruments.key_range(note, clip.duration()) else {
                    return;
                };
                let assignment = key_assignment(instruments, note, range, instruments.controls_for_key(note));
                let mut voices = SampleStack::new(assignment, self.settings.speed / self.rate, event.velocity, 0.0, event.sound, self.rate);
                if voices.head().is_some() {
                    if self.mono_enabled() {
                        self.mono.advance(event.tick, self.settings.instruments.glide, self.rate);
                        self.mono.on(mono::Note {
                            source: SEQUENCED_KEY, key: note, velocity: event.velocity, bend: 0.0,
                            gate: true, release_at: Some(event.tick + event.midi_ticks),
                            assignment, sound: event.sound, speed: self.settings.speed,
                        }, voices, self.settings.instruments.glide, self.rate);
                    } else {
                        voices.release_at = release_at;
                        self.start_piano(PianoVoice {
                            source: SEQUENCED_KEY, note, held: event.grid, gate: false, voices,
                        });
                    }
                }
            }
        }
    }

    fn advance_pattern(&mut self) {
        if !self.pattern || self.clock_waiting_start {
            return;
        }
        if self.count_in > 0 {
            if self.count_in.is_multiple_of(self.count_in_beat) {
                self.click_frames = (self.rate * 0.025) as u32;
                self.click_accent = (self.count_in / self.count_in_beat).is_multiple_of(4);
            }
            self.count_in -= 1;
            if self.count_in == 0 { self.prepare_pattern_sample(); }
            return;
        }
        let mut now = (self.pattern_tick + 1e-7).floor() as u64;
        let length = u64::from(self.playback_sequence().map_or_else(
            || self.settings.pattern_ticks(), Sequence::ticks,
        ));
        if let Some((_, id)) = self.song.as_ref()
            && now >= self.song_window.as_ref().map_or(length, |window| window.end_tick)
        {
            if let Some(window) = &self.song_window && window.repeat {
                self.seek_pattern(window.first_step);
                self.chop = None;
                self.pad_tails.clear();
                self.piano.retain(|voice| voice.source != SEQUENCED_KEY);
                self.voices.clear();
                self.prepare_pattern_sample();
                now = self.pattern_tick as u64;
            } else {
                self.finished_song = *id;
                self.song = None;
                self.song_window = None;
                self.set_pattern(false, false);
                return;
            }
        }
        if let Some(output) = &mut self.midi_output { output.advance(now, self.external_clock); }
        loop {
            let tick = swung_step(self.next_step, self.settings.swing);
            if !self.early_step_queued && !self.next_step.is_multiple_of(STEPS as u64)
                && now >= tick - crate::model::groove::EARLY_TICKS
            {
                self.schedule_step(self.next_step, true);
                self.early_step_queued = true;
            }
            if tick > now { break; }
            self.play_pending_steps(tick);
            self.tick();
        }
        self.play_pending_steps(now);
        let origin = self.scene.as_ref().map_or(0, |scene| scene.first_step * u64::from(TICKS_PER_STEP));
        let length = self.playback_sequence().map_or(self.settings.pattern_ticks(), Sequence::ticks);
        while !self.performance().is_empty() && self.next_performance_tick <= now {
            let absolute = self.next_performance_tick;
            let tick = (absolute.saturating_sub(origin) % u64::from(length)) as u32;
            let mut index = self
                .performance()
                .partition_point(|event| event.tick < tick);
            while let Some(event) = self
                .performance()
                .get(index)
                .filter(|event| event.tick == tick)
            {
                if !self.skip_recorded.contains(&(absolute, event.input)) {
                    self.play_event(ResolvedEvent::recorded(event, absolute));
                }
                index += 1;
            }
            self.next_performance_tick += 1;
        }
        if self.next_performance_tick <= now {
            self.next_performance_tick = now + 1;
        }
        self.skip_recorded.retain(|&(tick, _)| tick > now);
        if self.external_clock {
            if let Some(seconds) = self.pulse_seconds {
                self.pattern_tick = (self.pattern_tick + 20.0 / (seconds * self.rate))
                    .min(self.next_pulse as f64 - 1e-5);
            }
        } else {
            self.pattern_tick +=
                f64::from(self.settings.bpm) * f64::from(TICKS_PER_BAR) / (240.0 * self.rate);
        }
    }

    fn performance(&self) -> &[PlayedChop] {
        self.playback_sequence().map_or(&self.settings.performance, |sequence| &sequence.performance)
    }

    pub fn next_frame(&mut self) -> Frame {
        let position = self.pattern_tick;
        let synced = self.retrospective.is_some() && self.capture_follows_transport();
        let (sample, drums) = self.next_stems();
        if let Some(capture) = &mut self.retrospective {
            let delta = synced.then_some(self.pattern_tick - position);
            let free = f64::from(self.settings.bpm) * f64::from(TICKS_PER_BAR) / (240.0 * self.rate);
            capture.frame(delta, free);
        }
        let click = if self.click_frames == 0 {
            0.0
        } else {
            let remaining = f64::from(self.click_frames) / self.rate;
            self.click_frames -= 1;
            let frequency = if self.click_accent { 1_450.0 } else { 950.0 };
            (f64::from(TAU) * frequency * (0.025 - remaining)).sin() as f32
                * (remaining as f32 / 0.025).powi(3)
                * 0.20 * self.click_gain
        };
        let monitor = self.input_monitor.as_mut().map_or([0.0; 2], |monitor| monitor.next(self.rate));
        if self.audition_active.is_some() && self.audition_gain > 0.0 && self.listening.is_none() {
            self.beat_audition_frames += 1;
        }
        let audition = self.audition_active.as_mut().map_or([0.0; 2], |voice| {
            voice.next(self.rate, self.audition_gain)
        });
        self.unlimited_peak = 0.0;
        std::array::from_fn(|channel| {
            let mixed = sample[channel] + drums[channel] + click + monitor[channel] + audition[channel];
            self.unlimited_peak = self.unlimited_peak.max(mixed.abs());
            if mixed.abs() <= 0.8 {
                mixed
            } else {
                mixed.signum() * (0.8 + 0.2 * ((mixed.abs() - 0.8) * 5.0).tanh())
            }
        })
    }

    pub fn next_stems(&mut self) -> (Frame, Frame) {
        if let Some(voice) = &mut self.listening {
            self.listening_frames += 1;
            let at = voice.position;
            let mut frame = voice.clip.read(at * f64::from(voice.clip.rate));
            let fade = ((at - voice.start).min(voice.end - at) * 100.0).clamp(0.0, 1.0) as f32;
            frame = frame.map(|sample| sample * fade * voice.gain);
            if let Some((beat, anchor)) = voice.beat_grid {
                let since = (at - anchor).rem_euclid(beat) as f32;
                if since < 0.025 {
                    let click = (since * 1500.0 * TAU).sin() * (1.0 - since / 0.025).powi(2) * 0.30;
                    frame = frame.map(|sample| sample + click);
                }
            }
            voice.position += 1.0 / self.rate;
            if voice.position >= voice.end {
                self.listening = None;
            }
            return (frame, [0.0; 2]);
        }
        let position = self.pattern_tick;
        self.advance_pattern();
        let tick = self.pattern_tick as u64;
        if let Some(chop) = &mut self.chop {
            chop.voices.advance_gate(tick);
        }
        let mut sample = self.sample_frame();
        let route = self.stem_route;
        if route == StemRoute::Keys {
            sample = [0.0; 2];
        }
        let clip = self.clip.as_deref();
        self.pad_tails.retain_mut(|chop| {
            let Some(frame) = clip.and_then(|clip| chop.frame(clip, self.rate, self.choke_frames))
            else {
                return false;
            };
            if route != StemRoute::Keys {
                for (out, value) in sample.iter_mut().zip(frame) {
                    *out += value;
                }
            }
            true
        });
        self.piano.retain_mut(|voice| {
            voice.voices.advance_gate(tick);
            let Some(frame) = clip.and_then(|clip| voice.voices.frame(clip, self.rate)) else {
                return false;
            };
            if route != StemRoute::Pads {
                for (out, value) in sample.iter_mut().zip(frame) {
                    *out += value;
                }
            }
            true
        });
        self.melody.retain_mut(|voice| {
            let Some(frame) = voice.frame(clip, self.rate, self.pattern_tick) else { return false };
            if route != StemRoute::Pads {
                for (out, value) in sample.iter_mut().zip(frame) { *out += value; }
            }
            true
        });
        if self.mono_enabled()
            && let Some(frame) = clip.and_then(|clip| {
                self.mono.frame(clip, tick, self.settings.instruments.glide, self.rate)
            })
            && route != StemRoute::Pads
        {
            for (out, value) in sample.iter_mut().zip(frame) {
                *out += value;
            }
        }
        let mut drums = [0.0; 2];
        let solo = self.settings.drum_mix.iter().any(|strip| strip.solo);
        self.voices.retain_mut(|voice| {
            let (frames, source_rate) = if let Some(index) = voice.clip_source {
                let Some(source) = clip.and_then(|root| root.source(index)) else { return false };
                (&*source.frames, source.rate)
            } else {
                (&*self.drums[voice.kit][voice.track], RENDER_RATE)
            };
            let choke_gain = voice.choke.gain(self.choke_frames);
            if voice.position >= frames.len() as f64 || choke_gain == 0.0 {
                return false;
            }
            let strip = self.settings.drum_mix[voice.track];
            let routed = !matches!(route, StemRoute::Drum(track) if track != voice.track);
            if routed && !strip.mute && (!solo || strip.solo) {
                let mut frame = interpolate(frames, voice.position);
                if let Some(filter) = &mut voice.filter {
                    frame = filter.apply(frame);
                }
                let left = 1.0 - strip.pan.max(0.0);
                let right = 1.0 + strip.pan.min(0.0);
                drums[0] += frame[0] * voice.gain * strip.gain * left * choke_gain;
                drums[1] += frame[1] * voice.gain * strip.gain * right * choke_gain;
            }
            voice.position += f64::from(source_rate) * voice.rate_multiplier / self.rate;
            voice.started = true;
            true
        });
        let drums = self.drum_fx.process(
            drums,
            self.rate,
            self.settings.bpm,
            FxAmount {
                filter: if self.settings.drum_fx_bypass { 0.0 } else { self.settings.filter },
                delay: if self.settings.drum_fx_bypass { 0.0 } else { self.settings.delay },
                reverb: if self.settings.drum_fx_bypass { 0.0 } else { self.settings.reverb },
                delay_beats: self.settings.delay_division.beats(),
            },
        );
        let song = self.song.as_ref().filter(|(sequence, _)| {
            !self.clock_waiting_start
                && (!sequence.automation.is_empty() || !sequence.section_automation.is_empty())
        });
        let Some((sequence, _)) = song else {
            return (
                sample.map(|sample| sample * self.settings.sample_level),
                drums.map(|sample| sample * self.settings.drum_level),
            );
        };
        let local = sequence.section_automation_at(position + 1e-7);
        let value = |target: AutomationTarget| {
            if let Some((automation, local_tick)) = local
                && !automation.points(target).is_empty()
            {
                automation.value(target, local_tick, target.baseline(self.settings.sample_level, self.settings.drum_level))
            } else {
                sequence
                    .automation
                    .value(target, position, target.baseline(self.settings.sample_level, self.settings.drum_level))
            }
        };
        let sample_level = value(AutomationTarget::SampleVolume);
        let drum_level = value(AutomationTarget::DrumVolume);
        let master = value(AutomationTarget::MasterVolume);
        let mut sample = sample.map(|frame| frame * sample_level);
        let mut drums = drums.map(|frame| frame * drum_level);
        if self.song_mix_fx && !self.settings.mix_fx_bypass {
            let effects = FxAmount {
                filter: 1.0 - value(AutomationTarget::LowPass),
                delay: value(AutomationTarget::Delay),
                reverb: value(AutomationTarget::Reverb),
                delay_beats: self.settings.delay_division.beats(),
            };
            sample = self.mix_fx[0].process(sample, self.rate, self.settings.bpm, effects);
            drums = self.mix_fx[1].process(drums, self.rate, self.settings.bpm, effects);
        }
        (
            sample.map(|frame| frame * master),
            drums.map(|frame| frame * master),
        )
    }

    fn sample_frame(&mut self) -> Frame {
        let Some(root) = &self.clip else {
            return [0.0; 2];
        };
        let speed = self.settings.speed / self.rate;
        if let Some(chop) = &mut self.chop {
            let frame = chop.frame(root, self.rate, self.choke_frames);
            if let Some(head) = chop.voices.head().filter(|head| head.source == self.preview_source)
                && let Some(clip) = root.source(head.source)
            {
                self.cursor = head.voice.position.clamp(0.0, clip.duration());
            }
            if frame.is_none() {
                self.chop = None;
            }
            return frame.unwrap_or([0.0; 2]);
        }
        if !self.preview {
            return [0.0; 2];
        }
        let index = if self.pattern_preview { self.settings.loop_source } else { self.preview_source };
        let Some(clip) = root.source(index) else {
            self.preview = false;
            return [0.0; 2];
        };
        let loop_range = self.settings.active_loop().filter(|_| self.settings.loop_source == index);
        let (start, end) = loop_range.unwrap_or((0.0, clip.duration()));
        if loop_range.is_some() && !(start..end).contains(&self.cursor) {
            self.cursor = start;
        }
        let frame = clip.read(self.cursor * f64::from(clip.rate));
        let fade = ((self.cursor - start) * 500.0)
            .min((end - self.cursor) * 500.0)
            .clamp(0.0, 1.0) as f32;
        self.cursor += speed;
        if loop_range.is_some() {
            if self.cursor >= end {
                self.cursor = start;
            }
        } else if self.cursor >= clip.duration() {
            self.cursor = clip.duration();
            self.preview = false;
        }
        [frame[0] * fade, frame[1] * fade]
    }
}

pub fn noise(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / 8_388_608.0 - 1.0
}

fn drum(track: usize, kit: usize) -> Vec<Frame> {
    let mut seed = 3100 + track as u32 * 91 + kit as u32 * 103;
    let rate = RENDER_RATE as f32;
    let mut phase = 0.0_f32;
    let mut low = 0.0_f32;
    let mut previous = 0.0_f32;
    let decay = [1.0, 1.32, 0.75, 0.84][kit];
    let seconds = [
        0.34, 0.25, 0.33, 0.27, 0.23, 0.11, 0.22, 0.08, 0.34, 0.92, 0.52,
    ][track];
    (0..(seconds * decay * rate) as usize)
        .map(|i| {
            let t = i as f32 / rate;
            let n = noise(&mut seed);
            low += (n - low) * 0.17;
            let bright = n - low;
            let e = |time: f32| (-t / (time * decay)).exp();
            let signal = match track {
                0 => {
                    phase += TAU * ([53.0, 42.0, 61.0, 50.0][kit] + 105.0 * e(0.017)) / rate;
                    (phase.sin() * e(0.083) + bright * e(0.004) * 0.17) * 0.79
                }
                1 => {
                    (low * e(0.048) * 1.15
                        + bright * e(0.038) * 0.65
                        + (t * 183.0 * TAU).sin() * e(0.026) * 0.30)
                        * 0.62
                }
                2..=4 => {
                    phase += TAU * (77.0 + (track - 2) as f32 * 35.0 + 79.0 * e(0.026)) / rate;
                    (phase.sin() * e(0.075) + bright * e(0.013) * 0.15) * 0.56
                }
                5 => (bright * e(0.007) + (t * 935.0 * TAU).sin() * e(0.015)) * 0.39,
                6 => {
                    let bursts =
                        (-(t - 0.012).abs() / 0.003).exp() + (-(t - 0.024).abs() / 0.004).exp();
                    (bright * 0.55 + low * 0.55) * (bursts + e(0.038)) * 0.48
                }
                _ => {
                    let metallic = ((t * 2879.0 * TAU).sin()
                        + (t * 5533.0 * TAU).sin()
                        + (t * 7417.0 * TAU).sin())
                        / 3.0;
                    let tail = [0.018, 0.075, 0.21, 0.11][track - 7];
                    (bright * 0.56 + metallic * 0.44) * e(tail) * 0.43
                }
            };
            previous += (signal - previous) * if kit == 3 { 0.28 } else { 0.85 };
            let signal = previous * (t * 2300.0).min(1.0);
            [signal, signal * 0.96]
        })
        .collect()
}
