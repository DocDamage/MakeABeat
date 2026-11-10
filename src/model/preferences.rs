use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub zoom: f32,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    pub show_pads: bool,
    pub stereo_waveform: bool,
    pub crate_sort: usize,
    pub pads_width: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            high_contrast: false,
            reduce_motion: false,
            show_pads: true,
            stereo_waveform: false,
            crate_sort: 0,
            pads_width: 410.0,
        }
    }
}
