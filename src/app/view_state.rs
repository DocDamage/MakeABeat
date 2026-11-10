use crate::{
    ui::{arrangement::Timeline, piano_roll::PianoRoll, sample, sequencer},
    waveform::{Snap, WaveformView},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Dig,
    Queue,
    Studio,
    Search,
    Crate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StudioView {
    #[default]
    Sample,
    Perform,
    Sequence,
    Arrange,
    Mix,
}

#[derive(Default)]
pub(crate) struct CommandPalette {
    pub open: bool,
    pub focus: bool,
    pub query: String,
}

pub(crate) struct ViewState {
    pub page: Page,
    pub studio_view: StudioView,
    pub commands: CommandPalette,
    pub waveform: WaveformView,
    pub wave_snap: Snap,
    pub wave_beats: bool,
    pub wave_height: f32,
    pub arrangement_timeline: Timeline,
    pub piano_roll: Option<PianoRoll>,
    pub sequence_tool: Option<sequencer::Tool>,
    pub sample_tool: Option<sample::Tool>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            page: Page::default(),
            studio_view: StudioView::default(),
            commands: CommandPalette::default(),
            waveform: WaveformView::default(),
            wave_snap: Snap::default(),
            wave_beats: false,
            wave_height: 118.0,
            arrangement_timeline: Timeline::default(),
            piano_roll: None,
            sequence_tool: None,
            sample_tool: None,
        }
    }
}
