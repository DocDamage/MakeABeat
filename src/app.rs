pub(crate) mod actions;
mod audition_workflow;
mod beat_workflow;
mod extraction_workflow;
mod groove_workflow;
mod library_persistence;
mod recovery_workflow;
pub(crate) mod sequence_workflow;
pub(crate) mod search_workflow;
mod taste_workflow;
pub(crate) mod transcription_workflow;
mod project_session;

use actions::Action;
use crate::{
    analysis::{AudioProfile, Emphasis, MatchBy, Sound},
    audio::{CapturedChop, Clip, Command, Output, PadMode},
    catalog::ReleaseFilters,
    files::{self, Project},
    midi::{self, Message, Route, Signal, Target},
    model::{
        BarContent, ChopInput, DRUMS, DawTarget, DigPreset, GENRES, KeyChop, Library, MAX_NAMED_SAMPLES, MAX_PATTERNS,
        NamedSample, PIANO_HIGH, PIANO_LOW, PlayedChop, Quantize, QueueEntry, Record, STEPS,
        SampleBank, SampleControls, SavedRecord, Source, Studio, Verdict,
    },
    waveform::WaveformView,
    youtube::DigFilters,
};
use eframe::egui::{self, Key};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Dig,
    Queue,
    Studio,
    Search,
    Crate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudioView {
    Sample,
    Perform,
    Sequence,
    Arrange,
    Mix,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    YouTube,
    Demo,
    Local,
    Commons,
}

#[derive(Clone, Copy)]
pub enum OpenKind {
    Project,
    Recovery,
    Template,
}

const DRUM_PREVIEW_JOB: &str = "Preparing drum preview…";
const SNAPSHOT_PREVIEW_JOB: &str = "Preparing A/B comparison…";

type RetryJob = (&'static str, Arc<dyn Fn() -> files::Result<Job> + Send + Sync>);

pub enum Job {
    Loaded {
        record: Record,
        clip: Clip,
        bpm: u16,
        restore: Option<Box<Studio>>,
        history: Option<usize>,
        start: Option<f64>,
        open_studio: bool,
        view_count: Option<u64>,
    },
    Opened(Box<Project>, Option<Clip>, PathBuf, OpenKind),
    Saved(PathBuf, Arc<Project>),
    Folder(Vec<Record>),
    Tempo(usize, crate::tempo::Estimate),
    SourceAdded(Record, Clip, Option<usize>),
    StemsAdded(Vec<(Record, Clip)>),
    DrumKit(Box<Project>, Option<Clip>),
    Relinked(Source, Source),
    MidiMapping(String, midi::PortSettings),
    Queue(Vec<QueueEntry>, bool),
    Listen(Source, Clip, Option<(f64, f64)>),
    Suggestions(usize, Vec<NamedSample>),
    DrumTemplatePreview(usize, bool, Clip),
    SnapshotPreview(u64, [Arc<Clip>; 2], [f32; 2]),
    #[cfg(windows)]
    DragPrepared(PathBuf),
    Delivered(crate::daw::Delivery),
    Wrote(String),
}

pub const PAD_KEYS: [Key; STEPS] = [
    Key::Z,
    Key::X,
    Key::C,
    Key::V,
    Key::A,
    Key::S,
    Key::D,
    Key::F,
    Key::Q,
    Key::W,
    Key::E,
    Key::R,
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
];

pub const PIANO_KEYS: [Key; 17] = [
    Key::A,
    Key::W,
    Key::S,
    Key::E,
    Key::D,
    Key::F,
    Key::T,
    Key::G,
    Key::Y,
    Key::H,
    Key::U,
    Key::J,
    Key::K,
    Key::O,
    Key::L,
    Key::P,
    Key::Semicolon,
];

enum PendingChange {
    Replace(Box<Job>),
    Quit,
}

#[derive(Clone)]
struct QueueAudio {
    source: Source,
    clip: Arc<Clip>,
    matched_range: Option<(f64, f64)>,
}

#[derive(Clone, Copy)]
pub struct SceneRun {
    pub pattern: usize,
    pub offset: usize,
    pub bars: usize,
}

pub struct App {
    session: project_session::Session,
    pending_change: Option<PendingChange>,
    discard_after_analysis: bool,
    finishing_close: bool,
    closing: bool,
    ctx: egui::Context,
    pub page: Page,
    pub show_commands: bool,
    pub focus_commands: bool,
    pub command_query: String,
    pub pool: Pool,
    pub genres: [bool; GENRES.len()],
    pub dig_decade: Option<u16>,
    pub dig_max_views: String,
    pub release_year: String,
    pub release_country: String,
    pub release_label: String,
    pub unheard_only: bool,
    pub exclude_saved: bool,
    pub exclude_skipped: bool,
    pub preset_name: String,
    pub queue_tab: Verdict,
    pub queue_similar: bool,
    taste: taste_workflow::Taste,
    pub queue_marked: HashSet<Source>,
    pub queue_level_match: bool,
    queue_audio: Option<QueueAudio>,
    queue_range: Option<(f64, f64)>,
    pub queue_focus: Option<Source>,
    pub queue_advance: bool,
    pub queue_preview_seconds: f64,
    pub queue_preview_point: f64,
    pub queue_playing: Option<Source>,
    queue_preview_started: Option<Instant>,
    pub suggestions: Vec<NamedSample>,
    pub daw_dialog: bool,
    pub daw_pattern_only: bool,
    pub daw_options: crate::daw::Options,
    pub export_dialog: Option<crate::ui::export::Dialog>,
    #[cfg(windows)]
    pub drag_export: Option<PathBuf>,
    pub daw_delivery: Option<crate::daw::Delivery>,
    pub record: Option<Record>,
    pub view_count: Option<u64>,
    analysis: beat_workflow::Analysis,
    pub(crate) audition: audition_workflow::Audition,
    pub(crate) groove_editor: groove_workflow::GrooveEditor,
    pub(crate) search: search_workflow::Search,
    pub tempo_estimate: Option<crate::tempo::Estimate>,
    pub clip: Option<Arc<Clip>>,
    detached_clip: Option<Arc<Clip>>,
    pub active_source: usize,
    pub studio: Studio,
    pub waveform: WaveformView,
    pub wave_snap: crate::waveform::Snap,
    pub wave_beats: bool,
    pub wave_height: f32,
    pub samples: Vec<NamedSample>,
    pub sample_name: String,
    pub suggestion_bars: u8,
    pub suggestion_emphasis: Emphasis,
    pub match_priority: MatchBy,
    pub sample_rename: Option<(usize, String)>,
    pub selected_sample: Option<usize>,
    samples_in_project: bool,
    sent: Studio,
    pub library: Library,
    pub collection: Option<String>,
    pub new_collection: Option<String>,
    pub youtube_url: Option<String>,
    pub history: Vec<Record>,
    pub history_cursor: Option<usize>,
    demos: Vec<Record>,
    folder: Vec<Record>,
    pub output: Option<Output>,
    pub audio_error: Option<String>,
    pub capture_quantize: Quantize,
    pub position: f64,
    seeked_at: Option<Instant>,
    pub preview: bool,
    pub playing: bool,
    pub song_playing: Option<u64>,
    pub scene_active: Option<usize>,
    pub scene_pending: Option<usize>,
    pub scene_quantize: usize,
    pub scene_capture: bool,
    pub scene_take: Vec<SceneRun>,
    song_request: u64,
    pub studio_view: StudioView,
    pub arrangement_timeline: crate::ui::arrangement::Timeline,
    pub pattern_name: String,
    pub recording: bool,
    pub input_recording: Option<crate::input_audio::Recording>,
    pub input_device: Option<String>,
    pub input_monitor: bool,
    pub demucs_path: Option<PathBuf>,
    pub job_control: Option<crate::background::Control>,
    running_job: Option<RetryJob>,
    retry_job: Option<RetryJob>,
    pub click_track: bool,
    pub click_gain: f32,
    pub count_in_bars: u8,
    pub replace_take: bool,
    pub punch_in: bool,
    pub punch_first: usize,
    pub punch_last: usize,
    pub fill: bool,
    pub quantize: Quantize,
    pub piano_roll: Option<crate::ui::piano_roll::PianoRoll>,
    pub(crate) transcription: transcription_workflow::State,
    pub(crate) extraction: extraction_workflow::State,
    pub piano_roll_clipboard: Vec<PlayedChop>,
    recording_project: u64,
    recorded_gates: HashMap<(u32, ChopInput), u64>,
    pub step: Option<usize>,
    pub selected_pad: usize,
    pub pad_mode: PadMode,
    pub piano_key: u8,
    pub piano_arm: bool,
    pub piano_gate: bool,
    pub piano_octave: u8,
    pub piano_pointer: Option<u8>,
    computer_notes: [Option<u8>; PIANO_KEYS.len()],
    piano_held: HashSet<(u64, u8)>,
    harmony_held: HashMap<(u64, u8), Vec<u8>>,
    midi: midi::Input,
    pub midi_dialog: bool,
    pub midi_devices: Vec<midi::Device>,
    pub midi_error: Option<String>,
    pub midi_output_status: Option<String>,
    midi_pickup: HashMap<(u64, Signal), (f32, bool)>,
    midi_output_failed: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub midi_selected: Option<String>,
    pub midi_learn: Option<Target>,
    pub midi_target: Target,
    pub midi_last: Option<(u64, Signal, u8)>,
    pub midi_clock_bpm: Option<f64>,
    midi_clock_connection: Option<u64>,
    midi_last_clock: Option<Instant>,
    midi_pad_held: HashMap<(u64, Signal), usize>,
    midi_buttons: HashSet<(u64, Signal)>,
    midi_sustain: HashSet<u64>,
    midi_bend: HashMap<u64, f64>,
    pub keyboard: bool,
    pub bar: usize,
    pub bar_clipboard: Option<BarContent>,
    pub drum_templates: Option<crate::ui::drum_templates::Browser>,
    pub(crate) sequence_tool: Option<crate::ui::sequencer::Tool>,
    pub(crate) sample_tool: Option<crate::ui::sample::Tool>,
    pub(crate) reveal_audio_output: bool,
    pub variations: Option<crate::ui::variation::Dialog>,
    pub snapshot_dialog: Option<crate::ui::snapshots::Dialog>,
    pub snapshots: Arc<Vec<crate::model::snapshot::NamedSnapshot>>,
    source_ids: Vec<crate::model::snapshot::SourceId>,
    next_source_id: u64,
    snapshot_preview_request: Option<u64>,
    snapshot_preview: Option<([Arc<Clip>; 2], [f32; 2])>,
    pub taps: Vec<Instant>,
    pub status: Option<(String, bool)>,
    pub library_recovery: Option<Arc<files::LibraryRecovery>>,
    library_persistence: library_persistence::Persistence,
    pub busy: Option<(&'static str, Receiver<Result<Job, String>>)>,
    rng: u64,
}

impl App {
    pub fn new(ctx: egui::Context) -> Self {
        let (library, library_recovery) = files::read_library();
        library.interface.apply(&ctx);
        let (output, audio_error) = match Output::open(&library.audio) {
            Ok(output) => (Some(output), None),
            Err(error) => (None, Some(error)),
        };
        let rng = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |t| t.as_nanos() as u64);
        let midi = midi::Input::start(ctx.clone(), library.midi.disabled());
        Self {
            session: project_session::Session::new(
                Project::default(),
                rng,
                ctx.clone(),
            ),
            pending_change: None,
            discard_after_analysis: false,
            finishing_close: false,
            closing: false,
            library_persistence: library_persistence::Persistence::new(ctx.clone()),
            ctx,
            page: Page::Dig,
            show_commands: false,
            focus_commands: false,
            command_query: String::new(),
            pool: Pool::YouTube,
            genres: [false; GENRES.len()],
            dig_decade: None,
            dig_max_views: String::new(),
            release_year: String::new(),
            release_country: String::new(),
            release_label: String::new(),
            unheard_only: false,
            exclude_saved: false,
            exclude_skipped: false,
            preset_name: String::new(),
            queue_tab: Verdict::Pending,
            queue_similar: false,
            taste: Default::default(),
            queue_marked: HashSet::new(),
            queue_level_match: false,
            queue_audio: None,
            queue_range: None,
            queue_focus: None,
            queue_advance: true,
            queue_preview_seconds: 12.0,
            queue_preview_point: 0.22,
            queue_playing: None,
            queue_preview_started: None,
            suggestions: Vec::new(),
            daw_dialog: false,
            daw_pattern_only: false,
            daw_options: Default::default(),
            export_dialog: None,
            #[cfg(windows)]
            drag_export: None,
            daw_delivery: None,
            record: None,
            view_count: None,
            analysis: beat_workflow::Analysis::default(),
            audition: Default::default(),
            groove_editor: Default::default(),
            search: Default::default(),
            tempo_estimate: None,
            clip: None,
            detached_clip: None,
            active_source: 0,
            studio: Studio::default(),
            waveform: WaveformView::default(),
            wave_snap: Default::default(),
            wave_beats: false,
            wave_height: 118.0,
            samples: Vec::new(),
            sample_name: String::new(),
            suggestion_bars: 2,
            suggestion_emphasis: Emphasis::Balanced,
            match_priority: MatchBy::Balanced,
            sample_rename: None,
            selected_sample: None,
            samples_in_project: false,
            sent: Studio::default(),
            library,
            library_recovery: library_recovery.map(Arc::new),
            collection: None,
            new_collection: None,
            youtube_url: None,
            history: Vec::new(),
            history_cursor: None,
            demos: files::demos(),
            folder: Vec::new(),
            output,
            audio_error,
            capture_quantize: Quantize::Off,
            position: 0.0,
            seeked_at: None,
            preview: false,
            playing: false,
            song_playing: None,
            scene_active: None,
            scene_pending: None,
            scene_quantize: 1,
            scene_capture: false,
            scene_take: Vec::new(),
            song_request: 0,
            studio_view: StudioView::Sample,
            arrangement_timeline: Default::default(),
            pattern_name: String::new(),
            recording: false,
            input_recording: None,
            input_device: None,
            input_monitor: false,
            demucs_path: None,
            job_control: None,
            running_job: None,
            retry_job: None,
            click_track: false,
            click_gain: 1.0,
            count_in_bars: 0,
            replace_take: false,
            punch_in: false,
            punch_first: 1,
            punch_last: 1,
            fill: false,
            quantize: Quantize::Off,
            piano_roll: None,
            transcription: Default::default(),
            extraction: Default::default(),
            piano_roll_clipboard: Vec::new(),
            recording_project: 0,
            recorded_gates: HashMap::new(),
            step: None,
            selected_pad: 0,
            pad_mode: PadMode::Slice,
            piano_key: 60,
            piano_arm: false,
            piano_gate: false,
            piano_octave: 4,
            piano_pointer: None,
            computer_notes: [None; PIANO_KEYS.len()],
            piano_held: HashSet::new(),
            harmony_held: HashMap::new(),
            midi,
            midi_dialog: false,
            midi_devices: Vec::new(),
            midi_error: None,
            midi_output_status: None,
            midi_pickup: HashMap::new(),
            midi_output_failed: None,
            midi_selected: None,
            midi_learn: None,
            midi_target: Target::Play,
            midi_last: None,
            midi_clock_bpm: None,
            midi_clock_connection: None,
            midi_last_clock: None,
            midi_pad_held: HashMap::new(),
            midi_buttons: HashSet::new(),
            midi_sustain: HashSet::new(),
            midi_bend: HashMap::new(),
            keyboard: false,
            bar: 0,
            bar_clipboard: None,
            drum_templates: None,
            sequence_tool: None,
            sample_tool: None,
            reveal_audio_output: false,
            variations: None,
            snapshot_dialog: None,
            snapshots: Arc::default(),
            source_ids: Vec::new(),
            next_source_id: 1,
            snapshot_preview_request: None,
            snapshot_preview: None,
            taps: Vec::new(),
            status: None,
            busy: None,
            rng: rng.max(1),
        }
    }

    pub fn source_record(&self, index: usize) -> Option<&Record> {
        if index == 0 { self.record.as_ref() } else { self.studio.sources.get(index - 1) }
    }

    pub fn active_clip(&self) -> Option<&Clip> {
        self.clip.as_deref()?.source(self.active_source)
    }

    fn source_arc(&self, index: usize) -> Option<Arc<Clip>> {
        Clip::source_arc(self.clip.as_ref()?, index)
    }

    pub fn duration(&self) -> f64 {
        self.active_clip().map_or(0.0, Clip::duration)
    }

    pub fn source_duration(&self, index: usize) -> f64 {
        self.clip.as_deref().and_then(|clip| clip.source(index)).map_or(0.0, Clip::duration)
    }

    pub fn pad_range(&self, index: usize) -> Option<(f64, f64)> {
        self.studio.slice(index, self.source_duration(self.studio.pad_sources[index]))
    }

    pub fn key_range(&self, note: u8) -> Option<(f64, f64)> {
        self.studio.key_range(note, self.source_duration(self.studio.key_source(note)))
    }

    pub fn selection(&self) -> Option<(f64, f64)> {
        if self.studio.loop_source == self.active_source { self.studio.loop_range() } else { None }
    }

    pub fn select_source(&mut self, index: usize) {
        if index == self.active_source || index > self.studio.sources.len() {
            return;
        }
        self.clear_extraction();
        self.active_source = index;
        self.waveform = WaveformView::default();
        self.suggestions.clear();
        self.tempo_estimate = None;
        self.selected_sample = None;
        self.preview = false;
        self.piano_arm = false;
        self.send(Command::PreviewSource(index));
        self.seek(self.selection().map_or(0.0, |range| range.0));
    }

    pub fn set_loop_edge(&mut self, is_start: bool) {
        if self.studio.loop_source != self.active_source {
            self.studio.loop_source = self.active_source;
            self.studio.loop_in = None;
            self.studio.loop_out = None;
            self.studio.looping = false;
        }
        if is_start {
            self.studio.loop_in = Some(self.position);
        } else {
            self.studio.loop_out = Some(self.position);
        }
    }

    pub fn assign_pad_range(&mut self, pad: usize, source: usize, (start, end): (f64, f64)) {
        self.studio.pads[pad] = Some(start);
        self.studio.pad_ends[pad] = Some(end);
        self.studio.pad_sources[pad] = source;
    }

    pub fn swap_pads(&mut self, first: usize, second: usize) {
        // Resolve linked ends before moving either start marker.
        for pad in [first, second] {
            if let Some((_, end)) = self.pad_range(pad) {
                self.studio.pad_ends[pad] = Some(end);
            }
        }
        self.studio.pads.swap(first, second);
        self.studio.pad_ends.swap(first, second);
        self.studio.pad_sources.swap(first, second);
        self.studio.pad_names.swap(first, second);
        self.studio.pad_colors.swap(first, second);
        self.studio.pad_controls.swap(first, second);
        self.studio.choke_groups.pads.swap(first, second);
        let a = self.studio.instruments.pads.remove(&first);
        let b = self.studio.instruments.pads.remove(&second);
        self.studio.instruments.pads.extend(a.map(|layers| (second, layers)));
        self.studio.instruments.pads.extend(b.map(|layers| (first, layers)));
        self.selected_pad = second;
    }

    pub fn send(&self, command: Command) {
        if let Some(output) = &self.output {
            output.send(command);
        }
    }

    pub(crate) fn sync(&mut self) {
        self.studio.validate_media(self.clip.as_deref());
        self.studio.identify_working_events();
        self.studio.sync_pattern();
        if !self.source_ids.is_empty() || !self.snapshots.is_empty() || self.studio.uses_beat_maps() {
            self.sync_source_ids();
        }
        if self.sent != self.studio {
            if !self.sent.same_capture_instruments(&self.studio)
                && let Some(output) = &self.output
            {
                output.performance.invalidate_preview();
            }
            let settings = self.audio_settings();
            let mut previous = std::mem::take(&mut self.sent);
            previous.beat_maps.clear();
            previous.warp_sources.clear();
            previous.grooves.clear();
            if settings != previous {
                self.send(Command::Settings(Box::new(settings)));
            }
            self.sent = self.studio.clone();
        }
    }

    fn audio_settings(&self) -> Studio {
        let mut settings = self.studio.clone();
        settings.beat_maps.clear();
        settings.warp_sources.clear();
        settings.grooves.clear();
        settings
    }

    pub fn retry_audio(&mut self) {
        self.set_audio_output(self.library.audio.clone());
    }

    pub fn set_audio_output(&mut self, settings: crate::audio::OutputSettings) {
        if self.recording || self.input_recording.is_some() {
            return;
        }
        match Output::open(&settings) {
            Ok(output) => {
                self.stop_all();
                self.output = Some(output);
                self.audio_error = None;
                self.send(Command::Settings(Box::new(self.audio_settings())));
                self.send(Command::Clip(self.clip.clone()));
                self.send(Command::PreviewSource(self.active_source));
                self.send(Command::ClickTrack(self.click_track));
                self.send(Command::ClickGain(self.click_gain));
                self.send(Command::Fill(self.fill));
                if self.midi_clock_connection.is_some() {
                    self.midi.route_clock(
                        self.midi_clock_connection,
                        self.output.as_ref().map(Output::sender),
                        self.studio.bpm,
                    );
                }
                self.seek(self.position);
                if self.library.audio != settings {
                    self.library.audio = settings;
                    self.persist_library();
                }
            }
            Err(error) if self.output.is_some() => self.status = Some((error, true)),
            Err(error) => self.audio_error = Some(error),
        }
    }

    fn random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    pub fn submit(
        &mut self,
        name: &'static str,
        work: impl FnOnce() -> files::Result<Job> + Send + 'static,
    ) {
        if self.input_recording.is_some() {
            self.status = Some(("Finish the input recording first.".into(), false));
            return;
        }
        if self.busy.is_some() {
            return;
        }
        self.status = None;
        self.retry_job = None;
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        self.busy = Some((name, rx));
        std::thread::spawn(move || {
            let result = work().map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn submit_controlled(
        &mut self,
        name: &'static str,
        work: impl FnOnce() -> files::Result<Job> + Clone + Send + Sync + 'static,
    ) {
        self.start_controlled((name, Arc::new(move || work.clone()())));
    }

    fn start_controlled(&mut self, job: RetryJob) {
        if self.busy.is_some() || self.input_recording.is_some() {
            return;
        }
        let control = crate::background::Control::default();
        let worker_control = control.clone();
        let work = job.1.clone();
        self.submit(job.0, move || worker_control.run(|| {
            let job = work()?;
            crate::background::check()?;
            Ok(job)
        }));
        self.job_control = Some(control);
        self.running_job = Some(job);
    }

    pub fn can_retry_job(&self) -> bool {
        self.retry_job.is_some() && self.busy.is_none() && self.input_recording.is_none()
    }

    pub fn retry_background_job(&mut self) {
        if self.can_retry_job() && let Some(job) = self.retry_job.take() {
            self.start_controlled(job);
        }
    }

    pub fn dismiss_job_status(&mut self) {
        self.status = None;
        self.retry_job = None;
    }

    fn finish_job(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let Some((_, receiver)) = &self.busy else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The background job ended unexpectedly".into()),
        };
        let preview_job = self.drum_preview_pending();
        self.busy = None;
        let cancelled = self.job_control.take().is_some_and(|job| job.state().cancelled);
        let retry = self.running_job.take().filter(|_| !preview_job);
        if cancelled {
            if !preview_job {
                self.retry_job = retry;
                self.status = Some(("Operation cancelled.".into(), false));
            }
            return;
        }
        self.observe_edits(self.recording || self.ctx.input(|input| input.pointer.primary_down()));
        match result {
            Ok(Job::Loaded { .. } | Job::Opened(..))
                if matches!(self.pending_change, Some(PendingChange::Quit)) => {}
            Ok(job @ (Job::Loaded { .. } | Job::Opened(..))) if self.session.dirty() => {
                self.stop_recording();
                self.pending_change = Some(PendingChange::Replace(Box::new(job)));
            }
            Ok(job) => self.apply_job(job),
            Err(error) => {
                self.retry_job = retry;
                self.status = Some((error, true));
            }
        }
    }

    fn apply_job(&mut self, job: Job) {
        match job {
            Job::Loaded {
                record,
                clip,
                bpm,
                restore,
                history,
                start,
                open_studio,
                view_count,
            } => {
                let mut settings = restore.map_or_else(
                    || {
                        let mut settings = self.studio.clone();
                        if !settings.has_pattern() && settings.patterns.is_empty() {
                            settings.bpm = bpm;
                        }
                        settings.pads = [None; STEPS];
                        settings.pad_ends = [None; STEPS];
                        settings.warp_markers.clear();
                        settings.beat_maps.clear();
                        settings.warp_sources.clear();
                        settings.pad_names = Default::default();
                        settings.pad_colors = [0; STEPS];
                        settings.drum_sources = [None; DRUMS.len()];
                        settings.pad_controls = Default::default();
                        settings.choke_groups.pads = Default::default();
                        settings.instruments = Default::default();
                        settings.chops = Default::default();
                        settings.step_sounds = Default::default();
                        settings.step_ids = Default::default();
                        settings.step_feel = Default::default();
                        settings.event_id_next = 0;
                        settings.key_chops.clear();
                        settings.chromatic = None;
                        settings.key_controls.clear();
                        settings.key_steps = Default::default();
                        settings.performance.clear();
                        settings.editing_pattern = None;
                        settings.loop_in = None;
                        settings.loop_out = None;
                        settings.looping = false;
                        settings.speed = 1.0;
                        settings
                    },
                    |settings| *settings,
                );
                settings.validate(clip.duration());
                self.set_record(Some(record.clone()), Some(clip), settings);
                self.view_count = view_count;
                if let Some(start) = start.filter(|&start| start < self.duration()) {
                    self.seek(start);
                }
                self.history_cursor = history.or_else(|| {
                    self.remember_heard(&record.source);
                    self.history.push(record);
                    self.library.heard += 1;
                    self.persist_library();
                    Some(self.history.len() - 1)
                });
                self.session.reset(self.project_snapshot(), None, None);
                self.toggle_preview();
                if open_studio {
                    self.youtube_url = None;
                    self.page = Page::Studio;
                }
            }
            Job::Opened(project, clip, path, kind) => {
                self.set_record(project.record, clip, project.studio);
                self.samples = project.samples;
                self.source_ids = project.source_ids;
                self.next_source_id = self.source_ids.iter().map(|id| id.0).max().unwrap_or(0).saturating_add(1);
                self.snapshots = project.snapshots;
                self.sync();
                self.samples_in_project = true;
                self.history_cursor = None;
                self.page = Page::Studio;
                match kind {
                    OpenKind::Recovery => {
                        self.session.available_recovery = None;
                        self.session.reset(self.project_snapshot(), None, Some(path));
                        self.status = Some(("Session recovered. Save it as a project.".into(), false));
                    }
                    OpenKind::Template => {
                        self.session.reset(self.project_snapshot(), None, None);
                        self.status = Some(("New project created from template.".into(), false));
                    }
                    OpenKind::Project => {
                        self.session.reset(self.project_snapshot(), Some(path.clone()), None);
                        self.remember_project(path);
                        self.status = Some(("Project opened.".into(), false));
                    }
                }
            }
            Job::Saved(path, snapshot) => {
                self.session.mark_saved(path.clone(), snapshot);
                self.samples_in_project = true;
                self.remember_project(path.clone());
                self.status = Some((format!("Saved {}", path.display()), false));
                if !self.session.dirty() {
                    self.continue_change(false);
                }
            }
            Job::Folder(records) => {
                let total = records.len();
                self.folder = records;
                self.pool = Pool::Local;
                self.genres.fill(false);
                self.status = Some((format!("{total} audio files ready to dig."), false));
            }
            Job::Wrote(message) => self.status = Some((message, false)),
            Job::Tempo(source, estimate) => {
                if source == self.active_source {
                    self.tempo_estimate = Some(estimate);
                }
            }
            Job::SourceAdded(record, clip, pad) => {
                let pad = if let Some(wanted) = pad {
                    if self.studio.pads[wanted].is_none() {
                        Some(wanted)
                    } else if let Some(next) = self.studio.pads.iter().position(Option::is_none) {
                        Some(next)
                    } else {
                        self.status = Some(("Free a pad before resampling the pattern.".into(), true));
                        return;
                    }
                } else {
                    None
                };
                self.stop_all();
                self.detached_clip = None;
                let duration = clip.duration();
                if let Some(root) = &mut self.clip {
                    let root = Arc::make_mut(root);
                    root.sources.truncate(self.studio.sources.len());
                    root.sources.push(Arc::new(clip));
                    self.studio.sources.push(record);
                    self.active_source = 0;
                    self.send(Command::Clip(self.clip.clone()));
                    self.select_source(self.studio.sources.len());
                } else {
                    self.clip = Some(Arc::new(clip));
                    self.record = Some(record);
                    self.waveform = WaveformView::default();
                    self.send(Command::Clip(self.clip.clone()));
                    self.seek(0.0);
                }
                self.samples_in_project = true;
                if let Some(pad) = pad {
                    let name = self.source_record(self.active_source)
                        .map_or_else(|| "Bounce".into(), |record| record.title.clone());
                    self.assign_pad_range(pad, self.active_source, (0.0, duration));
                    self.studio.pad_names[pad] = name;
                    self.studio.pad_controls[pad] = SampleControls::default();
                    self.selected_pad = pad;
                }
                self.page = Page::Studio;
                self.studio_view = StudioView::Sample;
            }
            Job::StemsAdded(stems) => {
                let count = stems.len();
                for (record, clip) in stems {
                    self.apply_job(Job::SourceAdded(record, clip, None));
                }
                self.status = Some((format!("Added {count} separated stems."), false));
            }
            Job::DrumKit(project, clip) => self.apply_drum_kit(*project, clip),
            Job::MidiMapping(id, mapping) => {
                self.library.midi.ports.insert(id, mapping);
                self.midi_settings_changed();
                self.status = Some(("MIDI mapping imported.".into(), false));
            }
            Job::Relinked(old, replacement) => {
                for saved in &mut self.library.records {
                    if saved.record.source == old {
                        saved.record.source = replacement.clone();
                    }
                }
                for bank in &mut self.library.sample_banks {
                    if bank.source == old {
                        bank.source = replacement.clone();
                    }
                }
                for entry in &mut self.library.queue {
                    if entry.record.source == old {
                        entry.record.source = replacement.clone();
                    }
                }
                for source in &mut self.library.heard_sources {
                    if *source == old {
                        *source = replacement.clone();
                    }
                }
                self.library.taste.relink(&old, &replacement);
                self.invalidate_taste();
                self.persist_library();
                if self.status.is_none() {
                    self.status = Some(("Relinked crate audio.".into(), false));
                }
            }
            Job::Queue(entries, similar) => {
                let count = entries.len();
                for entry in entries.into_iter().rev() {
                    self.library
                        .queue
                        .retain(|queued| queued.record.source != entry.record.source);
                    self.library.queue.insert(0, entry);
                }
                self.queue_tab = Verdict::Pending;
                self.queue_similar = similar;
                self.page = Page::Queue;
                self.status = Some((format!("{count} tracks added to the queue."), false));
                self.invalidate_taste();
                self.persist_library();
            }
            Job::Listen(source, clip, range) if self.page == Page::Queue && self.output.is_some() => {
                self.queue_audio = Some(QueueAudio {
                    source,
                    clip: Arc::new(clip),
                    matched_range: range,
                });
                self.play_queue_audio(None);
            }
            Job::Listen(..) => {}
            Job::DrumTemplatePreview(index, use_groove, clip) => {
                if self.can_preview_drum_template() && self.drum_templates.as_mut()
                    .is_some_and(|browser| browser.accept_preview(index, use_groove))
                {
                    self.stop_listening();
                    self.preview = false;
                    self.send(Command::Play(false));
                    let duration = clip.duration();
                    self.send(Command::Listen(Arc::new(clip), (0.0, duration), 1.0, None));
                }
            }
            Job::Suggestions(source, suggestions) => {
                if source != self.active_source {
                    return;
                }
                self.status = Some((
                    if suggestions.is_empty() {
                        "No audible ranges found.".into()
                    } else {
                        format!("{} cuts ready to audition.", suggestions.len())
                    },
                    false,
                ));
                self.suggestions = suggestions;
            }
            Job::Delivered(delivery) => {
                self.status = Some((
                    format!(
                        "Exported to {} of {} destinations.",
                        delivery.written.len(),
                        delivery.written.len() + delivery.failed.len()
                    ),
                    !delivery.failed.is_empty(),
                ));
                self.daw_delivery = Some(delivery);
            }
            Job::SnapshotPreview(request, clips, gains) => {
                if self.snapshot_preview_request == Some(request) {
                    self.snapshot_preview = Some((clips, gains));
                }
            }
            #[cfg(windows)]
            Job::DragPrepared(path) => {
                self.drag_export = Some(path);
                self.status = Some(("WAV ready to drag.".into(), false));
            }
        }
    }

    fn set_record(&mut self, record: Option<Record>, clip: Option<Clip>, mut studio: Studio) {
        self.reset_analysis();
        self.clear_transcription();
        self.clear_extraction();
        self.stop_beat_audition();
        self.groove_editor = Default::default();
        self.scene_active = None;
        self.scene_pending = None;
        self.scene_capture = false;
        self.scene_take.clear();
        self.stop_listening();
        self.suggestions.clear();
        self.tempo_estimate = None;
        self.playing = false;
        self.song_playing = None;
        self.studio_view = StudioView::Sample;
        self.variations = None;
        self.snapshot_dialog = None;
        self.snapshots = Arc::default();
        self.source_ids.clear();
        self.next_source_id = 1;
        self.clear_snapshot_preview();
        self.arrangement_timeline = Default::default();
        self.stop_recording();
        self.recording_project += 1;
        self.piano_roll = None;
        self.piano_roll_clipboard.clear();
        self.bar_clipboard = None;
        self.recorded_gates.clear();
        self.preview = false;
        self.step = None;
        self.send(Command::Pattern(false));
        self.send(Command::Play(false));
        self.record = record;
        self.view_count = None;
        self.active_source = 0;
        self.detached_clip = None;
        self.clip = clip.map(Arc::new);
        studio.validate_media(self.clip.as_deref());
        studio.validate_source_analysis(self.clip.as_deref());
        studio.validate_song();
        studio.identify_saved_events();
        self.studio = studio;
        self.waveform = WaveformView::default();
        self.samples = self
            .record
            .as_ref()
            .and_then(|record| {
                self.library
                    .sample_banks
                    .iter()
                    .find(|bank| bank.source == record.source)
            })
            .map_or_else(Vec::new, |bank| bank.samples.clone());
        let duration = self.duration();
        NamedSample::validate_list(&mut self.samples, duration);
        self.samples_in_project = false;
        self.sample_name.clear();
        self.sample_rename = None;
        self.selected_sample = None;
        self.taps.clear();
        self.bar = 0;
        self.selected_pad = 0;
        self.send(Command::Clip(self.clip.clone()));
        self.sync();
        let source = self.studio.loop_source;
        if source == 0 {
            self.seek(self.studio.loop_in.unwrap_or(0.0));
        } else {
            self.select_source(source);
        }
    }

    pub fn load(&mut self, record: Record, restore: Option<Studio>, history: Option<usize>) {
        self.submit_controlled("Loading audio…", move || {
            let (clip, bpm) = files::load(&record)?;
            Ok(Job::Loaded {
                record,
                clip,
                bpm,
                restore: restore.map(Box::new),
                history,
                start: None,
                open_studio: false,
                view_count: None,
            })
        });
    }

    pub fn save_dig_preset(&mut self) {
        let name = self.preset_name.trim().to_owned();
        if name.is_empty() || self.discovery_filters().is_none() {
            return;
        }
        self.library.dig_presets.retain(|preset| preset.name != name);
        self.library.dig_presets.insert(0, DigPreset {
            name,
            genres: self.genres,
            decade: self.dig_decade,
            max_views: self.dig_max_views.clone(),
            year: self.release_year.clone(),
            country: self.release_country.clone(),
            label: self.release_label.clone(),
        });
        self.library.dig_presets.truncate(24);
        self.preset_name.clear();
        self.persist_library();
    }

    pub fn apply_dig_preset(&mut self, index: usize) {
        if let Some(preset) = self.library.dig_presets.get(index) {
            self.genres = preset.genres;
            self.dig_decade = preset.decade;
            self.dig_max_views = preset.max_views.clone();
            self.release_year = preset.year.clone();
            self.release_country = preset.country.clone();
            self.release_label = preset.label.clone();
            self.pool = Pool::YouTube;
        }
    }

    fn discovery_exclusions(&self) -> Vec<Source> {
        let mut sources = Vec::new();
        if self.unheard_only {
            sources.extend(self.library.heard_sources.iter().cloned());
        }
        if self.exclude_saved {
            sources.extend(self.library.records.iter().map(|saved| saved.record.source.clone()));
        }
        if self.exclude_skipped {
            sources.extend(self.library.queue.iter().filter(|entry| entry.verdict == Verdict::Skip)
                .map(|entry| entry.record.source.clone()));
        }
        sources
    }

    pub fn dig(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let seed = self.random();
        let index = seed as usize;
        let selected = GENRES
            .iter()
            .zip(self.genres)
            .filter_map(|(&genre, selected)| selected.then_some(genre))
            .collect::<Vec<_>>();
        let exclusions = self.discovery_exclusions();
        if self.pool == Pool::YouTube {
            let Some((filters, release)) = self.discovery_filters() else {
                return;
            };
            let genres = if selected.is_empty() {
                &GENRES[1..]
            } else {
                &selected
            };
            let genre = genres[index % genres.len()];
            let active = self.record.as_ref().and_then(youtube_id).map(str::to_owned);
            let heard = self
                .library
                .heard_sources
                .iter()
                .filter_map(source_youtube_id)
                .map(str::to_owned)
                .collect::<HashSet<_>>();
            let mut excluded: HashSet<_> = exclusions.iter().filter_map(source_youtube_id)
                .map(str::to_owned).collect();
            excluded.extend(active.clone());
            if release.active() {
                excluded.extend(heard.iter().cloned());
            }
            let use_discovery = release.active() || !exclusions.is_empty();
            self.submit_controlled("Digging YouTube records…", move || {
                if use_discovery {
                    let entries = crate::youtube::discover(genre, seed, &excluded, filters, &release)?;
                    for entry in entries.into_iter().take(3) {
                        crate::background::check()?;
                        if let Ok((clip, bpm)) = files::load(&entry.record) {
                            return Ok(Job::Loaded {
                                record: entry.record, clip, bpm, restore: None, history: None,
                                start: None, open_studio: true, view_count: entry.views,
                            });
                        }
                    }
                    return Err("Matching YouTube audio could not be loaded. Try broader filters.".into());
                }
                let result = crate::youtube::dig(
                    genre,
                    seed.rotate_right(17),
                    active.as_deref(),
                    &heard,
                    filters,
                )?;
                Ok(Job::Loaded {
                    record: result.record,
                    clip: result.clip,
                    bpm: 90,
                    restore: None,
                    history: None,
                    start: None,
                    open_studio: true,
                    view_count: result.views,
                })
            });
            return;
        }
        let active = self.record.clone();
        if self.pool == Pool::Commons {
            let genre = if selected.is_empty() {
                "All"
            } else {
                selected[index % selected.len()]
            };
            self.submit_controlled("Digging Wikimedia Commons…", move || {
                let mut options = files::online_records(genre)?;
                options.retain(|record| !exclusions.contains(&record.source));
                if options.is_empty() {
                    return Err("No Commons records match the current exclusions.".into());
                }
                let record = select_record(&options, active.as_ref(), index);
                let (clip, bpm) = files::load(&record)?;
                Ok(Job::Loaded {
                    record,
                    clip,
                    bpm,
                    restore: None,
                    history: None,
                    start: None,
                    open_studio: false,
                    view_count: None,
                })
            });
            return;
        }
        let records = if self.pool == Pool::Local {
            &self.folder
        } else {
            &self.demos
        };
        let options: Vec<_> = records
            .iter()
            .filter(|record| !exclusions.contains(&record.source)
                && (selected.is_empty() || selected.contains(&record.genre.as_str())))
            .cloned()
            .collect();
        if options.is_empty() {
            let message = if records.is_empty() {
                "Choose a local audio folder from the source menu."
            } else {
                "No matches in this crate. Try All or switch to an online source."
            };
            self.status = Some((message.into(), true));
            return;
        }
        self.load(select_record(&options, active.as_ref(), index), None, None);
    }

    fn discovery_filters(&mut self) -> Option<(DigFilters, ReleaseFilters)> {
        let input = self.dig_max_views.trim();
        let max_views = if input.is_empty() {
            None
        } else {
            match input.replace(',', "").parse::<u64>() {
                Ok(count) => Some(count),
                Err(_) => {
                    self.status = Some((
                        "Enter a whole number for Max views, or leave it blank.".into(),
                        true,
                    ));
                    return None;
                }
            }
        };
        let year = self.release_year.trim();
        let year = if year.is_empty() {
            None
        } else {
            match year.parse::<u16>() {
                Ok(year) if (1900..=2099).contains(&year) => Some(year),
                _ => {
                    self.status = Some((
                        "Enter a four-digit release year, or leave it blank.".into(),
                        true,
                    ));
                    return None;
                }
            }
        };
        if let (Some(year), Some(decade)) = (year, self.dig_decade)
            && !(decade..decade + 10).contains(&year)
        {
            self.status = Some(("Release year is outside the selected decade.".into(), true));
            return None;
        }
        Some((
            DigFilters {
                decade: self.dig_decade,
                max_views,
            },
            ReleaseFilters {
                year,
                country: self.release_country.trim().to_owned(),
                label: self.release_label.trim().to_owned(),
            },
        ))
    }

    pub fn build_queue(&mut self, sample: Option<usize>) {
        if self.busy.is_some() {
            return;
        }
        let seed = self.random();
        let genres: Vec<_> = GENRES
            .iter()
            .zip(self.genres)
            .filter_map(|(&genre, on)| on.then_some(genre))
            .collect();
        let pool = self.pool;
        let (filters, release) = if pool == Pool::YouTube {
            let Some(filters) = self.discovery_filters() else {
                return;
            };
            filters
        } else {
            (DigFilters::default(), ReleaseFilters::default())
        };
        let reference =
            sample.and_then(|index| {
                let sample = self.samples.get(index)?;
                Some((self.source_arc(sample.source)?, sample.range()))
            });
        if sample.is_some() && reference.is_none() {
            return;
        }
        let genre = if genres.is_empty() {
            if release.active() || pool != Pool::YouTube {
                "All"
            } else {
                GENRES[1 + seed as usize % (GENRES.len() - 1)]
            }
        } else {
            genres[seed as usize % genres.len()]
        };
        let mut excluded = self.discovery_exclusions();
        excluded.extend(
            self.library
                .queue
                .iter()
                .map(|entry| entry.record.source.clone()),
        );
        if let Some(record) = &self.record {
            excluded.push(record.source.clone());
        }
        let local = match pool {
            Pool::Demo => self.demos.clone(),
            Pool::Local => self.folder.clone(),
            _ => Vec::new(),
        };
        let task = if reference.is_some() {
            "Finding similar sounds…"
        } else {
            "Finding records for the queue…"
        };
        let priority = self.match_priority;
        self.submit_controlled(task, move || {
            let reference: Option<Sound> = reference
                .map(|(clip, range)| {
                    AudioProfile::new(&clip, (range.0, range.1.min(range.0 + 16.0)))
                        .sound()
                        .ok_or("Choose a named cut with at least half a second of audible audio.")
                })
                .transpose()?;
            let mut entries = if pool == Pool::YouTube {
                let ids = excluded
                    .iter()
                    .filter_map(source_youtube_id)
                    .map(str::to_owned)
                    .collect();
                crate::youtube::discover(genre, seed, &ids, filters, &release)?
            } else {
                let mut records = if pool == Pool::Commons {
                    files::online_records(genre)?
                } else {
                    local
                };
                records.retain(|record| {
                    !excluded.contains(&record.source)
                        && (genres.is_empty() || genres.contains(&record.genre.as_str()))
                });
                if !records.is_empty() {
                    let offset = seed as usize % records.len();
                    records.rotate_left(offset);
                }
                records
                    .into_iter()
                    .take(12)
                    .map(|record| QueueEntry::new(record, None))
                    .collect()
            };
            if let Some(reference) = reference {
                crate::background::stage("Comparing candidate audio")?;
                entries = entries
                    .into_iter()
                    .take(6)
                    .filter_map(|mut entry| {
                        crate::background::check().ok()?;
                        let (clip, _) = files::load(&entry.record).ok()?;
                        let (range, score, aspects) = AudioProfile::new(&clip, (0.0, clip.duration()))
                            .best_match(reference, priority)?;
                        entry.match_range = Some((range.0, range.1.min(clip.duration())));
                        entry.similarity = Some(score);
                        entry.similarity_aspects = Some(aspects);
                        Some(entry)
                    })
                    .collect();
                entries.sort_by_key(|entry| std::cmp::Reverse(entry.similarity));
            }
            if entries.is_empty() {
                return Err(
                    "No new playable tracks found. Change the Dig filters or source.".into(),
                );
            }
            Ok(Job::Queue(entries, reference.is_some()))
        });
    }

    pub fn preview_queue(&mut self, index: usize) {
        let entry = self.library.queue[index].clone();
        self.queue_focus = Some(entry.record.source.clone());
        if self.queue_playing.as_ref() == Some(&entry.record.source) {
            self.stop_listening();
            return;
        }
        if self.queue_audio.as_ref().is_some_and(|audio| audio.source == entry.record.source) {
            self.play_queue_audio(None);
            return;
        }
        self.submit_controlled("Loading queue preview…", move || {
            let (clip, _) = files::load(&entry.record)?;
            Ok(Job::Listen(entry.record.source, clip, entry.match_range))
        });
    }

    fn play_queue_audio(&mut self, seek: Option<f64>) {
        let Some(QueueAudio { source, clip, matched_range }) = self.queue_audio.clone() else {
            return;
        };
        if self.output.is_none() { return; }
        let duration = clip.duration();
        let seconds = self.queue_preview_seconds;
        let start = (duration * self.queue_preview_point).min((duration - seconds).max(0.0));
        let range = if let Some(seek) = seek {
            let start = seek.clamp(0.0, (duration - 0.005).max(0.0));
            (start, (start + seconds).min(duration))
        } else { matched_range.unwrap_or((start, (start + seconds).min(duration))) };
        let gain = if self.queue_level_match {
            let (sum, count) = clip.region(range).unwrap_or_default().iter().step_by(
                ((range.1 - range.0) * f64::from(clip.rate) / 8192.0).round().max(1.0) as usize,
            ).fold((0.0_f32, 0), |(sum, count), frame| {
                (sum + (frame[0] * frame[0] + frame[1] * frame[1]) * 0.5, count + 1)
            });
            let rms = (sum / count.max(1) as f32).sqrt();
            (0.18 / rms.max(0.01)).clamp(0.1, 4.0)
        } else { 1.0 };
        self.set_playing(false);
        self.preview = false;
        self.send(Command::Play(false));
        self.send(Command::Listen(clip, range, gain, None));
        let mut changed = false;
        if let Some(entry) = self.library.queue.iter_mut().find(|entry| entry.record.source == source) {
            changed = !entry.auditioned;
            entry.auditioned = true;
        }
        if self.remember_heard(&source) {
            self.library.heard += 1;
            changed = true;
        }
        self.queue_playing = Some(source);
        self.queue_range = Some(range);
        self.queue_preview_started = Some(Instant::now());
        if changed { self.persist_library(); }
    }

    pub fn queue_audition(&self) -> Option<(f64, f64)> {
        let QueueAudio { source, clip, .. } = self.queue_audio.as_ref()?;
        let focused = self.queue_focused()?;
        if self.library.queue[focused].record.source != *source { return None; }
        let range = self.queue_range.unwrap_or((0.0, clip.duration()));
        let position = self.queue_preview_started.map_or(range.0, |start| (range.0 + start.elapsed().as_secs_f64()).min(range.1));
        Some((position, clip.duration()))
    }

    pub fn scrub_queue(&mut self, position: f64) {
        if self.queue_audition().is_some() { self.play_queue_audio(Some(position)); }
    }

    pub fn open_queue(&mut self, index: usize) {
        let entry = self.library.queue[index].clone();
        self.submit_controlled("Opening queued track in Studio…", move || {
            let (clip, bpm) = files::load(&entry.record)?;
            Ok(Job::Loaded {
                record: entry.record,
                clip,
                bpm,
                restore: None,
                history: None,
                start: entry.match_range.map(|range| range.0),
                open_studio: true,
                view_count: entry.views,
            })
        });
    }

    pub fn queue_decide(&mut self, index: usize, verdict: Verdict) {
        self.queue_decide_with_reason(index, verdict, Default::default());
    }

    pub fn queue_decide_with_reason(&mut self, index: usize, verdict: Verdict, reason: crate::recommendations::SkipReason) {
        let Some(entry) = self.library.queue.get(index) else { return };
        let source = entry.record.source.clone();
        self.decide_queue_sources(&[source], verdict, Some(index), reason);
    }

    pub fn queue_decide_marked(&mut self, verdict: Verdict) {
        let marked: Vec<_> = self.library.queue.iter()
            .filter(|entry| self.queue_visible(entry) && self.queue_marked.contains(&entry.record.source))
            .map(|entry| entry.record.source.clone()).collect();
        self.decide_queue_sources(&marked, verdict, None, Default::default());
    }

    fn decide_queue_sources(
        &mut self, sources: &[Source], verdict: Verdict, from: Option<usize>, reason: crate::recommendations::SkipReason,
    ) {
        let order = self.queue_indices();
        if !self.library.taste.decide(&mut self.library.queue, sources, verdict, reason) { return; }
        self.queue_marked.retain(|source| !sources.contains(source));
        if self.queue_playing.as_ref().is_some_and(|source| sources.contains(source)) {
            self.stop_listening();
        }
        if self.audition.preview.as_ref().and_then(|preview| preview.queue_origin.as_ref())
            .is_some_and(|source| sources.contains(source))
        {
            self.stop_beat_audition();
        }
        if self.queue_advance {
            let next = from.and_then(|from| order.iter().position(|&index| index == from)).map_or(0, |index| index + 1);
            self.queue_focus = order[next..].iter().chain(&order[..next])
                .map(|&index| &self.library.queue[index])
                .find(|entry| self.queue_visible(entry)).map(|entry| entry.record.source.clone());
        }
        self.invalidate_taste();
        self.persist_library();
    }

    pub fn undo_queue(&mut self) {
        if let Some((source, verdict)) = self.library.taste.undo(&mut self.library.queue) {
            self.queue_tab = verdict;
            self.queue_focus = Some(source);
            self.invalidate_taste();
            self.persist_library();
        }
    }

    fn queue_visible(&self, entry: &QueueEntry) -> bool {
        entry.verdict == self.queue_tab && (!self.queue_similar || entry.similarity.is_some())
    }

    pub fn queue_focused(&self) -> Option<usize> {
        let order = self.queue_indices();
        self.queue_focus.as_ref().and_then(|source| {
            order.iter().copied().find(|&index| self.library.queue[index].record.source == *source)
        }).or_else(|| order.first().copied())
    }

    fn stop_listening(&mut self) {
        self.extraction.playing = false;
        self.queue_playing = None;
        self.send(Command::StopListening);
        self.queue_preview_started = None;
    }

    fn remember_heard(&mut self, source: &Source) -> bool {
        if self.library.heard_sources.contains(source) {
            return false;
        }
        self.library.heard_sources.push(source.clone());
        true
    }

    pub fn suggest_cuts(&mut self) {
        let source = self.active_source;
        let Some(clip) = self.source_arc(source) else {
            return;
        };
        let span = f64::from(self.suggestion_bars) * 240.0 / f64::from(self.studio.bpm);
        let emphasis = self.suggestion_emphasis;
        self.submit_controlled("Analyzing the waveform…", move || {
            let duration = clip.duration();
            let suggestions = AudioProfile::new(&clip, (0.0, duration)).suggestions(span, duration, emphasis);
            Ok(Job::Suggestions(source, suggestions))
        });
    }

    pub fn select_suggestion(&mut self, index: usize, save: bool) {
        let sample = self.suggestions[index].clone();
        let range = sample.range();
        self.selected_sample = None;
        self.waveform.reveal(range, self.duration());
        self.set_loop_range(range);
        self.seek(range.0);
        if save {
            self.sample_name = sample.name;
            self.save_selection();
        }
    }

    pub fn add_daw_targets(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Choose DAW destination folders")
            .pick_folders()
        else {
            return;
        };
        for path in paths {
            if let Some(target) = self
                .library
                .daw_targets
                .iter_mut()
                .find(|target| target.path == path)
            {
                target.enabled = true;
            } else {
                self.library.daw_targets.push(DawTarget {
                    path,
                    enabled: true,
                });
            }
        }
        self.persist_library();
    }

    pub fn export_daw(&mut self) {
        self.read_recorded_chops();
        self.sync();
        let targets: Vec<_> = self
            .library
            .daw_targets
            .iter()
            .filter(|target| target.enabled)
            .map(|target| target.path.clone())
            .collect();
        if self.busy.is_some() || targets.is_empty() {
            return;
        }
        self.daw_delivery = None;
        let title = self
            .record
            .as_ref()
            .map_or_else(|| "Lost Samples Flip".into(), |record| record.title.clone());
        let clip = self.clip.clone();
        let mut studio = self.studio.clone();
        if self.daw_pattern_only {
            studio.sections.clear();
        }
        let samples = self.samples.clone();
        let options = self.daw_options;
        self.submit_controlled("Rendering DAW pack…", move || {
            Ok(Job::Delivered(crate::daw::export_with(
                &targets, &title, studio, clip, &samples, options,
            )?))
        });
    }

    pub fn select_history(&mut self, index: usize) {
        if let Some(record) = self.history.get(index) {
            self.load(record.clone(), None, Some(index));
        }
    }

    pub fn prev(&mut self) {
        if let Some(index) = self.history_cursor.and_then(|i| i.checked_sub(1)) {
            self.select_history(index);
        }
    }

    pub fn next(&mut self) {
        match self.history_cursor {
            Some(index) if index + 1 < self.history.len() => self.select_history(index + 1),
            _ => self.dig(),
        }
    }

    pub fn import(&mut self) {
        if self.busy.is_none()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Audio files", files::AUDIO_EXTENSIONS)
                .pick_file()
        {
            self.load(files::local_record(path), None, None);
        }
    }

    fn project_frame_count(&self) -> usize {
        self.clip.as_ref().map_or(0, |root| {
            root.frames.len()
                + root.sources.iter().take(self.studio.sources.len())
                    .map(|clip| clip.frames.len()).sum::<usize>()
        })
    }

    pub fn can_add_source(&self) -> bool {
        self.busy.is_none() && self.input_recording.is_none()
            && self.studio.sources.len() + 1 < crate::model::MAX_SOURCES
    }

    pub fn record_input(&mut self) {
        if !self.can_add_source() || self.recording { return; }
        let available = files::MAX_PROJECT_FRAMES.saturating_sub(self.project_frame_count()).min(files::MAX_PROJECT_FRAMES / 2);
        match crate::input_audio::Recording::start(self.input_device.as_deref(), available) {
            Ok((recording, monitor)) => {
                recording.monitor(self.input_monitor && self.output.is_some());
                self.send(Command::InputMonitor(Some(monitor)));
                self.input_recording = Some(recording);
                self.status = None;
            }
            Err(error) => self.status = Some((error, true)),
        }
    }

    pub fn finish_input(&mut self) {
        let Some(recording) = self.input_recording.take() else { return };
        let title = format!("Input recording · {}", recording.name);
        self.send(Command::InputMonitor(None));
        let (clip, reason) = recording.finish();
        if let Some(clip) = clip {
            let record = Record {
                title, artist: "Project audio".into(), genre: String::new(),
                source: Source::Generated(self.random()), release: None,
            };
            self.apply_job(Job::SourceAdded(record, clip, None));
            self.status = Some((reason.unwrap_or("Recording added to the project.").into(), false));
        } else {
            self.status = Some((
                reason.unwrap_or("The input did not capture any audio.").into(),
                true,
            ));
        }
    }

    pub fn add_source(&mut self) {
        if self.clip.is_none() {
            self.import();
            return;
        }
        if !self.can_add_source() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Add a project source")
            .add_filter("Audio files", files::AUDIO_EXTENSIONS)
            .pick_file()
        else { return };
        let total = self.project_frame_count();
        let record = files::local_record(path);
        self.submit_controlled("Adding project source…", move || {
            let (clip, _) = files::load(&record)?;
            if total + clip.frames.len() > files::MAX_PROJECT_FRAMES {
                return Err("Adding this source would exceed the project audio limit.".into());
            }
            Ok(Job::SourceAdded(record, clip, None))
        });
    }

    pub fn separate_stems(&mut self) {
        if !self.can_add_source() || self.recording
            || self.studio.sources.len() + 1 + crate::separation::STEMS.len() > crate::model::MAX_SOURCES
        { return; }
        let Some(clip) = self.source_arc(self.active_source) else { return };
        let range = self.selection().unwrap_or((0.0, clip.duration()));
        let program = self.demucs_path.clone();
        let remaining = files::MAX_PROJECT_FRAMES.saturating_sub(self.project_frame_count());
        let names: Vec<_> = crate::separation::STEMS.into_iter().map(|stem| Record {
            title: format!("Stem · {stem}"), artist: "Project audio".into(), genre: String::new(),
            source: Source::Generated(self.random()), release: None,
        }).collect();
        self.submit_controlled("Separating stems locally…", move || {
            let stems = crate::separation::separate(program.as_deref(), &clip, range, remaining)?;
            Ok(Job::StemsAdded(names.into_iter().zip(stems.into_iter().map(|(_, clip)| clip)).collect()))
        });
    }

    pub fn save_drum_kit(&mut self) {
        if self.busy.is_some() { return; }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save reusable drum kit").add_filter("Lost Samples kit", &["flipkit"])
            .set_file_name("drums.flipkit").save_file()
        else { return };
        let mut kit = Studio {
            kit: self.studio.kit,
            drum_mix: self.studio.drum_mix,
            filter: self.studio.filter,
            reverb: self.studio.reverb,
            delay: self.studio.delay,
            delay_division: self.studio.delay_division,
            drum_fx_bypass: self.studio.drum_fx_bypass,
            ..Studio::default()
        };
        let mut source_map = HashMap::new();
        kit.choke_groups.drums = self.studio.choke_groups.drums;
        let mut sources = Vec::new();
        for (lane, source) in self.studio.drum_sources.into_iter().enumerate() {
            let Some(source) = source else { continue };
            let (Some(record), Some(clip)) = (self.source_record(source), self.source_arc(source)) else { continue };
            let index = *source_map.entry(source).or_insert_with(|| {
                let index = sources.len();
                sources.push((record.clone(), clip));
                index
            });
            kit.drum_sources[lane] = Some(index);
        }
        let (record, audio) = if sources.is_empty() {
            (None, None)
        } else {
            let (record, first) = sources.remove(0);
            let mut audio = (*first).clone();
            audio.sources.clear();
            for (record, clip) in sources {
                kit.sources.push(record);
                audio.sources.push(clip);
            }
            (Some(record), Some(audio))
        };
        let project = Arc::new(Project { record, studio: kit, ..Project::default() });
        self.submit_controlled("Saving drum kit…", move || {
            files::publish_export(&path, |output| files::save_project(output, &project, audio.as_ref()))?;
            Ok(Job::Wrote(format!("Saved drum kit {}", path.display())))
        });
    }

    pub fn load_drum_kit(&mut self) {
        if self.busy.is_some() || self.recording || self.input_recording.is_some() { return; }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Load a reusable drum kit").add_filter("Lost Samples kit", &["flipkit"])
            .pick_file()
        else { return };
        self.submit_controlled("Loading drum kit…", move || {
            let (project, audio) = files::open_project(&path)?;
            Ok(Job::DrumKit(Box::new(project), audio))
        });
    }

    fn apply_drum_kit(&mut self, project: Project, audio: Option<Clip>) {
        let Project { record, studio: kit, .. } = project;
        let offset = self.clip.as_ref().map_or(0, |_| self.studio.sources.len() + 1);
        if let Some(mut first) = audio {
            let Some(record) = record else {
                self.status = Some(("The kit is missing its first drum source.".into(), true));
                return;
            };
            let frames = first.frames.len() + first.sources.iter().map(|clip| clip.frames.len()).sum::<usize>();
            if offset + kit.sources.len() + 1 > crate::model::MAX_SOURCES
                || self.project_frame_count() + frames > files::MAX_PROJECT_FRAMES
            {
                self.status = Some(("The kit needs more project audio space.".into(), true));
                return;
            }
            self.stop_all();
            let sources = std::mem::take(&mut first.sources);
            if let Some(root) = &mut self.clip {
                let root = Arc::make_mut(root);
                root.sources.truncate(offset - 1);
                root.sources.push(Arc::new(first));
                root.sources.extend(sources);
                self.studio.sources.push(record);
            } else {
                first.sources = sources;
                self.clip = Some(Arc::new(first));
                self.record = Some(record);
            }
            self.studio.sources.extend(kit.sources);
            self.samples_in_project = true;
            self.send(Command::Clip(self.clip.clone()));
            self.send(Command::PreviewSource(self.active_source));
            self.seek(self.position);
        }
        self.studio.kit = kit.kit;
        self.studio.drum_sources = kit.drum_sources.map(|source| source.map(|index| index + offset));
        self.studio.drum_mix = kit.drum_mix;
        self.studio.choke_groups.drums = kit.choke_groups.drums;
        self.studio.filter = kit.filter;
        self.studio.reverb = kit.reverb;
        self.studio.delay = kit.delay;
        self.studio.delay_division = kit.delay_division;
        self.studio.drum_fx_bypass = kit.drum_fx_bypass;
        self.status = Some(("Drum kit applied.".into(), false));
    }

    pub fn resample_mix(&mut self, song: bool) {
        if !self.can_add_source() || self.recording {
            return;
        }
        let Some(pad) = self.studio.pads.iter().position(Option::is_none) else { return };
        self.read_recorded_chops();
        self.sync();
        let sequence = if song {
            let Some(sequence) = self.studio.song_sequence() else { return };
            sequence
        } else {
            if !self.studio.has_pattern() && self.studio.active_loop().is_none() {
                return;
            }
            self.studio.active_sequence()
        };
        let studio = self.studio.clone();
        let clip = self.clip.clone();
        let remaining = files::MAX_PROJECT_FRAMES.saturating_sub(self.project_frame_count());
        let record = Record {
            title: format!("Bounce {:02}", self.studio.sources.len() + 1),
            artist: "Project audio".into(),
            genre: String::new(),
            source: Source::Generated(self.random()),
            release: None,
        };
        self.submit_controlled(if song { "Resampling arrangement…" } else { "Resampling pattern…" }, move || {
            let audio = files::bounce_mix(studio, clip, sequence, remaining)?;
            Ok(Job::SourceAdded(record, audio, Some(pad)))
        });
    }

    pub fn resample_selected_pad(&mut self) {
        if !self.can_add_source() || self.recording { return; }
        let Some(destination) = self.studio.pads.iter().position(Option::is_none) else { return };
        let pad = self.selected_pad;
        if self.pad_range(pad).is_none() { return; }
        let Some(clip) = self.clip.clone() else { return };
        let Some(source) = clip.source(self.studio.pad_sources[pad]) else { return };
        let rate = source.rate;
        let remaining = files::MAX_PROJECT_FRAMES.saturating_sub(self.project_frame_count());
        let studio = self.studio.clone();
        let record = Record {
            title: format!("Processed pad {:02}", pad + 1),
            artist: "Project audio".into(),
            genre: String::new(),
            source: Source::Generated(self.random()),
            release: None,
        };
        self.submit_controlled("Resampling pad…", move || {
            let velocity = studio.pad_layers(pad).preview_velocity();
            let render = crate::audio::pad_frames(&clip, &studio, pad, velocity, rate)
                .ok_or("The pad has no playable layers.")?;
            let mut frames = Vec::new();
            for (index, frame) in render.take(remaining + 1).enumerate() {
                if index.is_multiple_of(4096) {
                    crate::background::check()?;
                }
                frames.push(frame);
            }
            if frames.is_empty() || frames.len() > remaining {
                return Err("The processed pad exceeds the remaining project audio space.".into());
            }
            Ok(Job::SourceAdded(record, Clip::new(rate, frames), Some(destination)))
        });
    }

    pub fn import_youtube(&mut self, input: &str) {
        match crate::youtube::VideoLink::parse(input) {
            Ok(link) => self.submit_controlled("Importing YouTube audio…", move || {
                let (record, clip) = crate::youtube::load(&link.id)?;
                Ok(Job::Loaded {
                    record,
                    clip,
                    bpm: 90,
                    restore: None,
                    history: None,
                    start: link.start,
                    open_studio: true,
                    view_count: None,
                })
            }),
            Err(error) => self.status = Some((error.to_string(), true)),
        }
    }

    pub fn export_region(&mut self, range: (f64, f64), filename: &str) {
        self.export_source_region(self.active_source, range, filename);
    }

    pub fn export_cut(&mut self, pad: bool, processed: bool, options: files::WavOptions) {
        if self.busy.is_some() { return; }
        let (range, source, controls, name) = if pad {
            let pad = self.selected_pad;
            let Some(range) = self.pad_range(pad) else { return };
            (range, self.studio.pad_sources[pad], self.studio.pad_controls[pad], format!("pad-{:02}.wav", pad + 1))
        } else {
            let Some(range) = self.selection() else { return };
            (range, self.active_source, SampleControls::default(), "selection.wav".into())
        };
        let Some(clip) = self.source_arc(source) else { return };
        let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).set_file_name(name).save_file()
        else { return };
        let controls = processed.then_some((controls, self.studio.speed));
        let processed_pad = (pad && processed).then(|| (self.studio.clone(), self.clip.clone(), self.selected_pad));
        self.submit_controlled("Exporting cut…", move || {
            files::publish_export(&path, |output| {
                if let Some((studio, Some(root), index)) = &processed_pad {
                    files::export_pad(output, root, studio, *index, options)
                } else {
                    files::export_cut(output, &clip, range, controls, options)
                }
            })?;
            Ok(Job::Wrote(format!("Exported {}", path.display())))
        });
    }

    #[cfg(windows)]
    pub fn prepare_drag_export(&mut self, kind: crate::platform::drag_export::Export, options: files::WavOptions) {
        if self.busy.is_some() || self.recording || self.input_recording.is_some() {
            return;
        }
        self.read_recorded_chops();
        self.sync();
        let studio = self.studio.clone();
        let clip = self.clip.clone();
        self.submit_controlled("Preparing drag WAV…", move || {
            use crate::platform::drag_export;
            let path = drag_export::prepare(&drag_export::directory()?, kind, studio.clone(), clip.clone(), options)?;
            Ok(Job::DragPrepared(path))
        });
    }

    #[cfg(windows)]
    pub fn delete_drag_export(&mut self) {
        if let Some(path) = &self.drag_export {
            match std::fs::remove_file(path) {
                Ok(()) => self.drag_export = None,
                Err(error) => self.status = Some((format!("Could not delete the export: {error}"), true)),
            }
        }
    }

    pub fn export_source_region(&mut self, source: usize, range: (f64, f64), filename: &str) {
        if self.busy.is_some() {
            return;
        }
        let Some(clip) = self.source_arc(source) else {
            return;
        };
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV", &["wav"])
            .set_file_name(filename)
            .save_file()
        {
            self.submit_controlled("Exporting sample…", move || {
                files::publish_export(&path, |output| files::export_region(output, &clip, range))?;
                Ok(Job::Wrote(format!("Exported {}", path.display())))
            });
        }
    }

    pub fn save_selection(&mut self) {
        let Some((start, end)) = self.selection() else {
            return;
        };
        if self.samples.len() >= MAX_NAMED_SAMPLES {
            return;
        }
        let name = self.sample_name.trim();
        let name = if name.is_empty() {
            format!("Sample {}", self.samples.len() + 1)
        } else {
            name.to_owned()
        };
        self.samples.push(NamedSample { source: self.active_source, name, start, end });
        self.selected_sample = Some(self.samples.len() - 1);
        self.sample_name.clear();
        self.persist_samples();
    }

    pub fn recall_sample(&mut self, index: usize) {
        let range = self.samples[index].range();
        self.select_source(self.samples[index].source);
        self.selected_sample = Some(index);
        self.waveform.reveal(range, self.duration());
        self.set_loop_range(range);
        self.seek(range.0);
    }

    pub fn set_loop_range(&mut self, (start, end): (f64, f64)) {
        if self.studio.loop_source != self.active_source {
            self.studio.looping = false;
        }
        self.studio.loop_source = self.active_source;
        self.studio.loop_in = Some(start);
        self.studio.loop_out = Some(end);
        self.sync();
    }

    pub fn audition_range(&mut self, (start, end): (f64, f64)) {
        if self.playing || self.output.is_none() {
            return;
        }
        self.stop_listening();
        self.sync();
        self.seek(start);
        self.preview = true;
        self.send(Command::Audition(start, end));
    }

    pub fn update_sample(&mut self, index: usize) {
        if let Some((start, end)) = self.selection() {
            self.samples[index].source = self.active_source;
            self.samples[index].start = start;
            self.samples[index].end = end;
            self.persist_samples();
        }
    }

    pub fn rename_sample(&mut self, index: usize, name: String) {
        self.samples[index].name = name;
        self.persist_samples();
    }

    pub fn remove_sample(&mut self, index: usize) {
        self.samples.remove(index);
        self.selected_sample = self
            .selected_sample
            .filter(|&selected| selected != index)
            .map(|selected| selected - usize::from(selected > index));
        self.sample_rename = None;
        self.persist_samples();
    }

    pub fn export_sample(&mut self, index: usize) {
        let sample = &self.samples[index];
        let name = sample
            .name
            .replace(|c: char| c.is_control() || "<>:\"/\\|?*".contains(c), "_");
        self.export_source_region(sample.source, sample.range(), &format!("sample-{}-{name}.wav", index + 1));
    }

    pub fn rename_samples(&mut self, indices: &[usize], prefix: &str) {
        for (number, &index) in indices.iter().enumerate() {
            let suffix = format!(" {:02}", number + 1);
            let prefix: String = prefix.chars().take(crate::model::MAX_SAMPLE_NAME - suffix.len()).collect();
            self.samples[index].name = format!("{prefix}{suffix}");
        }
        self.persist_samples();
    }

    pub fn export_samples(&mut self, indices: &[usize]) {
        if self.busy.is_some() || indices.is_empty() {
            return;
        }
        let cuts: Vec<_> = indices.iter().filter_map(|&index| {
            let sample = self.samples.get(index)?;
            let clip = self.source_arc(sample.source)?;
            let name: String = sample.name.chars().map(|c| {
                if c.is_control() || ['<', '>', ':', '"', '/', '\\', '|', '?', '*'].contains(&c) {
                    '_'
                } else {
                    c
                }
            }).take(40).collect();
            Some((clip, sample.range(), format!("cut-{:03}-{name}.wav", index + 1)))
        }).collect();
        if cuts.len() != indices.len() {
            self.status = Some(("One or more selected cuts have a missing source.".into(), true));
            return;
        }
        let Some(folder) = rfd::FileDialog::new().set_title("Choose a folder for the cut pack").pick_folder()
        else { return };
        self.submit_controlled("Exporting selected cuts…", move || {
            let pack = tempfile::Builder::new().prefix("lost-samples-cuts-").tempdir_in(folder)?;
            for (clip, range, name) in &cuts {
                files::export_region(&pack.path().join(name), clip, *range)?;
            }
            crate::background::publish()?;
            let path = pack.keep();
            Ok(Job::Wrote(format!("Exported {} cuts to {}", cuts.len(), path.display())))
        });
    }

    fn persist_samples(&mut self) {
        if self.samples_in_project {
            return;
        }
        if let Some(record) = &self.record {
            let source = record.source.clone();
            self.library
                .sample_banks
                .retain(|bank| bank.source != source);
            if !self.samples.is_empty() {
                self.library.sample_banks.push(SampleBank {
                    source,
                    samples: self.samples.clone(),
                });
            }
            self.persist_library();
        }
    }

    pub fn audition_tempo(&mut self, bpm: u16) {
        if self.playing || self.output.is_none() { return; }
        let Some(clip) = self.source_arc(self.active_source) else { return };
        let range = self.selection().filter(|(start, end)| end - start >= 6.0)
            .unwrap_or((0.0, clip.duration()));
        let beat = 60.0 / f64::from(bpm);
        let center = (range.0 + beat * 0.5).min(range.1);
        let anchor = crate::waveform::transient_near(&clip, center, beat * 0.5);
        let end = range.1.min(range.0 + 8.0);
        self.stop_all();
        self.send(Command::Listen(clip, (range.0, end), 0.8, Some((beat, anchor))));
    }

    pub fn estimate_tempo(&mut self) {
        let source = self.active_source;
        let Some(clip) = self.source_arc(source) else {
            return;
        };
        let selection = self.selection();
        self.submit_controlled("Estimating tempo…", move || {
            let bpm = crate::tempo::estimate(&clip, selection)
                .ok_or("No steady tempo found. Try a section with a clear beat, or use Tap.")?;
            Ok(Job::Tempo(source, bpm))
        });
    }

    pub fn pick_folder(&mut self) {
        if self.busy.is_none()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            self.submit_controlled("Scanning folder…", move || {
                let mut records = Vec::new();
                for entry in std::fs::read_dir(path)? {
                    crate::background::check()?;
                    let entry = entry?;
                    let path = entry.path();
                    if entry.file_type()?.is_file()
                        && path
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|ext| {
                                files::AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                            })
                    {
                        records.push(files::local_record(path));
                    }
                }
                if records.is_empty() {
                    return Err("The folder has no supported audio files".into());
                }
                records.sort_by(|a, b| a.title.cmp(&b.title));
                Ok(Job::Folder(records))
            });
        }
    }

    fn project_snapshot(&self) -> Project {
        Project {
            record: self.record.clone(),
            studio: self.studio.clone(),
            samples: self.samples.clone(),
            source_ids: self.source_ids.clone(),
            snapshots: self.snapshots.clone(),
        }
    }

    fn sync_source_ids(&mut self) {
        use crate::model::snapshot::SourceId;
        let count = self.clip.as_ref().map_or(0, |_| self.studio.sources.len() + 1);
        self.source_ids.truncate(count);
        while self.source_ids.len() < count {
            let id = SourceId(self.next_source_id);
            self.next_source_id = self.next_source_id.wrapping_add(1).max(1);
            if !self.source_ids.contains(&id) {
                self.source_ids.push(id);
            }
        }
    }

    pub fn save_named_snapshot(&mut self, name: &str) -> files::Result<()> {
        use crate::model::snapshot::{MAX_SNAPSHOT_BYTES, MAX_SNAPSHOTS, NamedSnapshot};
        let name: String = name.trim().chars().take(crate::model::MAX_SAMPLE_NAME).collect();
        if self.recording || name.is_empty() || self.snapshots.iter().any(|item| item.name.eq_ignore_ascii_case(&name)) {
            return Err("Use a unique snapshot name with recording stopped.".into());
        }
        if self.snapshots.len() >= MAX_SNAPSHOTS {
            return Err("Delete a snapshot to make room for another.".into());
        }
        self.observe_edits(false);
        let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
        let previous_ids = self.source_ids.clone();
        self.sync_source_ids();
        let mut project = self.project_snapshot();
        Arc::make_mut(&mut project.snapshots).push(NamedSnapshot::new(
            name, timestamp, self.source_ids.clone(), self.studio.clone(), self.samples.clone(),
        ));
        let sizes = serde_json::to_vec(&project.snapshots).and_then(|snapshots| {
            serde_json::to_vec(&project).map(|metadata| (snapshots.len(), metadata.len()))
        });
        let fits = sizes.is_ok_and(|(snapshots, metadata)| {
            snapshots <= MAX_SNAPSHOT_BYTES && metadata <= files::MAX_PROJECT_METADATA
        });
        if !fits {
            self.source_ids = previous_ids;
            return Err("This snapshot would exceed the project metadata limit.".into());
        }
        self.snapshots = project.snapshots;
        self.observe_edits(false);
        Ok(())
    }

    pub fn delete_named_snapshot(&mut self, index: usize) {
        if index < self.snapshots.len() && !self.recording {
            self.observe_edits(false);
            Arc::make_mut(&mut self.snapshots).remove(index);
            self.observe_edits(false);
        }
    }

    fn resolve_snapshot(&self, index: usize) -> Option<(Studio, Vec<NamedSample>)> {
        let (mut studio, mut samples) = self.snapshots.get(index)?
            .resolve(&self.studio.sources, &self.source_ids)?;
        studio.validate_media(self.clip.as_deref());
        studio.validate_source_analysis(self.clip.as_deref());
        studio.validate_song();
        NamedSample::validate_sources(&mut samples, |source| {
            self.clip.as_deref()?.source(source).map(Clip::duration)
        });
        Some((studio, samples))
    }

    pub fn restore_named_snapshot(&mut self, index: usize) -> bool {
        if self.playing || self.recording {
            return false;
        }
        let Some((mut studio, samples)) = self.resolve_snapshot(index) else { return false };
        self.observe_edits(false);
        self.stop_all();
        studio.event_id_next = studio.event_id_next.max(self.studio.event_id_next);
        studio.identify_saved_events();
        self.studio = studio;
        self.samples = samples;
        self.samples_in_project = true;
        self.reset_pattern_editor();
        self.arrangement_timeline.sections_changed();
        self.select_source(self.studio.loop_source);
        self.observe_edits(false);
        self.status = Some(("Snapshot restored.".into(), false));
        true
    }

    pub fn prepare_snapshot_preview(
        &mut self, index: usize, song: bool, start: usize, match_level: bool,
    ) -> files::Result<()> {
        if self.busy.is_some() || self.input_recording.is_some() || self.recording || self.output.is_none() {
            return Err("Finish the current operation before preparing a comparison.".into());
        }
        self.sync();
        let (saved, _) = self.resolve_snapshot(index).ok_or("The snapshot's sources are unavailable.")?;
        let current = self.studio.clone();
        let sequence = |studio: &Studio| {
            if song { studio.song_sequence() } else { Some(studio.active_sequence()) }
        };
        let a = sequence(&current).ok_or("The working project has no playable song.")?;
        let b = sequence(&saved).ok_or("The snapshot has no playable song.")?;
        let end = a.bars.len().min(b.bars.len()).min(start.saturating_add(4));
        if start >= end {
            return Err("Choose a starting bar present in both versions.".into());
        }
        self.clear_snapshot_preview();
        let request = self.random();
        self.snapshot_preview_request = Some(request);
        let clip = self.clip.clone();
        self.submit_controlled(SNAPSHOT_PREVIEW_JOB, move || {
            let render = |studio: Studio, sequence| {
                files::bounce_window(studio, clip.clone(), sequence, Some(start..end), files::MAX_PROJECT_FRAMES)
            };
            let a = Arc::new(render(current.clone(), a.clone())?);
            let b = Arc::new(render(saved.clone(), b.clone())?);
            let gains = if match_level {
                let rms = |clip: &Clip| {
                    let sum = clip.frames.iter().map(|frame| {
                        f64::from(frame[0]).powi(2) + f64::from(frame[1]).powi(2)
                    }).sum::<f64>();
                    (sum / (clip.frames.len().max(1) as f64 * 2.0)).sqrt() as f32
                };
                let levels = [rms(&a), rms(&b)];
                let target = levels[0].min(levels[1]);
                levels.map(|level| if target > 1e-5 { target / level } else { 1.0 })
            } else {
                [1.0; 2]
            };
            Ok(Job::SnapshotPreview(request, [a, b], gains))
        });
        Ok(())
    }

    pub fn snapshot_preview_ready(&self) -> bool {
        self.snapshot_preview.is_some()
    }

    pub fn play_snapshot_preview(&mut self, side: usize) {
        if self.recording {
            return;
        }
        let Some((clips, gains)) = &self.snapshot_preview else { return };
        let clip = clips[side].clone();
        let gain = gains[side];
        let end = clip.duration();
        self.stop_all();
        self.send(Command::Listen(clip, (0.0, end), gain, None));
    }

    pub fn clear_snapshot_preview(&mut self) {
        if self.snapshot_preview.take().is_some() {
            self.send(Command::StopListening);
        }
        if self.snapshot_preview_request.take().is_some()
            && self.busy.as_ref().is_some_and(|(name, _)| *name == SNAPSHOT_PREVIEW_JOB)
            && let Some(job) = &self.job_control
        {
            job.cancel();
        }
        if self.retry_job.as_ref().is_some_and(|(name, _)| *name == SNAPSHOT_PREVIEW_JOB) {
            self.retry_job = None;
        }
    }

    pub(crate) fn observe_edits(&mut self, group: bool) {
        self.sync();
        let current = &self.session.current;
        if current.studio != self.studio || current.samples != self.samples || current.record != self.record
            || current.source_ids != self.source_ids || !Arc::ptr_eq(&current.snapshots, &self.snapshots)
        {
            self.clear_snapshot_preview();
            self.analysis_invalidate();
            self.session.observe(self.project_snapshot(), group);
        } else if !group {
            self.session.end_group();
        }
    }

    pub fn project_name(&self) -> String {
        self.session.path.as_ref().and_then(|path| path.file_stem())
            .map_or_else(|| "Untitled project".into(), |name| name.to_string_lossy().into_owned())
    }

    pub fn project_dirty(&self) -> bool {
        self.session.dirty()
    }

    pub fn can_recover(&self) -> bool {
        self.session.available_recovery.is_some()
    }

    pub fn show_recovery(&mut self) {
        self.session.show_recovery = self.can_recover();
    }

    fn remember_project(&mut self, path: PathBuf) {
        self.library.recent_projects.retain(|recent| *recent != path);
        self.library.recent_projects.insert(0, path);
        self.library.recent_projects.truncate(12);
        self.persist_library();
    }

    pub fn open(&mut self, path: PathBuf) {
        self.open_project(path, OpenKind::Project);
    }

    fn open_project(&mut self, path: PathBuf, kind: OpenKind) {
        self.submit_controlled("Opening project…", move || {
            let (project, clip) = match kind {
                OpenKind::Recovery => files::recovery::open(&path)?,
                _ => files::open_project(&path)?,
            };
            Ok(Job::Opened(Box::new(project), clip, path, kind))
        });
    }

    pub fn open_dialog(&mut self) {
        if self.busy.is_none()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Lost Samples project", &["flip"])
                .pick_file()
        {
            self.open(path);
        }
    }

    fn template_folder(&mut self) -> Option<PathBuf> {
        let directory = files::app_data_dir()?.join("templates");
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.status = Some((format!("Could not open the template folder: {error}"), true));
            return None;
        }
        Some(directory)
    }

    pub fn new_from_template(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(folder) = self.template_folder() else { return };
        if let Some(path) = rfd::FileDialog::new().set_title("New project from template")
            .set_directory(folder).add_filter("Lost Samples project", &["flip"]).pick_file()
        {
            self.open_project(path, OpenKind::Template);
        }
    }

    pub fn save_template(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(folder) = self.template_folder() else { return };
        let Some(path) = rfd::FileDialog::new().set_title("Save a reusable project template")
            .set_directory(folder).set_file_name("template.flip")
            .add_filter("Lost Samples project", &["flip"]).save_file()
        else { return };
        if self.session.path.as_ref() == Some(&path) {
            self.status = Some(("Choose a different file for the template.".into(), true));
            return;
        }
        self.read_recorded_chops();
        self.observe_edits(self.recording);
        let snapshot = self.session.current.clone();
        let clip = self.clip.clone();
        self.submit_controlled("Saving project template…", move || {
            files::publish_export(&path, |output| files::save_project(output, &snapshot, clip.as_deref()))?;
            Ok(Job::Wrote(format!("Saved template {}", path.display())))
        });
    }

    pub fn save_dialog(&mut self) {
        self.save_project(false);
    }

    pub fn save_project(&mut self, save_as: bool) {
        if self.busy.is_some() {
            return;
        }
        let path = if let Some(path) = self.session.path.clone().filter(|_| !save_as) {
            path
        } else {
            let name = format!("{}.flip", self.project_name());
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Lost Samples project", &["flip"])
                .set_file_name(name)
                .save_file()
            else {
                return;
            };
            path
        };
        self.read_recorded_chops();
        self.observe_edits(self.recording);
        let snapshot = self.session.current.clone();
        let clip = self.clip.clone();
        self.submit_controlled("Saving project…", move || {
            files::publish_export(&path, |output| files::save_project(output, &snapshot, clip.as_deref()))?;
            Ok(Job::Saved(path, snapshot))
        });
    }

    pub fn can_undo_project(&self, redo: bool) -> bool {
        !self.recording && self.pending_change.is_none() && self.session.can_travel(redo)
    }

    pub fn undo_project(&mut self, redo: bool) {
        if !self.can_undo_project(redo) {
            return;
        }
        let Some(snapshot) = self.session.travel(redo) else { return };
        self.analysis_invalidate();
        self.recording_project += 1;
        self.recorded_gates.clear();
        self.stop_song();
        self.studio = snapshot.studio.clone();
        self.source_ids.clone_from(&snapshot.source_ids);
        self.snapshots = snapshot.snapshots.clone();
        self.clear_snapshot_preview();
        if self.record.is_some() && snapshot.record.is_none() {
            self.stop_all();
            self.detached_clip = self.clip.take();
            self.send(Command::Clip(None));
            self.position = 0.0;
        } else if self.record.is_none() && snapshot.record.is_some() {
            self.clip = self.detached_clip.take();
            self.send(Command::Clip(self.clip.clone()));
        }
        self.record = snapshot.record.clone();
        if self.active_source > self.studio.sources.len() {
            self.select_source(0);
        }
        if let Some(roll) = self.piano_roll.as_mut() {
            roll.reset_selection();
        }
        let samples_changed = self.samples != snapshot.samples;
        self.samples = snapshot.samples.clone();
        self.selected_sample = None;
        self.sample_rename = None;
        self.bar = self.bar.min(self.studio.bars - 1);
        self.arrangement_timeline.sections_changed();
        if samples_changed {
            self.persist_samples();
        }
        self.sync();
    }

    fn continue_change(&mut self, discard: bool) {
        if matches!(self.pending_change, Some(PendingChange::Quit))
            && (self.analysis_job().is_some() || self.extraction_job().is_some())
        {
            self.discard_after_analysis |= discard;
            return;
        }
        if let Some(change) = self.pending_change.take() {
            if discard {
                self.session.discard();
            }
            match change {
                PendingChange::Replace(job) => self.apply_job(*job),
                PendingChange::Quit => self.finish_close(),
            }
        }
    }

    fn finish_close(&mut self) {
        self.finishing_close = true;
        self.session.clear_recovery();
        self.ctx.request_repaint();
    }

    fn project_dialogs(&mut self, ctx: &egui::Context) {
        if self.finishing_close {
            crate::ui::responsive_modal(ctx, "flush_local_data", 465.0, |ui| {
                if let Some(error) = self.crate_save_error().filter(|_| !self.library_persistence.pending()) {
                    ui.heading("Could not save crate changes");
                    ui.label(error);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Retry save").clicked() {
                            self.retry_crate_save();
                        }
                        if ui.button("Close without saving crate").clicked() {
                            self.library_persistence.error = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.finishing_close = false;
                        }
                    });
                } else {
                    ui.spinner();
                    ui.label("Finishing local saves…");
                }
            });
        } else if self.pending_change.is_some() {
            let mut cancel = false;
            let modal = crate::ui::responsive_modal(ctx, "unsaved_project", 465.0, |ui| {
                if self.discard_after_analysis {
                    ui.spinner();
                    ui.label("Stopping local analysis…");
                    return;
                }
                ui.heading("Save your changes?");
                ui.label(format!("{} has unsaved edits.", self.project_name()));
                if let Some((job, _)) = &self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(*job);
                        crate::ui::job_controls(self, ui);
                    });
                } else if let Some((message, _)) = &self.status {
                    ui.label(message);
                    if self.can_retry_job() && ui.button("Retry last operation").clicked() {
                        self.retry_background_job();
                    }
                }
                ui.add_enabled_ui(self.busy.is_none(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Save").clicked() {
                            self.save_dialog();
                        }
                        if ui.button("Discard changes").clicked() {
                            self.continue_change(true);
                        }
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            });
            if self.busy.is_none() && (cancel || modal.should_close()) {
                self.pending_change = None;
                self.discard_after_analysis = false;
            }
        } else if self.session.show_recovery {
            let mut dismiss = false;
            let modal = crate::ui::responsive_modal(ctx, "recover_project", 465.0, |ui| {
                ui.heading("Recover your previous session?");
                ui.label("A saved recovery snapshot is available.");
                ui.horizontal(|ui| {
                    if ui.button("Recover session").clicked()
                        && let Some(path) = self.session.available_recovery.clone()
                    {
                        self.open_project(path, OpenKind::Recovery);
                        dismiss = true;
                    }
                    dismiss |= ui.button("Not now").clicked();
                });
            });
            self.session.show_recovery = !(dismiss || modal.should_close());
        }
    }

    pub fn export(&mut self, extension: &str) {
        self.export_configured(extension, false, files::WavOptions::default());
    }

    pub fn export_configured(&mut self, extension: &str, pattern_only: bool, options: files::WavOptions) {
        self.export_configured_mode(extension, pattern_only, options, Default::default());
    }

    pub fn export_configured_mode(&mut self, extension: &str, pattern_only: bool, options: files::WavOptions, mode: crate::transcription::midi::Mode) {
        if self.busy.is_some() {
            return;
        }
        let filename = match extension {
            "wav" if !pattern_only && !self.studio.sections.is_empty() => "song.wav",
            "mid" if !pattern_only && !self.studio.sections.is_empty() => "song.mid",
            "wav" => "flip.wav",
            "mid" => "flip.mid",
            "csv" => "crate.csv",
            _ => "crate.json",
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(extension.to_ascii_uppercase(), &[extension])
            .set_file_name(filename)
            .save_file()
        else {
            return;
        };
        let name = format!("Exported {}", path.display());
        self.read_recorded_chops();
        self.sync();
        match extension {
            "wav" | "mid" => {
                let mut studio = self.studio.clone();
                if pattern_only {
                    studio.sections.clear();
                }
                let clip = self.clip.clone();
                let wav = extension == "wav";
                let task = if studio.sections.is_empty() {
                    "Exporting pattern…"
                } else {
                    "Exporting song…"
                };
                self.submit_controlled(task, move || {
                    if wav {
                        files::publish_export(&path, |output| files::export_wav(output, studio, clip, options))?;
                    } else {
                        files::publish_export(&path, |output| files::export_midi_mode(output, &studio, mode))?;
                    }
                    Ok(Job::Wrote(name))
                });
            }
            _ => {
                let csv = extension == "csv";
                let library = Library {
                    audio: Default::default(),
                    interface: Default::default(),
                    recent_projects: Vec::new(),
                    dig_presets: Vec::new(),
                    records: self
                        .library
                        .records
                        .iter()
                        .filter(|entry| {
                            entry.in_collection(self.collection.as_deref())
                        })
                        .cloned()
                        .collect(),
                    collections: self.library.collections.clone(),
                    heard: self.library.heard,
                    queue: Vec::new(),
                    heard_sources: Vec::new(),
                    daw_targets: Vec::new(),
                    midi: midi::Settings::default(),
                    beat_python: None,
                    search_python: None,
                    pitch_python: None,
                    sam_python: None,
                    sam_ffmpeg: None,
                    taste: Default::default(),
                    sample_banks: self
                        .library
                        .sample_banks
                        .iter()
                        .filter(|bank| {
                            self.collection.is_none()
                                || self.library.records.iter().any(|entry| {
                                    entry.in_collection(self.collection.as_deref())
                                        && entry.record.source == bank.source
                                })
                        })
                        .cloned()
                        .collect(),
                };
                let library = Arc::new(library);
                self.submit_controlled("Exporting crate…", move || {
                    if csv {
                        files::publish_export(&path, |output| files::export_csv(output, &library))?;
                    } else {
                        files::publish_export(&path, |output| files::export_json(output, &library))?;
                    }
                    Ok(Job::Wrote(name))
                });
            }
        }
    }

    pub fn is_saved(&self) -> bool {
        self.record.as_ref().is_some_and(|record| {
            self.library
                .records
                .iter()
                .any(|entry| entry.record.source == record.source)
        })
    }

    pub fn save_record(&mut self) {
        self.read_recorded_chops();
        self.sync();
        if !self.studio.sources.is_empty()
            || self.record.as_ref().is_some_and(|record| matches!(record.source, Source::Generated(_) | Source::Extracted(_)))
        {
            self.status = Some(("Save the project to keep its embedded sources.".into(), false));
            return;
        }
        if let Some(record) = &self.record {
            let saved = self
                .library
                .records
                .iter()
                .find(|entry| entry.record.source == record.source);
            let mut entry = saved.cloned().unwrap_or_else(|| SavedRecord {
                record: record.clone(),
                studio: self.studio.clone(),
                collection: None,
                other_collections: Vec::new(),
                tags: String::new(),
                notes: String::new(),
                favorite: false,
            });
            entry.studio = self.studio.clone();
            if let Some(collection) = self.collection.as_deref() && !entry.in_collection(Some(collection)) {
                entry.toggle_collection(collection);
            }
            self.library
                .records
                .retain(|saved| saved.record.source != entry.record.source);
            self.library.records.insert(0, entry);
            self.status = Some(("Saved record and flip settings to the crate.".into(), false));
            self.persist_library();
        }
    }

    pub fn relink_crate_audio(&mut self, source: Source) {
        if self.busy.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new().set_title("Find the missing recording")
            .add_filter("Audio files", files::AUDIO_EXTENSIONS).pick_file()
        else { return };
        self.submit_controlled("Checking replacement audio…", move || {
            files::decode_path(&path)?;
            Ok(Job::Relinked(source, Source::File(path)))
        });
    }

    pub fn persist_library(&mut self) {
        if self.library_recovery.is_some() {
            self.status = Some(("Crate changes are session-only until recovery.".into(), true));
            return;
        }
        self.library_persistence.changed();
    }

    pub fn crate_save_error(&self) -> Option<&str> {
        self.library_persistence.error.as_deref()
    }

    pub fn retry_crate_save(&mut self) {
        self.library_persistence.retry();
    }

    pub fn restoring_library(&self) -> bool {
        self.library_persistence.restoring()
    }

    pub fn restore_library(&mut self) {
        let Some(recovery) = &self.library_recovery else {
            return;
        };
        if let Err(error) = self.library_persistence.restore(recovery.clone()) {
            self.status = Some((format!("Could not restore the crate backup: {error}"), true));
        }
    }

    fn poll_library_persistence(&mut self) {
        let Some(result) = self.library_persistence.poll(
            &self.ctx, &self.library, self.finishing_close,
        ) else {
            return;
        };
        self.ctx.request_repaint();
        match result {
            Ok(preserved) => {
                self.library_recovery = None;
                let message = preserved.map_or_else(
                    || "Crate backup restored.".into(),
                    |path| {
                        format!("Crate backup restored. Damaged file kept at {}.", path.display())
                    },
                );
                self.status = Some((message, false));
            }
            Err(error) => {
                self.status = Some((format!("Could not restore the crate backup: {error}"), true));
            }
        }
    }

    pub fn seek(&mut self, seconds: f64) {
        self.position = seconds.clamp(0.0, self.duration());
        self.seeked_at = Some(Instant::now());
        self.send(Command::Seek(self.position));
    }

    pub fn jump(&mut self) {
        let fraction = (self.random() >> 11) as f64 / ((1_u64 << 53) as f64);
        self.seek(self.duration() * (0.10 + fraction * 0.78));
    }

    pub fn toggle_preview(&mut self) {
        if self.clip.is_some() && self.output.is_some() {
            self.stop_listening();
            self.preview = !self.preview;
            self.sync();
            self.send(Command::Play(self.preview));
        }
    }

    pub fn can_preview_drum_template(&self) -> bool {
        self.busy.is_none() && self.output.is_some() && !self.playing
            && !self.recording && self.input_recording.is_none()
    }

    pub fn drum_preview_pending(&self) -> bool {
        self.busy.as_ref().is_some_and(|(name, _)| *name == DRUM_PREVIEW_JOB)
    }

    pub fn stop_drum_preview(&mut self) {
        if self.drum_preview_pending() && let Some(job) = &self.job_control {
            job.cancel();
        }
        self.send(Command::StopListening);
    }

    pub fn preview_drum_template(&mut self, index: usize, use_groove: bool) {
        if !self.can_preview_drum_template() {
            return;
        }
        let template = &crate::model::drum_templates::TEMPLATES[index];
        let mut studio = self.studio.clone();
        studio.looping = false;
        if use_groove {
            studio.bpm = template.bpm;
            studio.swing = template.swing;
        }
        let mut bars: Vec<_> = template.bars.iter().map(|bar| bar.steps()).collect();
        if bars.len() == 1 {
            bars.push(bars[0].clone());
        }
        let sequence = crate::model::Sequence {
            bars,
            ..Default::default()
        };
        let clip = self.clip.clone();
        self.submit_controlled(DRUM_PREVIEW_JOB, move || {
            let preview = files::bounce_mix(studio, clip, sequence, files::MAX_PROJECT_FRAMES)?;
            Ok(Job::DrumTemplatePreview(index, use_groove, preview))
        });
    }

    pub fn toggle_pattern(&mut self) {
        if self.song_playing.is_some() {
            self.stop_song();
            self.set_playing(true);
        } else {
            self.set_playing(!self.playing);
        }
    }

    pub fn stop_all(&mut self) {
        self.stop_beat_audition();
        self.stop_recording();
        self.stop_listening();
        self.playing = false;
        self.song_playing = None;
        self.scene_active = None;
        self.scene_pending = None;
        self.preview = false;
        self.step = None;
        self.midi_panic();
        self.send(Command::StopAll);
    }

    fn set_playing(&mut self, playing: bool) {
        if playing && self.drum_templates.is_some() {
            self.stop_drum_preview();
        }
        if !playing {
            self.stop_beat_audition();
            self.stop_recording();
        }
        self.scene_active = None;
        self.scene_pending = None;
        if self.output.is_some() {
            self.playing = playing;
            self.song_playing = None;
            self.sync();
            if playing { self.queue_beat_audition(); }
            self.send(Command::Pattern(playing));
        }
    }

    pub fn can_launch_scene(&self) -> bool {
        self.output.is_some() && self.song_playing.is_none() && !self.recording
            && self.midi_clock_connection.is_none()
    }

    pub fn launch_scene(&mut self, index: usize) {
        if !self.can_launch_scene() { return; }
        self.sync();
        let Some(pattern) = self.studio.patterns.get(index) else { return };
        let mut sequence = pattern.notes.clone();
        sequence.bars.resize(pattern.length, Default::default());
        sequence.performance.retain(|note| note.tick < pattern.length as u32 * crate::model::TICKS_PER_BAR);
        if !std::mem::replace(&mut self.playing, true) { self.queue_beat_audition(); }
        self.scene_pending = Some(index);
        self.send(Command::LaunchScene(index, Arc::new(sequence), self.scene_quantize));
    }

    fn capture_scene_bar(&mut self, pattern: usize, offset: usize) {
        if !self.scene_capture { return; }
        let Some(scene) = self.studio.patterns.get(pattern) else { return };
        let available = crate::model::MAX_SONG_BARS.saturating_sub(self.studio.song_bars());
        if self.scene_take.iter().map(|run| run.bars).sum::<usize>() >= available { return; }
        if let Some(run) = self.scene_take.last_mut()
            && run.pattern == pattern && (run.offset + run.bars) % scene.length == offset
        {
            run.bars += 1;
        } else if self.scene_take.len() + self.studio.sections.len() < crate::model::MAX_SECTIONS {
            self.scene_take.push(SceneRun { pattern, offset, bars: 1 });
        }
    }

    pub fn append_scene_take(&mut self) {
        self.read_recorded_chops();
        self.scene_capture = false;
        let mut remaining = crate::model::MAX_SONG_BARS.saturating_sub(self.studio.song_bars());
        let space = crate::model::MAX_SECTIONS.saturating_sub(self.studio.sections.len());
        for run in self.scene_take.drain(..).take(space) {
            let bars = run.bars.min(remaining);
            if bars == 0 { break; }
            if self.studio.patterns.get(run.pattern).is_none() { continue; }
            let mut section = crate::model::Section::new(crate::model::SectionKind::Verse, run.pattern, 1);
            section.span = Some(crate::model::SectionSpan { offset: run.offset, bars });
            self.studio.sections.push(section);
            remaining -= bars;
        }
    }

    pub fn update_playing_automation(&self) {
        if let Some(id) = self.song_playing && let Some(sequence) = self.studio.song_sequence() {
            self.send(Command::SongAutomation(id, Arc::new(sequence)));
        }
    }

    pub fn play_song(&mut self) {
        self.start_song(None);
    }

    pub fn can_play_section(&self) -> bool {
        self.output.is_some() && !self.recording && self.midi_clock_connection.is_none()
    }

    pub fn play_section(&mut self, index: usize, repeat: bool) {
        if !self.can_play_section() {
            return;
        }
        let Some(range) = self.studio.section_range(index) else { return };
        let start = (range.start / crate::model::TICKS_PER_BAR) as usize;
        let end = if repeat {
            (range.end / crate::model::TICKS_PER_BAR) as usize
        } else {
            self.studio.song_bars()
        };
        self.start_song(Some((start..end, repeat)));
    }

    fn start_song(&mut self, window: Option<(std::ops::Range<usize>, bool)>) {
        if self.output.is_none() || self.recording {
            return;
        }
        self.sync();
        let Some(sequence) = self.studio.song_sequence() else {
            return;
        };
        self.stop_listening();
        self.scene_pending = None;
        self.scene_active = None;
        self.song_request += 1;
        self.song_playing = Some(self.song_request);
        if !std::mem::replace(&mut self.playing, true) { self.queue_beat_audition(); }
        self.step = None;
        let sequence = Arc::new(sequence);
        let command = match window {
            Some((range, repeat)) => Command::SongWindow(sequence, self.song_request, range, repeat),
            None => Command::Song(sequence, self.song_request),
        };
        self.send(command);
    }

    pub fn stop_song(&mut self) {
        if self.song_playing.is_some() {
            self.set_playing(false);
            self.step = None;
        }
    }

    pub fn open_commands(&mut self) {
        self.command_query.clear();
        self.focus_commands = true;
        self.show_commands = true;
    }

    pub fn open_arrangement(&mut self) {
        if self.pattern_name.trim().is_empty() {
            self.pattern_name = self.studio.next_pattern_name();
        }
        self.arrangement_timeline.cancel_drag();
        self.studio_view = StudioView::Arrange;
    }

    pub fn create_pattern(&mut self, blank: bool) {
        self.read_recorded_chops();
        let name = self.pattern_name.trim().to_owned();
        let save_draft = blank && self.studio.has_draft();
        if self.recording
            || name.is_empty()
            || self.studio.patterns.len() + usize::from(save_draft) >= MAX_PATTERNS
            || self
                .studio
                .patterns
                .iter()
                .any(|pattern| pattern.name.eq_ignore_ascii_case(&name))
        {
            return;
        }
        self.set_playing(false);
        self.read_recorded_chops();
        self.sync();
        if save_draft {
            let draft = self.studio.next_pattern_name_except(&name);
            self.studio.save_pattern(&draft, false);
        }
        self.studio.save_pattern(&name, blank);
        self.pattern_name = self.studio.next_pattern_name();
        self.reset_pattern_editor();
    }

    pub fn preview_performance(&mut self, bars: usize) {
        self.sync();
        self.capture_quantize = Quantize::Off;
        if let Some(output) = &self.output {
            let request = output.performance.invalidate_preview();
            output.send(Command::PreviewPerformance(bars, request));
        }
    }

    pub fn keep_performance(&mut self) {
        if self.recording || !self.sent.same_capture_instruments(&self.studio) {
            return;
        }
        let save_draft = self.studio.has_draft();
        if self.studio.patterns.len() + usize::from(save_draft) >= MAX_PATTERNS {
            self.status = Some(("Remove an unused pattern to make room for the capture.".into(), true));
            return;
        }
        if let Some(output) = &mut self.output {
            output.performance.poll();
            output.retire_settings();
        }
        let Some(preview) = self.output.as_ref().and_then(|output| output.performance.preview())
        else {
            return;
        };
        let notes = preview.notes(self.capture_quantize, self.studio.swing);
        let bars = preview.bars;
        self.observe_edits(false);
        let mut next = self.project_snapshot();
        let Some(name) = next.studio.save_captured_pattern(bars, notes) else { return };
        if !serde_json::to_vec(&next).is_ok_and(|bytes| bytes.len() <= files::MAX_PROJECT_METADATA) {
            self.status = Some(("Choose a shorter capture to stay within the project size limit.".into(), true));
            return;
        }
        if let Some(output) = &self.output {
            output.performance.invalidate_preview();
        }
        self.studio = next.studio;
        self.set_playing(false);
        self.midi_panic();
        self.pattern_name = self.studio.next_pattern_name();
        self.reset_pattern_editor();
        self.studio_view = StudioView::Sequence;
        self.observe_edits(false);
        self.status = Some((format!("Saved {name} as a new pattern."), false));
    }

    pub fn audition_pattern(&mut self, sequence: crate::model::Sequence) -> Option<u64> {
        if !self.can_play_section() {
            return None;
        }
        self.stop_all();
        self.sync();
        self.song_request += 1;
        let id = self.song_request;
        let bars = sequence.bars.len();
        self.song_playing = Some(id);
        self.playing = true;
        self.send(Command::SongWindow(Arc::new(sequence), id, 0..bars, true));
        Some(id)
    }

    pub fn keep_variation(&mut self, name: &str, sequence: crate::model::Sequence) -> bool {
        self.observe_edits(false);
        let mut next = self.project_snapshot();
        let Some(name) = next.studio.save_generated_pattern(name, sequence) else {
            return false;
        };
        if !serde_json::to_vec(&next).is_ok_and(|bytes| bytes.len() <= files::MAX_PROJECT_METADATA) {
            return false;
        }
        self.studio = next.studio;
        self.stop_all();
        self.reset_pattern_editor();
        self.observe_edits(false);
        self.status = Some((format!("Saved {name} as a new pattern."), false));
        true
    }

    pub fn edit_pattern(&mut self, index: usize) {
        if self.recording
            || index >= self.studio.patterns.len()
            || self.studio.editing_pattern == Some(index)
        {
            return;
        }
        self.set_playing(false);
        self.read_recorded_chops();
        if self.studio.has_draft() {
            if self.studio.patterns.len() == MAX_PATTERNS {
                self.status = Some((
                    "Remove an unused pattern before switching so this draft can be saved.".into(),
                    true,
                ));
                return;
            }
            self.studio
                .save_pattern(&self.studio.next_pattern_name(), false);
        }
        self.studio.load_pattern(index);
        self.reset_pattern_editor();
    }

    fn reset_pattern_editor(&mut self) {
        self.recording_project += 1;
        self.recorded_gates.clear();
        self.piano_roll = None;
        self.bar = 0;
        self.step = None;
        self.sync();
    }

    pub fn toggle_recording(&mut self) {
        if self.recording {
            self.stop_recording();
        } else if self.clip.is_some() && self.output.is_some()
            && self.scene_active.is_none() && self.scene_pending.is_none() {
            self.stop_song();
            self.read_recorded_chops();
            self.piano_roll = None;
            self.recording = true;
            self.stop_listening();
            let first = self.punch_first.clamp(1, self.studio.bars) - 1;
            let last = self.punch_last.clamp(first + 1, self.studio.bars);
            let range = self.punch_in.then_some(
                first as u32 * crate::model::TICKS_PER_BAR..last as u32 * crate::model::TICKS_PER_BAR,
            );
            if self.replace_take {
                self.studio.performance.retain(|note| {
                    !range.as_ref().map_or(note.tick < self.studio.bars as u32 * crate::model::TICKS_PER_BAR,
                        |range| range.contains(&note.tick))
                });
            }
            self.sync();
            let start = !self.playing && self.midi_clock_connection.is_none();
            if start { self.set_playing(true); }
            self.send(Command::RecordRange(range));
            self.send(Command::Record(Some((self.quantize, self.recording_project))));
            if start && self.count_in_bars > 0 { self.send(Command::CountIn(self.count_in_bars)); }
        }
    }

    fn stop_recording(&mut self) {
        if self.recording {
            self.recording = false;
            self.send(Command::Record(None));
        }
    }

    fn read_recorded_chops(&mut self) {
        while let Some(capture) = self.output.as_ref().and_then(Output::recorded) {
            let (project, id, event, start) = match capture {
                CapturedChop::Start(project, id, event) => (project, id, event, true),
                CapturedChop::End(project, id, event) => (project, id, event, false),
                CapturedChop::SceneBar { pattern, offset } => {
                    self.capture_scene_bar(pattern, offset);
                    continue;
                }
            };
            if project != self.recording_project {
                continue;
            }
            let key = (event.tick, event.input);
            let index = self
                .studio
                .performance
                .binary_search_by_key(&key, |event| (event.tick, event.input));
            if start {
                self.recorded_gates.remove(&key);
                if event.mode == PadMode::Gate {
                    self.recorded_gates.insert(key, id);
                }
                match index {
                    Ok(index) => self.studio.performance[index] = event,
                    Err(index) => self.studio.performance.insert(index, event),
                }
            } else if self.recorded_gates.get(&key) == Some(&id) {
                self.recorded_gates.remove(&key);
                if let Ok(index) = index {
                    self.studio.performance[index].length = event.length;
                }
            }
        }
    }

    pub fn toggle_loop(&mut self) {
        if let Some((start, end)) = self.selection() {
            self.studio.looping = !self.studio.looping;
            self.sync();
            if self.studio.looping && !(start..end).contains(&self.position) {
                self.seek(start);
            }
        }
    }

    pub fn pad(&mut self, index: usize, assign: bool) {
        self.play_pad(index, assign, 127);
    }

    fn play_pad(&mut self, index: usize, assign: bool, velocity: u8) {
        self.selected_pad = index;
        if self.clip.is_none() {
            return;
        }
        if assign || self.studio.pads[index].is_none() {
            self.studio.pads[index] = Some(self.position.min((self.duration() - 0.001).max(0.0)));
            self.studio.pad_ends[index] = None;
            self.studio.pad_sources[index] = self.active_source;
        } else {
            self.sync();
            self.send(Command::Pad(index, self.pad_mode, velocity));
        }
    }

    pub fn clear_pad(&mut self, index: usize) {
        self.studio.pads[index] = None;
        self.studio.pad_ends[index] = None;
        self.studio.pad_sources[index] = 0;
        self.studio.pad_names[index].clear();
        self.studio.pad_colors[index] = 0;
        self.studio.choke_groups.pads[index] = 0;
        self.studio.instruments.pads.remove(&index);
        self.studio
            .performance
            .retain(|event| event.input != ChopInput::Pad(index as u8));
        for cell in self.studio.chops.iter_mut().flatten() {
            if *cell == Some(index) {
                *cell = None;
            }
        }
    }

    pub fn set_piano_view(&mut self, enabled: bool) {
        if self.library.midi.piano_view != enabled {
            self.library.midi.piano_view = enabled;
            self.piano_arm = false;
            self.persist_library();
        }
    }

    pub fn assign_piano(&mut self, range: (f64, f64)) {
        self.assign_piano_source(self.active_source, range);
    }

    pub fn assign_piano_source(&mut self, source: usize, range: (f64, f64)) {
        self.studio.key_sources.insert(self.piano_key, source);
        self.studio
            .key_chops
            .insert(self.piano_key, KeyChop::Cut(range.0, range.1));
        self.piano_arm = false;
    }

    pub fn piano_pressed(&self, note: u8) -> bool {
        self.piano_held.iter().any(|&(_, held)| held == note)
    }

    pub fn piano_down(&mut self, source: u64, note: u8, velocity: u8) {
        if !(PIANO_LOW..=PIANO_HIGH).contains(&note) {
            return;
        }
        self.piano_key = note;
        if self.piano_arm
            && !self.studio.chromatic_enabled
            && let Some(range) = self.selection()
        {
            self.assign_piano(range);
        }
        self.piano_up(source, note);
        let notes: Vec<_> = if self.studio.chromatic_enabled {
            self.studio.harmony.notes(note).collect()
        } else {
            vec![note]
        };
        self.sync();
        let mut played = Vec::new();
        for note in notes {
            if let Some(range) = self.key_range(note) {
                if self.piano_held.insert((source, note)) {
                    self.send(Command::PianoOn {
                        source, note, velocity, range,
                        bend: self.midi_bend.get(&source).copied().unwrap_or(0.0),
                        controls: self.studio.controls_for_key(note),
                        gate: self.piano_gate,
                    });
                }
                played.push(note);
            }
        }
        self.harmony_held.insert((source, note), played);
    }

    pub fn piano_up(&mut self, source: u64, note: u8) {
        let Some(notes) = self.harmony_held.remove(&(source, note)) else { return };
        for note in notes {
            let shared = self.harmony_held.iter().any(|(&(origin, _), notes)| origin == source && notes.contains(&note));
            if !shared && self.piano_held.remove(&(source, note)) {
                self.send(Command::PianoOff(source, note, self.midi_sustain.contains(&source)));
            }
        }
    }

    pub fn write_arpeggio(&mut self, ascending: bool, division: Quantize) {
        if !self.studio.chromatic_enabled || self.recording || self.key_range(self.piano_key).is_none() {
            return;
        }
        let mut harmony = self.studio.harmony;
        if harmony.chord == crate::harmony::Chord::Single {
            harmony.chord = crate::harmony::Chord::Triad;
        }
        let mut keys: Vec<_> = harmony.notes(self.piano_key).collect();
        if keys.is_empty() { return; }
        if !ascending { keys.reverse(); }
        let unit = division.triplet_unit().unwrap_or(if division == Quantize::Eighth { 240 } else { 120 });
        let origin = self.bar as u32 * crate::model::TICKS_PER_BAR;
        self.studio.edit_roll_notes(|notes| {
            for (index, offset) in (0..crate::model::TICKS_PER_BAR).step_by(unit as usize).enumerate() {
                let tick = origin + offset;
                let input = ChopInput::Key(keys[index % keys.len()]);
                if !notes.iter().any(|note| note.tick == tick && note.input == input) {
                    notes.push(PlayedChop { id: Default::default(), tick, input, velocity: 100, mode: PadMode::Gate, length: Some(unit * 3 / 4), sound: Default::default(), expression: None });
                }
            }
            notes.sort_by_key(|note| (note.tick, note.input));
        });
    }

    pub fn connect_midi_output(&mut self) {
        self.send(Command::MidiOutput(None));
        self.midi_output_failed = None;
        let Some(id) = self.library.midi.output.clone() else {
            self.midi_output_status = None;
            return;
        };
        if self.output.is_none() {
            self.midi_output_status = Some("Start an audio output first. It supplies the MIDI clock.".into());
            return;
        }
        match crate::midi_out::Sink::connect(id, self.library.midi.send_clock) {
            Ok((sink, failed)) => {
                self.send(Command::MidiOutput(Some(Box::new(sink))));
                self.midi_output_failed = Some(failed);
                self.midi_output_status = Some("MIDI output connected.".into());
            }
            Err(error) => self.midi_output_status = Some(error),
        }
    }

    pub fn midi_mapping_file(&mut self, id: String, export: bool) {
        if self.busy.is_some() { return; }
        let dialog = rfd::FileDialog::new().add_filter("MIDI mapping JSON", &["json"]);
        if export {
            let Some(path) = dialog.set_file_name("midi-mapping.json").save_file() else { return };
            let mapping = self.library.midi.ports.get(&id).cloned().unwrap_or_default();
            self.submit_controlled("Exporting MIDI mapping…", move || {
                files::publish_export(&path, |output| files::write_atomically(output, |file| {
                    serde_json::to_writer_pretty(file, &mapping)?;
                    Ok(())
                }))?;
                Ok(Job::Wrote(format!("Exported {}", path.display())))
            });
        } else {
            let Some(path) = dialog.pick_file() else { return };
            self.submit_controlled("Importing MIDI mapping…", move || {
                use std::io::Read;
                let file = std::fs::File::open(path)?;
                if file.metadata()?.len() > 1024 * 1024 { return Err("This MIDI mapping is too large.".into()); }
                let mapping: midi::PortSettings = serde_json::from_reader(file.take(1024 * 1024))?;
                if mapping.channel.is_some_and(|channel| channel > 15) {
                    return Err("The MIDI mapping contains an invalid channel.".into());
                }
                Ok(Job::MidiMapping(id, mapping))
            });
        }
    }

    pub fn midi_settings_changed(&mut self) {
        self.midi_pickup.clear();
        self.midi_panic();
        self.midi.configure(&self.library.midi);
        self.refresh_midi_clock();
        self.persist_library();
    }

    fn selected_chop_controls(&mut self) -> &mut SampleControls {
        if self.library.midi.piano_view {
            let inherited = self.studio.controls_for_key(self.piano_key);
            if self.studio.chromatic_enabled
                && let Some(chromatic) = &mut self.studio.chromatic
            {
                return &mut chromatic.controls;
            }
            self.studio
                .key_controls
                .entry(self.piano_key)
                .or_insert(inherited)
        } else {
            &mut self.studio.pad_controls[self.selected_pad]
        }
    }

    fn refresh_midi_clock(&mut self) {
        let connection = self
            .midi_devices
            .iter()
            .find(|device| {
                Some(&device.id) == self.library.midi.clock.as_ref()
                    && !self
                        .library
                        .midi
                        .ports
                        .get(&device.id)
                        .is_some_and(|port| port.disabled)
            })
            .and_then(|device| device.connection);
        if self.midi_clock_connection != connection {
            self.stop_recording();
            self.midi_clock_connection = connection;
            self.beat_audition_clock_changed();
            self.midi_last_clock = None;
            self.midi_clock_bpm = None;
            self.playing = false;
            self.song_playing = None;
            self.midi.route_clock(
                connection,
                self.output.as_ref().map(Output::sender),
                self.studio.bpm,
            );
        }
    }

    pub fn midi_panic(&mut self) {
        self.send(Command::PianoSilence(None));
        self.piano_held.clear();
        self.harmony_held.clear();
        self.midi_sustain.clear();
        self.midi_bend.clear();
        for (_, pad) in std::mem::take(&mut self.midi_pad_held) {
            self.send(Command::Release(pad));
        }
        self.midi_buttons.clear();
    }

    fn midi_source(connection: u64, channel: u8) -> u64 {
        connection * 16 + u64::from(channel) + 2
    }

    fn midi_release_channel(&mut self, connection: u64, channel: u8) {
        let source = Self::midi_source(connection, channel);
        self.send(Command::PianoSilence(Some(source)));
        self.piano_held.retain(|&(origin, _)| origin != source);
        self.harmony_held.retain(|&(origin, _), _| origin != source);
        self.midi_sustain.remove(&source);
        self.midi_bend.remove(&source);
        let mut released = Vec::new();
        self.midi_pad_held.retain(|&(id, signal), pad| {
            if id == connection
                && matches!(signal, Signal::Note(ch, _) | Signal::Cc(ch, _) if ch == channel)
            {
                released.push(*pad);
                false
            } else {
                true
            }
        });
        self.midi_buttons.retain(|&(id, signal)| {
            id != connection
                || !matches!(signal, Signal::Note(ch, _) | Signal::Cc(ch, _) if ch == channel)
        });
        for pad in released {
            if !self.midi_pad_held.values().any(|&held| held == pad) {
                self.send(Command::Release(pad));
            }
        }
    }

    fn poll_midi(&mut self) {
        for _ in 0..1024 {
            match self.midi.event() {
                Some(midi::Event::Devices(devices)) => {
                    let disconnected: Vec<_> = self
                        .midi_devices
                        .iter()
                        .filter_map(|old| old.connection)
                        .filter(|id| !devices.iter().any(|new| new.connection == Some(*id)))
                        .collect();
                    for id in disconnected {
                        for channel in 0..16 {
                            self.midi_release_channel(id, channel);
                        }
                    }
                    self.midi_devices = devices;
                    self.refresh_midi_clock();
                }
                Some(midi::Event::Message(id, message)) => self.receive_midi(id, message),
                Some(midi::Event::Clock(id, message, period))
                    if self.midi_clock_connection == Some(id) =>
                {
                    self.receive_clock(message, period)
                }
                Some(midi::Event::Clock(..)) => (),
                Some(midi::Event::Error(error)) => self.midi_error = Some(error),
                None => break,
            }
        }
        if self
            .midi_last_clock
            .is_some_and(|last| last.elapsed() > Duration::from_millis(600))
        {
            self.midi_last_clock = None;
            self.midi_clock_bpm = None;
            if self.playing {
                self.set_playing(false);
            }
        }
        if self.midi_last_clock.is_some() {
            self.ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn receive_clock(&mut self, message: Message, period: Option<f64>) {
        match message {
            Message::Tick => {
                self.midi_last_clock = Some(Instant::now());
                if let Some(seconds) = period {
                    let bpm = 60.0 / (24.0 * seconds);
                    self.midi_clock_bpm = Some(bpm);
                    self.studio.bpm = bpm.round().clamp(40.0, 240.0) as u16;
                    self.sync();
                }
            }
            Message::Start | Message::Continue => {
                if message == Message::Start && self.playing { self.beat_audition_clock_jumped(); }
                if self.drum_templates.is_some() {
                    self.stop_drum_preview();
                }
                self.playing = self.output.is_some();
            },
            Message::Stop => {
                self.stop_beat_audition();
                self.playing = false;
                self.song_playing = None;
                self.stop_recording();
            }
            Message::Position(_) => self.beat_audition_clock_jumped(),
            _ => (),
        }
    }

    fn receive_midi(&mut self, connection: u64, message: Message) {
        let Some(device) = self
            .midi_devices
            .iter()
            .find(|device| device.connection == Some(connection))
        else {
            return;
        };
        let profile = device.profile;
        let settings = self.library.midi.ports.get(&device.id);
        if settings.is_some_and(|settings| settings.disabled) { return; }
        let channel = match message {
            Message::Note(channel, ..) | Message::Control(channel, ..) | Message::Bend(channel, ..) => Some(channel),
            _ => None,
        };
        if let Some(wanted) = settings.and_then(|settings| settings.channel)
            && channel.is_some_and(|channel| channel != wanted) { return; }
        let message = match message {
            Message::Note(channel, note, velocity) => Message::Note(channel, note,
                settings.map_or(velocity, |settings| settings.velocity.apply(velocity))),
            message => message,
        };
        match message {
            Message::Start | Message::Continue | Message::Stop
                if profile == midi::Profile::LaunchkeyMk4
                    && self.midi_clock_connection.is_none() =>
            {
                self.set_playing(message != Message::Stop);
            }
            Message::Control(channel, 120 | 123, _) => {
                self.midi_release_channel(connection, channel)
            }
            Message::Bend(channel, value) => {
                let source = Self::midi_source(connection, channel);
                let semitones = (f64::from(value) - 8192.0) / 4096.0;
                self.midi_bend.insert(source, semitones);
                self.send(Command::PianoBend(source, semitones));
            }
            Message::Note(..) | Message::Control(..) => self.midi_signal(connection, message),
            _ => (),
        }
    }

    fn midi_parameter_value(&self, target: Target) -> f32 {
        let controls = if self.library.midi.piano_view {
            if self.studio.chromatic_enabled {
                self.studio.chromatic.map_or_else(SampleControls::default, |sample| sample.controls)
            } else { self.studio.controls_for_key(self.piano_key) }
        } else { self.studio.pad_controls[self.selected_pad] };
        match target {
            Target::SampleLevel => self.studio.sample_level,
            Target::DrumLevel => self.studio.drum_level,
            Target::Filter => self.studio.filter,
            Target::Delay => self.studio.delay,
            Target::Reverb => self.studio.reverb,
            Target::Swing => self.studio.swing,
            Target::ChopGain => controls.gain / 2.0,
            Target::ChopPan => (controls.pan * 63.0 + 64.0) / 127.0,
            Target::ChopTune => (controls.tune * (63.0 / 24.0) + 64.0) / 127.0,
            Target::ChopAttack => f32::from(controls.attack_ms) / 1_000.0,
            Target::ChopRelease => f32::from(controls.release_ms) / 1_000.0,
            _ => 0.0,
        }
    }

    fn midi_signal(&mut self, connection: u64, message: Message) {
        let Some((signal, value)) = message.signal() else {
            return;
        };
        let Some(device) = self
            .midi_devices
            .iter()
            .find(|device| device.connection == Some(connection))
        else {
            return;
        };
        let settings = self.library.midi.ports.get(&device.id);
        let route = settings.map_or(Route::Auto, |settings| settings.route);
        let profile = device.profile;
        let settings = self.library.midi.ports.get(&device.id);
        let cc_mode = settings.map_or(midi::CcMode::Absolute, |settings| settings.cc_mode);
        let pickup = settings.is_some_and(|settings| settings.soft_takeover);
        let mapped = settings
            .and_then(|settings| {
                settings
                    .bindings
                    .iter()
                    .find(|(control, _)| *control == signal)
            })
            .map(|&(_, target)| target);
        let learn = self
            .midi_learn
            .filter(|target| {
                self.midi_selected.as_ref() == Some(&device.id)
                    && (matches!(signal, Signal::Cc(..)) || value > 0)
                    && (!target.continuous() || matches!(signal, Signal::Cc(..)))
            })
            .map(|target| (device.id.clone(), target));
        self.midi_last = Some((connection, signal, value));

        let control_id = (connection, signal);
        let down = match signal {
            Signal::Note(..) => value != 0,
            Signal::Cc(..) => value >= 64,
        };
        if !down {
            self.midi_buttons.remove(&control_id);
            if let Some(pad) = self.midi_pad_held.remove(&control_id)
                && !self.midi_pad_held.values().any(|&held| held == pad)
            {
                self.send(Command::Release(pad));
            }
            if let Signal::Note(channel, note) = signal {
                self.piano_up(Self::midi_source(connection, channel), note);
            }
        }
        if let Some((device, target)) = learn {
            let bindings = &mut self.library.midi.ports.entry(device).or_default().bindings;
            bindings.retain(|&(old, action)| old != signal && action != target);
            bindings.push((signal, target));
            self.midi_learn = None;
            self.persist_library();
            return;
        }

        let target = mapped.or_else(|| match signal {
            Signal::Cc(_, 7) => Some(Target::SampleLevel),
            Signal::Cc(_, 74) => Some(Target::Filter),
            _ if matches!(route, Route::Auto | Route::Pads) => profile.pad(signal).map(Target::Pad),
            _ => None,
        });
        if let Some(target) = target {
            if target.continuous() {
                let mut normalized = f32::from(value) / 127.0;
                if mapped.is_some() && matches!(signal, Signal::Cc(..)) {
                    let current = self.midi_parameter_value(target);
                    if let Some(delta) = cc_mode.delta(value) {
                        normalized = (current + f32::from(delta) / 127.0).clamp(0.0, 1.0);
                    } else if pickup {
                        let state = self.midi_pickup.entry(control_id).or_insert((normalized, false));
                        state.1 |= (current - normalized).abs() <= 2.0 / 127.0
                            || (state.0 - current) * (normalized - current) <= 0.0;
                        state.0 = normalized;
                        if !state.1 { return; }
                    }
                }
                let value = normalized;
                match target {
                    Target::SampleLevel => self.studio.sample_level = value,
                    Target::DrumLevel => self.studio.drum_level = value,
                    Target::Filter => {
                        self.studio.filter = if mapped.is_some() { value } else { 1.0 - value }
                    }
                    Target::Delay => self.studio.delay = value,
                    Target::Reverb => self.studio.reverb = value,
                    Target::Swing => self.studio.swing = value,
                    Target::ChopGain => self.selected_chop_controls().gain = value * 2.0,
                    Target::ChopPan => {
                        self.selected_chop_controls().pan =
                            ((value * 127.0 - 64.0) / 63.0).max(-1.0)
                    }
                    Target::ChopTune => {
                        self.selected_chop_controls().tune =
                            (((value * 127.0 - 64.0) / 63.0).max(-1.0) * 24.0).round()
                    }
                    Target::ChopAttack => {
                        self.selected_chop_controls().attack_ms = (value * 1000.0).round() as u16
                    }
                    Target::ChopRelease => {
                        self.selected_chop_controls().release_ms = (value * 1000.0).round() as u16
                    }
                    _ => (),
                }
                self.sync();
            } else if down && self.midi_buttons.insert(control_id) {
                match target {
                    Target::Play => self.set_playing(true),
                    Target::Stop => self.set_playing(false),
                    Target::Record => self.toggle_recording(),
                    Target::Loop => self.toggle_loop(),
                    Target::Preview => self.toggle_preview(),
                    Target::Tap if self.midi_clock_connection.is_none() => self.tap(),
                    Target::ChopReverse => {
                        let controls = self.selected_chop_controls();
                        controls.reverse = !controls.reverse;
                    }
                    Target::Pad(pad) if pad < STEPS => {
                        self.midi_pad_held.insert(control_id, pad);
                        self.play_pad(pad, false, value);
                    }
                    Target::Drum(drum) if drum < DRUMS.len() => {
                        self.send(Command::MidiDrum(drum, value))
                    }
                    _ => (),
                }
            }
            return;
        }

        match signal {
            Signal::Cc(channel, 64) => {
                let source = Self::midi_source(connection, channel);
                if down {
                    self.midi_sustain.insert(source);
                } else if self.midi_sustain.remove(&source) {
                    self.send(Command::PianoPedalUp(source));
                }
            }
            Signal::Note(channel, note) if down && route != Route::Controls => {
                let piano =
                    route == Route::Piano || route == Route::Auto && self.library.midi.piano_view;
                if piano {
                    self.piano_down(Self::midi_source(connection, channel), note, value);
                } else if let Some(pad) = Studio::pad_for_key(note).or_else(|| {
                    note.checked_sub(24)
                        .map(usize::from)
                        .filter(|&pad| pad < STEPS)
                }) {
                    self.midi_pad_held.insert(control_id, pad);
                    self.play_pad(pad, false, value);
                }
            }
            _ => (),
        }
    }

    pub fn tap(&mut self) {
        let now = Instant::now();
        if self
            .taps
            .last()
            .is_some_and(|last| now.duration_since(*last) > Duration::from_secs(2))
        {
            self.taps.clear();
        }
        self.taps.push(now);
        if self.taps.len() > 6 {
            self.taps.remove(0);
        }
        if self.taps.len() >= 2 {
            let mean =
                now.duration_since(self.taps[0]).as_secs_f64() / (self.taps.len() - 1) as f64;
            self.studio.bpm = (60.0 / mean).round().clamp(40.0, 240.0) as u16;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        for (index, key) in PIANO_KEYS.into_iter().enumerate() {
            if ctx.input(|input| input.key_released(key) || !input.focused)
                && let Some(note) = self.computer_notes[index].take()
            {
                self.piano_up(1, note);
            }
        }
        for (index, key) in PAD_KEYS.into_iter().enumerate() {
            if ctx.input(|input| input.key_released(key)) {
                self.send(Command::Release(index));
            }
        }
        let pressed = |target, command, shift: Option<bool>| {
            ctx.input_mut(|input| {
                let mut found = false;
                input.events.retain(|event| {
                    let matched = matches!(event, egui::Event::Key {
                        key, pressed: true, repeat: false, modifiers, ..
                    } if *key == target && modifiers.command == command && !modifiers.alt
                        && shift.is_none_or(|shift| modifiers.shift == shift));
                    found |= matched;
                    !matched
                });
                found
            })
        };
        if self.show_commands || self.drum_templates.is_some() || self.variations.is_some()
            || self.snapshot_dialog.is_some()
        { return; }
        if Action::Commands.pressed(pressed) {
            Action::Commands.run(self);
            return;
        }
        let (focused, popup) = ctx.memory(|memory| (memory.focused(), memory.any_popup_open()));
        let editing_text = focused.is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if self.youtube_url.is_none() && !self.daw_dialog && !self.midi_dialog && self.export_dialog.is_none() {
            if Action::SaveAs.pressed(pressed) {
                Action::SaveAs.run(self);
            } else if Action::Save.pressed(pressed) {
                Action::Save.run(self);
            }
            if !popup && !editing_text {
                if Action::Redo.pressed(pressed) {
                    Action::Redo.run(self);
                } else if Action::Undo.pressed(pressed) {
                    Action::Undo.run(self);
                }
            }
        }
        if self.youtube_url.is_some()
            || self.daw_dialog
            || self.export_dialog.is_some()
            || self.midi_dialog
            || self.piano_roll.is_some()
            || self.transcription.editor.is_some()
            || self.sequence_tool.is_some()
        {
            return;
        }
        let key = |key| pressed(key, false, None);
        if Action::Open.pressed(pressed) {
            Action::Open.run(self);
        }
        if Action::Import.pressed(pressed) {
            Action::Import.run(self);
        }
        if popup || editing_text {
            return;
        }
        if let Some(widget) = focused && key(Key::Escape) {
            ctx.memory_mut(|memory| memory.surrender_focus(widget));
            return;
        }
        if self.page == Page::Queue {
            if key(Key::Z) {
                self.undo_queue();
            }
            let index = self.queue_focused();
            let down = focused.is_none() && key(Key::ArrowDown);
            if focused.is_none() && (down || key(Key::ArrowUp)) {
                let visible = self.queue_indices();
                if let Some(row) = index.and_then(|index| visible.iter().position(|&item| item == index)) {
                    let next = if down { (row + 1).min(visible.len() - 1) } else { row.saturating_sub(1) };
                    self.queue_focus = Some(self.library.queue[visible[next]].record.source.clone());
                }
                return;
            }
            if let Some(index) = index {
                if focused.is_none() && key(Key::Space) {
                    self.preview_queue(index);
                } else if focused.is_none() && key(Key::Enter) {
                    self.open_queue(index);
                } else if key(Key::K) {
                    self.queue_decide(index, Verdict::Keep);
                } else if key(Key::J) {
                    self.queue_decide(index, Verdict::Skip);
                } else if key(Key::L) {
                    self.queue_decide(index, Verdict::Later);
                }
            }
            return;
        }
        if focused.is_none() && Action::Preview.pressed(pressed) {
            Action::Preview.run(self);
        }
        if focused.is_none() && Action::Pattern.pressed(pressed) {
            Action::Pattern.run(self);
        }
        if key(Key::Backtick) {
            self.keyboard = !self.keyboard;
            if self.keyboard {
                self.page = Page::Studio;
            }
        }
        let shift = ctx.input(|input| input.modifiers.shift);
        if self.page == Page::Studio && self.keyboard {
            if self.library.midi.piano_view {
                for (index, key_code) in PIANO_KEYS.into_iter().enumerate() {
                    if key(key_code) {
                        let note = 12 * (self.piano_octave + 1) + index as u8;
                        if (PIANO_LOW..=PIANO_HIGH).contains(&note) {
                            self.computer_notes[index] = Some(note);
                            self.piano_down(1, note, 127);
                        }
                    }
                }
                return;
            } else {
                for (index, key_code) in PAD_KEYS.into_iter().enumerate() {
                    if key(key_code) {
                        self.pad(index, shift);
                    }
                }
                if key(Key::Backspace) {
                    self.clear_pad(self.selected_pad);
                }
            }
        } else {
            if key(Key::D) {
                self.dig();
            }
            if key(Key::O) {
                self.prev();
            }
            if key(Key::P) {
                self.next();
            }
            if key(Key::F) {
                self.jump();
            }
            if key(Key::W) {
                self.save_record();
            }
            if key(Key::S) {
                self.page = if self.page == Page::Studio {
                    Page::Dig
                } else {
                    Page::Studio
                };
            }
            for action in [Action::LoopIn, Action::LoopOut, Action::ToggleLoop] {
                if action.pressed(pressed) {
                    action.run(self);
                }
            }
        }
        if key(Key::B) {
            self.tap();
        }
        if focused.is_none() && self.page == Page::Studio
            && let Some(time) = &mut self.studio.pads[self.selected_pad]
        {
            let amount = if shift { 0.1 } else { 0.01 };
            if key(Key::ArrowLeft) {
                *time -= amount;
            }
            if key(Key::ArrowRight) {
                *time += amount;
            }
        }
    }
}

fn youtube_id(record: &Record) -> Option<&str> {
    source_youtube_id(&record.source)
}

fn source_youtube_id(source: &Source) -> Option<&str> {
    match source {
        Source::YouTube { id } => Some(id),
        _ => None,
    }
}

fn select_record(records: &[Record], active: Option<&Record>, seed: usize) -> Record {
    let index = seed % records.len();
    let index = if records.len() > 1 && active == Some(&records[index]) {
        (index + 1) % records.len()
    } else {
        index
    };
    records[index].clone()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.read_recorded_chops();
        if let Some(output) = &mut self.output {
            output.performance.poll();
        }
        if let Some(output) = &self.output {
            if self
                .seeked_at
                .is_none_or(|time| time.elapsed() > Duration::from_millis(60))
            {
                self.position = output.meters.position().min(self.duration());
            }
            self.preview = output.meters.preview();
            self.step = output.meters.step();
            self.scene_active = output.meters.scene();
            if self.scene_active.is_some() && self.scene_active == self.scene_pending {
                self.scene_pending = None;
            }
            if self.song_playing == Some(output.meters.finished_song()) {
                self.playing = false;
                self.song_playing = None;
            }
            if output.meters.failed() {
                self.audio_error = Some("Audio device disconnected.".into());
                self.recording = false;
                self.recorded_gates.clear();
                self.output = None;
                self.playing = false;
                self.song_playing = None;
                self.preview = false;
            }
        }
        self.poll_taste_listening();
        self.finish_job();
        if self.midi_output_failed.as_ref().is_some_and(|failed| failed.load(std::sync::atomic::Ordering::Relaxed)) {
            self.send(Command::MidiOutput(None));
            self.midi_output_status = Some("MIDI output disconnected. Reconnect from MIDI devices.".into());
            self.midi_output_failed = None;
        }
        if self.input_recording.as_ref().is_some_and(|take| take.stop_reason().is_some())
            || (self.input_recording.is_some() && ctx.input(|input| input.viewport().close_requested()))
        {
            self.finish_input();
        }
        if !self.finishing_close && self.pending_change.is_none() && !self.session.show_recovery {
            self.poll_midi();
        }
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .and_then(|file| file.path.clone())
        });
        if let Some(path) = dropped
            && self.youtube_url.is_none()
            && self.pending_change.is_none()
            && !self.finishing_close
            && !self.session.show_recovery
        {
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("flip"))
            {
                self.open(path);
            } else {
                self.load(files::local_record(path), None, None);
            }
        }
        let was_recording = self.recording;
        if !self.finishing_close && self.pending_change.is_none() && !self.session.show_recovery {
            self.shortcuts(ctx);
        }
        crate::ui::draw(self, ctx);
        self.project_dialogs(ctx);
        if self.page != Page::Studio {
            self.piano_arm = false;
        }
        if (self.page != Page::Studio || !self.library.midi.piano_view || self.midi_dialog)
            && let Some(note) = self.piano_pointer.take()
        {
            self.piano_up(0, note);
        }
        if self.page != Page::Queue && self.queue_playing.is_some() {
            self.stop_listening();
        }
        if self
            .queue_preview_started
            .is_some_and(|start| start.elapsed() > Duration::from_millis(200))
            && self
                .output
                .as_ref()
                .is_none_or(|output| !output.meters.listening())
        {
            self.stop_listening();
        }
        let pointer_edit = ctx.input(|input| input.pointer.primary_down());
        self.observe_edits(pointer_edit || self.recording || was_recording);
        self.poll_analysis();
        self.poll_beat_audition();
        self.poll_sample_search();
        self.poll_taste();
        self.poll_transcription();
        self.poll_extraction();
        if !self.closing && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.finishing_close {
                if let Some(job) = &self.job_control {
                    job.cancel();
                }
                self.reset_analysis();
                self.stop_beat_audition();
                self.close_sample_search();
                self.clear_transcription();
                self.clear_extraction();
                if self.session.dirty() {
                    self.stop_recording();
                    self.pending_change.get_or_insert(PendingChange::Quit);
                } else if self.busy.is_some() || self.analysis_job().is_some() || self.extraction_job().is_some() {
                    self.pending_change.get_or_insert(PendingChange::Quit);
                } else {
                    self.finish_close();
                }
            }
        }
        if self.busy.is_none() && self.analysis_job().is_none() && self.extraction_job().is_none()
            && (!self.session.dirty() || self.discard_after_analysis)
            && matches!(self.pending_change, Some(PendingChange::Quit))
        {
            let discard = std::mem::take(&mut self.discard_after_analysis);
            self.continue_change(discard);
        }
        let recovery_enabled = self.input_recording.is_none()
            && self.pending_change.is_none()
            && !self.finishing_close;
        if let Some(error) = self.session.autosave(self.clip.as_ref(), recovery_enabled) {
            self.status = Some((error, true));
            ctx.request_repaint();
        }
        self.poll_library_persistence();
        if self.finishing_close
            && !self.closing
            && !self.session.cleaning_recovery()
            && !self.library_persistence.pending()
            && self.crate_save_error().is_none()
        {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.clip.is_some()
            || self.input_recording.is_some()
            || self.session.dirty()
            || self.playing
            || self.busy.is_some()
            || self.queue_playing.is_some()
            || self.drum_templates.is_some()
            || self.analysis_job().is_some()
            || self.search_job().is_some()
            || self.taste_waiting()
            || self.transcription_job().is_some()
            || self.extraction_job().is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a working Windows audio output device"]
    fn snapshot_comparison_matches_levels_and_leaves_working_state_intact() -> files::Result<()> {
        let mut app = App::new(egui::Context::default());
        assert!(app.output.is_some());
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.25; 2]; 4_000])));
        app.send(Command::Clip(app.clip.clone()));
        app.studio.bpm = 120;
        app.studio.bars = 2;
        app.studio.pads[0] = Some(0.0);
        app.studio.chops[1][0] = Some(0);
        app.save_named_snapshot("Snapshot A/B")?;
        app.studio.sample_level = 0.5;
        app.studio.bpm = 100;
        app.observe_edits(false);
        let before = app.project_snapshot();
        app.prepare_snapshot_preview(0, false, 1, true)?;
        let start = Instant::now();
        while app.busy.is_some() && start.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(10));
            app.finish_job();
        }
        let (clips, gains) = app.snapshot_preview.as_ref().ok_or("Comparison did not render")?;
        assert_eq!(clips[0].frames.len(), 115_200);
        assert_eq!(clips[1].frames.len(), 96_000);
        assert_eq!(gains[0], 1.0);
        assert!((0.4..0.6).contains(&gains[1]));
        for side in 0..2 {
            app.play_snapshot_preview(side);
            let start = Instant::now();
            while !app.output.as_ref().unwrap().meters.listening()
                && start.elapsed() < Duration::from_millis(300)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(app.output.as_ref().unwrap().meters.listening());
            assert!(app.project_snapshot() == before);
        }
        app.clear_snapshot_preview();
        Ok(())
    }

    #[test]
    fn snapshot_restore_and_variation_keep_each_create_one_undo_step() -> files::Result<()> {
        let mut app = App::new(egui::Context::default());
        app.studio.bpm = 108;
        app.studio.hits[0][0][0] = 2;
        app.save_named_snapshot("Original")?;
        app.studio.bpm = 122;
        app.studio.hits[0][0][0] = 0;
        app.observe_edits(false);
        let working = app.project_snapshot();
        assert!(app.restore_named_snapshot(0));
        assert_eq!(app.studio.bpm, 108);
        assert_eq!(app.studio.hits[0][0][0], 2);
        app.undo_project(false);
        assert!(app.project_snapshot() == working);
        let mut candidate = app.studio.active_sequence();
        candidate.bars[0].drums[7][8] = 2;
        assert!(app.keep_variation("Fill", candidate));
        assert_eq!(app.studio.patterns[0].name, "Fill");
        assert_eq!(app.studio.hits[0][7][8], 2);
        app.undo_project(false);
        assert!(app.project_snapshot() == working);
        Ok(())
    }

    #[test]
    #[ignore = "requires a working Windows audio output device"]
    fn stopped_transport_capture_survives_undo_reopen_and_export() -> files::Result<()> {
        let mut app = App::new(egui::Context::default());
        let output = app.output.as_ref().ok_or("No audio device opened")?;
        println!("Audio output {} at {} Hz, buffer request {:?}",
            output.device_name, output.sample_rate, output.requested_buffer);
        let clip = Arc::new(Clip::new(8_000, vec![[0.25; 2]; 4_000]));
        app.clip = Some(clip.clone());
        app.send(Command::Clip(app.clip.clone()));
        app.studio.pads[0] = Some(0.0);
        app.studio.pad_ends[0] = Some(0.4);
        app.studio.hits[0][0][4] = 1;
        app.observe_edits(false);
        let before = app.project_snapshot();

        app.send(Command::Pad(0, PadMode::Gate, 99));
        std::thread::sleep(Duration::from_millis(90));
        app.send(Command::Release(0));
        app.preview_performance(4);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            let output = app.output.as_mut().unwrap();
            output.performance.poll();
            if output.performance.preview().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.project_snapshot() == before);
        assert!(!app.recording && !app.playing);
        app.keep_performance();
        let captured = app.project_snapshot();
        assert_eq!(captured.studio.patterns.len(), 2);
        assert_eq!(captured.studio.patterns[0].notes.bars[0].drums[0][4], 1);
        let note = &captured.studio.performance[0];
        assert_eq!(note.tick, 0);
        assert_eq!(note.velocity, 99);
        assert!(note.length.is_some_and(|length| length > 1));
        app.undo_project(false);
        assert!(app.project_snapshot() == before);
        app.undo_project(true);
        assert!(app.project_snapshot() == captured);

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("capture.flip");
        files::save_project(&path, &captured, Some(&clip))?;
        let (project, media) = files::open_project(&path)?;
        assert!(project == captured);
        let midi = directory.path().join("capture.mid");
        files::export_midi(&midi, &project.studio)?;
        assert!(std::fs::read(midi)?.windows(3).any(|bytes| bytes == [0x90, 24, 99]));
        let wav = directory.path().join("capture.wav");
        files::export_wav(&wav, project.studio, media.map(Arc::new), files::WavOptions::default())?;
        assert!(hound::WavReader::open(wav)?.samples::<i32>().take(10_000)
            .any(|sample| sample.is_ok_and(|sample| sample > 0)));
        use std::sync::atomic::Ordering::Relaxed;
        let meters = &app.output.as_ref().unwrap().meters;
        println!("Observed buffer {} frames; short-test peak callback {:.3} ms",
            meters.callback_frames.load(Relaxed),
            meters.callback_peak_ns.load(Relaxed) as f64 / 1e6);
        Ok(())
    }

    #[test]
    fn undo_uses_key_event_modifiers_after_control_is_released() {
        let ctx = egui::Context::default();
        let mut app = App::new(ctx.clone());
        app.studio.bpm = 105;
        app.observe_edits(false);
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers { ctrl: true, command: true, ..Default::default() },
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.shortcuts(ctx));
        assert_eq!(app.studio.bpm, 90);
        assert!(!app.project_dirty());
    }

    #[test]
    #[ignore = "requires a Windows MIDI loopback and an audio output device"]
    fn windows_port_records_midi_pads_and_keys_into_the_pattern() {
        let mut app = App::new(egui::Context::default());
        assert!(app.output.is_some(), "{:?}", app.audio_error);
        app.library.midi = midi::Settings::default();
        app.library.midi.piano_view = true;
        app.midi.configure(&app.library.midi);
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 8_000])));
        app.studio.pads[0] = Some(0.0);
        app.studio.key_chops.insert(108, KeyChop::Cut(0.1, 0.9));
        app.send(Command::Clip(app.clip.clone()));
        app.toggle_recording();
        let timeout = Instant::now() + Duration::from_secs(15);
        while Instant::now() < timeout {
            app.poll_midi();
            app.read_recorded_chops();
            app.sync();
            let recorded = |input, velocity| {
                app.studio
                    .performance
                    .iter()
                    .any(|event| event.input == input && event.velocity == velocity)
            };
            if recorded(ChopInput::Pad(0), 87) && recorded(ChopInput::Key(108), 105) {
                return;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        panic!(
            "MIDI performance was not captured: {:?}",
            app.studio.performance
        );
    }
}
