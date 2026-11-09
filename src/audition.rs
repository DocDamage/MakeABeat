use crate::{
    audio::{Clip, Frame},
    files::Result,
    model::beat_map::BeatMap,
    stretch::{self, Marker},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

pub const MAX_BARS: usize = 8;
const MAX_FIT_FRAMES: usize = 2_400_000;

#[derive(Clone, PartialEq)]
struct Key {
    fingerprint: String,
    range: (f64, f64),
    map: Option<Arc<BeatMap>>,
    bpm: u16,
    manual_bars: Option<usize>,
}

pub struct Prepared {
    pub clip: Arc<Clip>,
    pub range: (f64, f64),
    pub fitted: Option<Arc<Clip>>,
    pub bars: Option<usize>,
    pub mapped: bool,
    pub status: Option<String>,
}

#[derive(Default)]
pub struct Cache {
    fits: VecDeque<(Key, Arc<Clip>)>,
    fingerprint: Option<(Weak<[Frame]>, String)>,
}

impl Cache {
    pub fn prepare(
        &mut self,
        clip: Arc<Clip>,
        selection: (f64, f64),
        map: Option<Arc<BeatMap>>,
        bpm: Option<u16>,
        manual_bars: Option<usize>,
    ) -> Result<Prepared> {
        let range = (
            selection.0.clamp(0.0, clip.duration()),
            selection.1.clamp(0.0, clip.duration()),
        );
        if clip.region(range).is_none() {
            return Err("Choose a non-empty passage to audition.".into());
        }
        let grid = map
            .as_deref()
            .and_then(|map| crate::groove::loop_beats(map, range, MAX_BARS));
        let mut prepared = Prepared {
            clip,
            range,
            fitted: None,
            bars: None,
            mapped: grid.is_some(),
            status: None,
        };
        if let Some(beats) = grid {
            prepared.range = (beats[0].seconds, beats[beats.len() - 1].seconds);
        }
        let Some(bpm) = bpm else {
            prepared.status =
                Some("Following external MIDI clock at original sample speed.".into());
            return Ok(prepared);
        };
        let (bars, markers) = if let Some(beats) = grid {
            let bars = (beats.len() - 1) / 4;
            let markers = beats[1..beats.len() - 1]
                .iter()
                .enumerate()
                .map(|(i, beat)| Marker {
                    source_seconds: beat.seconds,
                    target_beat: (i + 1) as f64,
                })
                .collect::<Vec<_>>();
            (bars, markers)
        } else if let Some(bars) = manual_bars {
            (bars.clamp(1, MAX_BARS), Vec::new())
        } else {
            prepared.status =
                Some("Choose how many bars the cut contains to fit it to the beat.".into());
            return Ok(prepared);
        };
        prepared.bars = Some(bars);
        let frames = Arc::downgrade(&prepared.clip.frames);
        if self
            .fingerprint
            .as_ref()
            .is_none_or(|(previous, _)| !Weak::ptr_eq(previous, &frames))
        {
            self.fingerprint = Some((frames, crate::analysis_worker::fingerprint(&prepared.clip)?));
        }
        let key = Key {
            fingerprint: self.fingerprint.as_ref().unwrap().1.clone(),
            range: prepared.range,
            map,
            bpm,
            manual_bars,
        };
        if let Some(index) = self.fits.iter().position(|(cached, _)| cached == &key) {
            let entry = self.fits.remove(index).unwrap();
            prepared.fitted = Some(entry.1.clone());
            self.fits.push_front(entry);
            return Ok(prepared);
        }
        let seconds = bars as f64 * 240.0 / f64::from(bpm);
        match stretch::fit_warp(
            &prepared.clip,
            prepared.range,
            seconds,
            f64::from(bpm),
            &markers,
            MAX_FIT_FRAMES,
        ) {
            Ok(clip) => {
                let clip = Arc::new(clip);
                prepared.fitted = Some(clip.clone());
                self.fits.push_front((key, clip));
                self.fits.truncate(4);
            }
            Err(error) => {
                crate::background::check()?;
                prepared.status = Some(format!("Original speed available. {error}"));
            }
        }
        Ok(prepared)
    }
}

pub struct Voice {
    pub serial: u64,
    pub clip: Arc<Clip>,
    pub range: (f64, f64),
    position: f64,
    bars: Option<usize>,
    bars_left: usize,
}

impl Voice {
    pub fn new(serial: u64, clip: Arc<Clip>, range: (f64, f64), bars: Option<usize>) -> Self {
        Self {
            serial,
            clip,
            range,
            position: range.0,
            bars,
            bars_left: bars.unwrap_or(0),
        }
    }

    pub fn is_fitted(&self) -> bool {
        self.bars.is_some()
    }

    pub fn on_bar(&mut self) {
        if let Some(bars) = self.bars {
            self.bars_left -= 1;
            if self.bars_left == 0 {
                self.position = self.range.0;
                self.bars_left = bars;
            }
        }
    }

    pub fn next(&mut self, rate: f64, gain: f32) -> Frame {
        if self.bars.is_some() && self.position >= self.range.1 - 1e-9 {
            // The rounded render can end a frame before the transport downbeat.
            return [0.0; 2];
        }
        let frame = self.clip.read(self.position * f64::from(self.clip.rate));
        // A short fade at the end avoids a click and leaves the downbeat intact.
        let fade = ((self.range.1 - self.position) * 500.0).clamp(0.0, 1.0) as f32;
        self.position += 1.0 / rate;
        if self.bars.is_none() && self.position >= self.range.1 - 1e-9 {
            self.position = self.range.0
                + (self.position - self.range.1).max(0.0) % (self.range.1 - self.range.0);
        }
        frame.map(|sample| sample * fade * gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::beat_map::Beat;

    #[test]
    fn fits_a_corrected_downbeat_and_reuses_only_matching_renders() -> Result<()> {
        let clip = Arc::new(Clip::new(8_000, vec![[0.15; 2]; 24_000]));
        let mut map = BeatMap::default();
        map.beats
            .extend([0.12, 0.3, 0.8, 1.31, 1.8, 2.3].map(Beat::manual));
        map.set_downbeat(1);
        let map = Arc::new(map);
        let mut cache = Cache::default();
        let first = cache.prepare(
            clip.clone(),
            (0.0, 2.31),
            Some(map.clone()),
            Some(120),
            None,
        )?;
        assert!(first.mapped);
        assert_eq!(first.range, (0.3, 2.3));
        let rendered = first.fitted.unwrap();
        assert_eq!(rendered.duration(), 2.0);
        let same_content = Arc::new(Clip::new(8_000, clip.frames.to_vec()));
        let again = cache.prepare(
            same_content,
            (0.0, 2.31),
            Some(map.clone()),
            Some(120),
            None,
        )?;
        assert!(Arc::ptr_eq(&rendered, &again.fitted.unwrap()));
        let faster = cache.prepare(
            clip.clone(),
            (0.0, 2.31),
            Some(map.clone()),
            Some(150),
            None,
        )?;
        assert_eq!(faster.fitted.unwrap().duration(), 1.6);
        let mut revised = (*map).clone();
        revised.move_beat(2, 0.81, clip.duration());
        let changed = cache.prepare(
            clip.clone(),
            (0.0, 2.31),
            Some(Arc::new(revised)),
            Some(120),
            None,
        )?;
        assert!(!Arc::ptr_eq(&rendered, &changed.fitted.unwrap()));
        let unsupported = cache.prepare(clip.clone(), (0.0, 2.31), Some(map), Some(40), None)?;
        assert!(unsupported.fitted.is_none());
        assert!(
            unsupported
                .status
                .unwrap()
                .contains("Original speed available")
        );
        let manual = cache.prepare(clip.clone(), (0.0, 2.0), None, Some(120), None)?;
        assert!(manual.fitted.is_none());
        let manual = cache.prepare(clip, (0.0, 2.0), None, Some(120), Some(1))?;
        assert_eq!(manual.fitted.unwrap().duration(), 2.0);
        Ok(())
    }
}
