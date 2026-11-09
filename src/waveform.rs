use crate::audio::Clip;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Snap {
    #[default]
    Off,
    ZeroCrossing,
    Transient,
    Beat,
}

impl Snap {
    pub const ALL: [Self; 4] = [Self::Off, Self::ZeroCrossing, Self::Transient, Self::Beat];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::ZeroCrossing => "Zero crossing",
            Self::Transient => "Transient",
            Self::Beat => "Beat",
        }
    }

    pub fn time(self, clip: &Clip, time: f64, bpm: u16) -> f64 {
        let duration = clip.duration();
        match self {
            Self::Off => time,
            Self::Beat => {
                let beat = 60.0 / f64::from(bpm);
                (time / beat).round() * beat
            }
            Self::ZeroCrossing => {
                let rate = f64::from(clip.rate);
                let center = (time * rate).round() as usize;
                let radius = (rate * 0.008).ceil() as usize;
                let start = center.saturating_sub(radius).max(1);
                let end = center.saturating_add(radius).min(clip.frames.len().saturating_sub(1));
                (start..=end).filter(|&index| {
                    let mono = |frame: [f32; 2]| frame[0] + frame[1];
                    let before = mono(clip.frames[index - 1]);
                    let after = mono(clip.frames[index]);
                    before == 0.0 || after == 0.0 || before.is_sign_negative() != after.is_sign_negative()
                }).min_by_key(|index| index.abs_diff(center))
                    .map_or(time, |index| index as f64 / rate)
            }
            Self::Transient => transient_near(clip, time, 0.06),
        }.clamp(0.0, duration)
    }
}

pub fn transient_near(clip: &Clip, time: f64, radius: f64) -> f64 {
    let hop = 0.003;
    let steps = (radius / hop).floor() as i32;
    let mut best = (0.035, time);
    for offset in -steps..=steps {
        let at = (time + f64::from(offset) * hop).clamp(0.0, clip.duration());
        let before = clip.peak_between((at - 0.006).max(0.0), at);
        let after = clip.peak_between(at, at + 0.006);
        let score = (after - before) / (1.0 + f64::from(offset.abs()) as f32 * 0.035);
        if score > best.0 {
            best = (score, at);
        }
    }
    best.1
}

#[derive(Clone, Copy)]
pub struct WaveformView {
    pub start: f64,
    pub zoom: f64,
}

impl Default for WaveformView {
    fn default() -> Self {
        Self {
            start: 0.0,
            zoom: 1.0,
        }
    }
}

impl WaveformView {
    pub const MAX_ZOOM: f64 = 512.0;

    pub fn range(self, duration: f64) -> (f64, f64) {
        let span = duration / self.zoom;
        let start = self.start.clamp(0.0, duration - span);
        (start, start + span)
    }

    pub fn zoom_by(&mut self, factor: f64, anchor: f64, duration: f64) {
        let (start, end) = self.range(duration);
        let span = end - start;
        let anchor = if (start..=end).contains(&anchor) {
            anchor
        } else {
            (start + end) * 0.5
        };
        let fraction = (anchor - start) / span;
        self.zoom = (self.zoom * factor).clamp(1.0, Self::MAX_ZOOM);
        self.start =
            (anchor - fraction * duration / self.zoom).clamp(0.0, duration - duration / self.zoom);
    }

    pub fn center_on(&mut self, time: f64, duration: f64) {
        let span = duration / self.zoom;
        self.start = (time - span * 0.5).clamp(0.0, duration - span);
    }

    pub fn fit(&mut self, (start, end): (f64, f64), duration: f64) {
        self.zoom = (duration / ((end - start) * 1.22)).clamp(1.0, Self::MAX_ZOOM);
        self.center_on((start + end) * 0.5, duration);
    }

    pub fn reveal(&mut self, (start, end): (f64, f64), duration: f64) {
        let (left, right) = self.range(duration);
        if start >= left && end <= right {
            return;
        }
        if end - start > right - left {
            self.fit((start, end), duration);
        } else {
            self.center_on((start + end) * 0.5, duration);
        }
    }
}
