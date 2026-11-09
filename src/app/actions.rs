use super::{App, Page, StudioView};
use crate::ui::{drum_templates, export, piano_roll, sample::Tool, snapshots};
use eframe::egui::Key;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Commands,
    Open,
    Save,
    SaveAs,
    Import,
    ImportYouTube,
    NewFromTemplate,
    SaveTemplate,
    Recover,
    Undo,
    Redo,
    Snapshots,
    Preview,
    Pattern,
    Song,
    Stop,
    Page(Page),
    Studio(StudioView),
    Sample(Tool),
    AddSource,
    AnalyzeBeats,
    Transcribe,
    SuggestCuts,
    ExportLoop,
    Export,
    ExportMidi,
    Templates,
    PianoRoll,
    IndexFolder,
    MidiSetup,
    AudioSetup,
    LoopIn,
    LoopOut,
    ToggleLoop,
}

#[derive(Clone, Copy)]
struct Shortcut {
    key: Key,
    command: bool,
    shift: Option<bool>,
    text: &'static str,
}

impl Action {
    pub(crate) const PALETTE: &[Self] = &[
        Self::Open,
        Self::Save,
        Self::SaveAs,
        Self::Import,
        Self::ImportYouTube,
        Self::NewFromTemplate,
        Self::SaveTemplate,
        Self::Recover,
        Self::Undo,
        Self::Redo,
        Self::Snapshots,
        Self::Preview,
        Self::Pattern,
        Self::Song,
        Self::Stop,
        Self::Page(Page::Dig),
        Self::Page(Page::Queue),
        Self::Page(Page::Crate),
        Self::Page(Page::Search),
        Self::Studio(StudioView::Sample),
        Self::Studio(StudioView::Perform),
        Self::Studio(StudioView::Sequence),
        Self::Studio(StudioView::Arrange),
        Self::Studio(StudioView::Mix),
        Self::Sample(Tool::Pads),
        Self::AddSource,
        Self::Sample(Tool::Slice),
        Self::Sample(Tool::BeatMap),
        Self::AnalyzeBeats,
        Self::Sample(Tool::Warp),
        Self::Sample(Tool::Groove),
        Self::Transcribe,
        Self::Sample(Tool::Transcribe),
        Self::Sample(Tool::Extract),
        Self::Sample(Tool::Stems),
        Self::Sample(Tool::Input),
        Self::SuggestCuts,
        Self::LoopIn,
        Self::LoopOut,
        Self::ToggleLoop,
        Self::ExportLoop,
        Self::Export,
        Self::ExportMidi,
        Self::Templates,
        Self::PianoRoll,
        Self::IndexFolder,
        Self::MidiSetup,
        Self::AudioSetup,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Commands => "Commands and settings",
            Self::Open => "Open project…",
            Self::Save => "Save project",
            Self::SaveAs => "Save project as…",
            Self::Import => "Import audio…",
            Self::ImportYouTube => "Import from YouTube URL…",
            Self::NewFromTemplate => "New from template…",
            Self::SaveTemplate => "Save as template…",
            Self::Recover => "Recover session…",
            Self::Undo => "Undo project edit",
            Self::Redo => "Redo project edit",
            Self::Snapshots => "Project snapshots…",
            Self::Preview => "Preview source",
            Self::Pattern => "Play or stop pattern",
            Self::Song => "Play or stop song",
            Self::Stop => "Stop all playback",
            Self::Page(Page::Dig) => "Open Dig",
            Self::Page(Page::Queue) => "Open Queue",
            Self::Page(Page::Crate) => "Open Crate",
            Self::Page(Page::Search) => "Search local samples",
            Self::Page(Page::Studio) => "Open Studio",
            Self::Studio(StudioView::Sample) => "Open waveform editor",
            Self::Studio(StudioView::Perform) => "Open performance controls",
            Self::Studio(StudioView::Sequence) => "Open step sequencer",
            Self::Studio(StudioView::Arrange) => "Open song arrangement",
            Self::Studio(StudioView::Mix) => "Open mixer",
            Self::Sample(Tool::Transcribe) => "Basic Pitch setup",
            Self::Sample(tool) => tool.title(),
            Self::AddSource => "Add audio source…",
            Self::AnalyzeBeats => "Analyze beat map with pulse estimator",
            Self::Transcribe => "Transcribe to notes…",
            Self::SuggestCuts => "Suggest cuts",
            Self::ExportLoop => "Export loop WAV…",
            Self::Export => "Export project audio or MIDI…",
            Self::ExportMidi => "Export MIDI…",
            Self::Templates => "Drum templates…",
            Self::PianoRoll => "Piano roll…",
            Self::IndexFolder => "Add sample folder to search index…",
            Self::MidiSetup => "MIDI devices and setup…",
            Self::AudioSetup => "Audio output setup",
            Self::LoopIn => "Set selection start at playhead",
            Self::LoopOut => "Set selection end at playhead",
            Self::ToggleLoop => "Toggle source loop",
        }
    }

    fn shortcut(self) -> Option<Shortcut> {
        let (key, command, shift, text) = match self {
            Self::Commands => (Key::K, true, Some(false), "Ctrl+K"),
            Self::Open => (Key::O, true, Some(false), "Ctrl+O"),
            Self::Save => (Key::S, true, Some(false), "Ctrl+S"),
            Self::SaveAs => (Key::S, true, Some(true), "Ctrl+Shift+S"),
            Self::Import => (Key::I, true, Some(false), "Ctrl+I"),
            Self::Undo => (Key::Z, true, Some(false), "Ctrl+Z"),
            Self::Redo => (Key::Z, true, Some(true), "Ctrl+Shift+Z / Ctrl+Y"),
            Self::Preview => (Key::Space, false, None, "Space"),
            Self::Pattern => (Key::Enter, false, None, "Enter"),
            Self::LoopIn => (Key::Z, false, None, "Z"),
            Self::LoopOut => (Key::C, false, None, "C"),
            Self::ToggleLoop => (Key::X, false, None, "X"),
            _ => return None,
        };
        Some(Shortcut {
            key,
            command,
            shift,
            text,
        })
    }

    pub(crate) fn shortcut_text(self) -> &'static str {
        self.shortcut().map_or("", |shortcut| shortcut.text)
    }

    pub(super) fn pressed(self, consume: impl Fn(Key, bool, Option<bool>) -> bool) -> bool {
        self.shortcut()
            .is_some_and(|key| consume(key.key, key.command, key.shift))
            || (self == Self::Redo && consume(Key::Y, true, Some(false)))
    }

    pub(crate) fn matches(self, query: &str) -> bool {
        let title = self.title().to_lowercase();
        let shortcuts = self.shortcut_text().to_lowercase();
        let keywords = match self {
            Self::Page(Page::Search) | Self::IndexFolder => "clap library",
            Self::Sample(Tool::Extract) => "sam audio cuda gpu setup",
            Self::Sample(Tool::Stems) => "demucs setup",
            Self::Sample(Tool::BeatMap) => "beat this downbeats setup",
            Self::Transcribe => "basic pitch notes",
            Self::Sample(Tool::Input) => "microphone line recording",
            Self::AudioSetup => "speaker playback device sample rate buffer",
            _ => "",
        };
        query
            .split_whitespace()
            .all(|term| title.contains(term) || shortcuts.contains(term) || keywords.contains(term))
    }

    pub(crate) fn unavailable(self, app: &App) -> Option<&'static str> {
        let working = app.busy.is_some() || app.input_recording.is_some();
        let source = app.active_clip().is_some();
        let reason = match self {
            Self::Open
            | Self::Save
            | Self::SaveAs
            | Self::Import
            | Self::ImportYouTube
            | Self::NewFromTemplate
            | Self::SaveTemplate
            | Self::Recover
            | Self::Export
            | Self::ExportMidi
            | Self::ExportLoop
            | Self::SuggestCuts
                if working =>
            {
                "Finish the current job or input recording."
            }
            Self::Recover if !app.can_recover() => "No recovery snapshot is available.",
            Self::Undo if !app.can_undo_project(false) => "No project edit is available to undo.",
            Self::Redo if !app.can_undo_project(true) => "No project edit is available to redo.",
            Self::Snapshots | Self::Transcribe | Self::Templates | Self::PianoRoll | Self::Song
                if app.recording =>
            {
                "Finish pattern recording first."
            }
            Self::Preview
            | Self::LoopIn
            | Self::LoopOut
            | Self::Transcribe
            | Self::SuggestCuts
            | Self::AnalyzeBeats
                if !source =>
            {
                "Load a source first."
            }
            Self::Preview | Self::Pattern | Self::Stop | Self::Song if app.output.is_none() => {
                "Connect an audio output first."
            }
            Self::Song
                if app.song_playing.is_none()
                    && !(1..=crate::model::MAX_SONG_BARS).contains(&app.studio.song_bars()) =>
            {
                "Arrange a playable song first."
            }
            Self::Sample(Tool::Slice) | Self::ExportLoop | Self::ToggleLoop
                if app.selection().is_none() =>
            {
                "Select a range on the waveform first."
            }
            Self::AddSource if !app.can_add_source() => {
                "Finish the current job or free a project source slot."
            }
            Self::AnalyzeBeats if app.analysis_job().is_some() => {
                "Finish the current beat analysis."
            }
            Self::AnalyzeBeats
                if {
                    let (start, end) = app.selection().unwrap_or((0.0, app.duration()));
                    !(0.2..=crate::analysis_worker::MAX_SECONDS).contains(&(end - start))
                } =>
            {
                "Select 0.2 seconds to three minutes of audio."
            }
            Self::Transcribe
                if {
                    let (start, end) = app.selection().unwrap_or((0.0, app.duration()));
                    end - start > crate::transcription::MAX_SECONDS
                } =>
            {
                "Select at most three minutes of audio."
            }
            Self::PianoRoll if !source && app.studio.performance.is_empty() => {
                "Load a source or record a performance first."
            }
            Self::ExportMidi if !app.studio.has_pattern() && app.studio.sections.is_empty() => {
                "Add pattern notes or arrange a song first."
            }
            Self::IndexFolder if app.search_job().is_some() => {
                "Finish or pause the current search job."
            }
            _ => return None,
        };
        Some(reason)
    }

    pub(crate) fn run(self, app: &mut App) {
        if self.unavailable(app).is_some() {
            return;
        }
        match self {
            Self::Commands => app.open_commands(),
            Self::Open => app.open_dialog(),
            Self::Save => app.save_dialog(),
            Self::SaveAs => app.save_project(true),
            Self::Import => app.import(),
            Self::ImportYouTube => {
                app.status = None;
                app.youtube_url = Some(String::new());
            }
            Self::NewFromTemplate => app.new_from_template(),
            Self::SaveTemplate => app.save_template(),
            Self::Recover => app.show_recovery(),
            Self::Undo => app.undo_project(false),
            Self::Redo => app.undo_project(true),
            Self::Snapshots => app.snapshot_dialog = Some(snapshots::Dialog::default()),
            Self::Preview => app.toggle_preview(),
            Self::Pattern => app.toggle_pattern(),
            Self::Song if app.song_playing.is_some() => app.stop_song(),
            Self::Song => app.play_song(),
            Self::Stop => app.stop_all(),
            Self::Page(Page::Search) => app.open_search(),
            Self::Page(page) => app.page = page,
            Self::Studio(view) => open_studio(app, view),
            Self::Sample(tool) => open_sample_tool(app, tool),
            Self::AddSource => app.add_source(),
            Self::AnalyzeBeats => {
                open_sample_tool(app, Tool::BeatMap);
                app.analyze_beat_map(false);
            }
            Self::Transcribe => {
                open_sample_tool(app, Tool::Transcribe);
                app.open_transcription();
            }
            Self::SuggestCuts => {
                open_sample_tool(app, Tool::Suggestions);
                app.suggest_cuts();
            }
            Self::ExportLoop => {
                if let Some(range) = app.selection() {
                    app.export_region(range, "loop.wav");
                }
            }
            Self::Export => app.export_dialog = Some(export::Dialog::new(app)),
            Self::ExportMidi => app.export("mid"),
            Self::Templates => {
                open_studio(app, StudioView::Sequence);
                app.drum_templates = Some(drum_templates::Browser::default());
            }
            Self::PianoRoll => app.piano_roll = Some(piano_roll::PianoRoll::new(app)),
            Self::IndexFolder => {
                app.open_search();
                app.index_search_folder();
            }
            Self::MidiSetup => app.midi_dialog = true,
            Self::AudioSetup => {
                open_studio(app, StudioView::Mix);
                app.reveal_audio_output = true;
            }
            Self::LoopIn => app.set_loop_edge(true),
            Self::LoopOut => app.set_loop_edge(false),
            Self::ToggleLoop => app.toggle_loop(),
        }
    }
}

fn open_studio(app: &mut App, view: StudioView) {
    app.page = Page::Studio;
    if view == StudioView::Arrange {
        app.open_arrangement();
    } else {
        app.arrangement_timeline.cancel_drag();
        app.studio_view = view;
    }
}

fn open_sample_tool(app: &mut App, tool: Tool) {
    open_studio(app, StudioView::Sample);
    app.sample_tool = Some(tool);
}
