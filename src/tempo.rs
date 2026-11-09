use crate::audio::Clip;
use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub bpm: u16,
    pub strength: u8,
}

pub fn beat_grid(clip: &Clip, bpm_hint: u16) -> crate::files::Result<Vec<crate::model::beat_map::Beat>> {
    use crate::model::beat_map::{Beat, MAX_BEATS};
    let duration = clip.duration();
    if duration < 0.2 || clip.peak_between(0.0, duration) < 0.001 {
        return Err("No beat evidence in this selection. Add manual beats or select a clearer passage.".into());
    }
    crate::background::stage("Estimating the pulse")?;
    let bpm = estimate(clip, None).map_or(bpm_hint.clamp(40, 240), |estimate| estimate.bpm);
    let period = 60.0 / f64::from(bpm);
    let attack = |at: f64| {
        let before = clip.peak_between((at - 0.016).max(0.0), at);
        let after = clip.peak_between(at, (at + 0.012).min(duration));
        (after - before).max(0.0)
    };
    let lookahead = (duration / period).floor().clamp(1.0, 14.0) as usize;
    let steps = (period / 0.005).ceil() as usize;
    let mut phase = 0.0;
    let mut best = 0.0_f32;
    for index in 0..steps {
        let at = index as f64 * 0.005;
        let score = (0..lookahead).map(|beat| {
            let expected = at + period * beat as f64;
            [-0.015, 0.0, 0.015].into_iter().map(|offset| attack((expected + offset).clamp(0.0, duration)))
                .fold(0.0_f32, f32::max)
        }).sum::<f32>();
        if score > best {
            phase = at;
            best = score;
        }
    }
    let mut beats = Vec::<Beat>::new();
    let mut expected = phase;
    let mut interval = period;
    while expected < duration - 0.015 && beats.len() < MAX_BEATS {
        crate::background::check()?;
        let start = beats.last().map_or(0.0, |beat| beat.seconds + period * 0.55);
        let refined = crate::waveform::transient_near(clip, expected, interval * 0.18)
            .clamp(start.min(duration), duration);
        let confidence = (attack(refined) * 3.0).clamp(0.0, 1.0);
        if let Some(previous) = beats.last()
            && confidence > 0.12 && previous.confidence.is_some_and(|confidence| confidence > 0.12)
        {
            interval = (0.72 * interval + 0.28 * (refined - previous.seconds))
                .clamp(period * 0.75, period * 1.25);
        }
        beats.push(Beat {
            seconds: refined, downbeat: beats.len().is_multiple_of(4), anchor: false,
            confidence: Some(confidence),
        });
        expected = refined + interval;
    }
    Ok(beats)
}

pub fn estimate(clip: &Clip, selection: Option<(f64, f64)>) -> Option<Estimate> {
    let rate = clip.rate as usize;
    let source = selection
        .and_then(|range| clip.region(range))
        .filter(|frames| frames.len() >= 6 * rate)
        .unwrap_or(&clip.frames);
    if source.len() < 6 * rate {
        return None;
    }
    let source = &source[..source.len().min(60 * rate)];
    let hop = (rate / 200).max(1);
    let envelope_rate = rate as f64 / hop as f64;
    let low_pass = 1.0 - (-TAU * 180.0 / rate as f64).exp();
    let mut low = [0.0; 2];
    let mut previous = [0.0; 2];
    let mut onsets = Vec::with_capacity(source.len() / hop);
    for (index, block) in source.chunks_exact(hop).enumerate() {
        if index.is_multiple_of(64) {
            crate::background::progress(index, source.len() / hop).ok()?;
        }
        let mut energy = [0.0; 2];
        for frame in block {
            for channel in 0..2 {
                let sample = f64::from(frame[channel]);
                low[channel] += low_pass * (sample - low[channel]);
                energy[0] += low[channel].powi(2);
                energy[1] += (sample - low[channel]).powi(2);
            }
        }
        let levels = energy.map(|power| (32.0 * (power / (2 * hop) as f64).sqrt()).ln_1p());
        let novelty = (levels[0] - previous[0]).max(0.0) + (levels[1] - previous[1]).max(0.0);
        onsets.push(novelty);
        previous = levels;
    }

    // Discard the leading transient and measure repeated onsets on a zero-mean envelope.
    let onsets = &mut onsets[(envelope_rate / 5.0) as usize..];
    let mean = onsets.iter().sum::<f64>() / onsets.len() as f64;
    if mean < 0.0001 {
        return None;
    }
    for value in onsets.iter_mut() {
        *value -= mean;
    }
    let min_lag = (60.0 * envelope_rate / 180.0).round() as usize;
    let max_lag = envelope_rate.round() as usize;
    let correlation = |lag: usize| {
        let (cross, left, right) = onsets
            .iter()
            .zip(&onsets[lag..])
            .fold((0.0, 0.0, 0.0), |(cross, left, right), (a, b)| {
                (cross + a * b, left + a * a, right + b * b)
            });
        cross / (left * right).sqrt().max(1e-12)
    };
    let mut best_lag = min_lag;
    let mut best_score = 0.0;
    for lag in min_lag..=max_lag {
        // Prefer the nearer repeat when a beat and its double score almost equally.
        let score = correlation(lag) * (1.0 - 0.12 * lag as f64 / max_lag as f64);
        if score > best_score {
            best_lag = lag;
            best_score = score;
        }
    }
    if best_score < 0.20 {
        return None;
    }
    let [left, middle, right] =
        [best_lag.saturating_sub(1), best_lag, best_lag + 1].map(correlation);
    let curvature = left + right - 2.0 * middle;
    let correction = if curvature < -1e-8 {
        (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let bpm = (60.0 * envelope_rate / (best_lag as f64 + correction)).round();
    Some(Estimate {
        bpm: bpm.clamp(60.0, 180.0) as u16,
        strength: (middle.clamp(0.0, 1.0) * 100.0).round() as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_steady_clicks_and_rejects_silence() {
        let rate = 8_000;
        let seconds = 16;
        let clicks = |bpm: f64| {
            (0..rate * seconds)
                .map(|frame| {
                    let beat = (f64::from(frame) * bpm / (60.0 * f64::from(rate))).fract();
                    let value = if beat < 0.02 { 0.75 } else { 0.0 };
                    [value; 2]
                })
                .collect()
        };
        assert_eq!(estimate(&Clip::new(rate, clicks(92.0)), None).map(|estimate| estimate.bpm), Some(92));
        assert_eq!(
            estimate(&Clip::new(rate, clicks(120.0)), Some((4.0, 12.0))).map(|estimate| estimate.bpm),
            Some(120)
        );
        let silence = Clip::new(rate, vec![[0.0; 2]; (rate * seconds) as usize]);
        assert_eq!(estimate(&silence, None), None);
    }
}
