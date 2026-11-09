use crate::{audio::Clip, files::Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub source_seconds: f64,
    pub target_beat: f64,
}

impl Marker {
    pub const MAX: usize = 32;
}


pub fn fit(clip: &Clip, range: (f64, f64), seconds: f64, limit: usize) -> Result<Clip> {
    fit_warp(clip, range, seconds, 60.0, &[], limit)
}

pub fn fit_warp(
    clip: &Clip, range: (f64, f64), seconds: f64, bpm: f64, markers: &[Marker], limit: usize,
) -> Result<Clip> {
    let input = clip.region(range).ok_or("Select a non-empty range.")?;
    let rate = clip.rate as usize;
    let target = (seconds * rate as f64).round() as usize;
    let ratio = target as f64 / input.len() as f64;
    let window = rate * 40 / 1_000;
    if !(0.5..=2.0).contains(&ratio) || input.len().min(target) < window * 2 {
        return Err("Use a selection of at least 80 ms and fit it between half and twice its original duration.".into());
    }
    if target > limit {
        return Err("The stretched recording would exceed the project audio limit.".into());
    }
    let mut anchors = vec![(0, 0)];
    for marker in markers {
        if marker.source_seconds <= range.0 || marker.source_seconds >= range.1 {
            continue;
        }
        let source = ((marker.source_seconds - range.0) * rate as f64).round() as usize;
        let output = (marker.target_beat * 60.0 / bpm * rate as f64).round() as usize;
        if source == 0 || source >= input.len() || output == 0 || output >= target {
            return Err("Warp markers must be inside the source selection and target length.".into());
        }
        anchors.push((output, source));
    }
    anchors.sort_unstable();
    anchors.push((target, input.len()));
    for (index, pair) in anchors.windows(2).enumerate() {
        let span = pair[1].0 - pair[0].0;
        let input = pair[1].1.saturating_sub(pair[0].1);
        if span == 0 || input == 0 {
            return Err(format!("Warp segment {} has overlapping or reversed markers. Move a marker so both timelines advance.", index + 1).into());
        }
        let ratio = span as f64 / input as f64;
        if !(0.5..=2.0).contains(&ratio) {
            return Err(format!("Warp segment {} requires {ratio:.2}× stretch. Adjust its markers to stay between 0.5× and 2.0×.", index + 1).into());
        }
    }
    if target == input.len() && anchors.len() == 2 {
        return Ok(Clip::new(clip.rate, input.to_vec()));
    }

    let hop = window / 2;
    let search = rate * 10 / 1_000;
    let stride = (rate / 8_000).max(1);
    let last_input = input.len() - window;
    let last_output = target - window;
    let mut output = vec![[0.0; 2]; target];
    output[..window].copy_from_slice(&input[..window]);
    let mut offset = hop;
    while offset < target {
        crate::background::check()?;
        let length = window.min(target - offset);
        let overlap = hop.min(length);
        let expected = if anchors.len() == 2 {
            (offset.min(last_output) as u64 * last_input as u64 / last_output as u64) as usize
        } else {
            let center = (offset + hop).min(target - 1);
            let right = anchors.partition_point(|&(out, _)| out <= center);
            let (from, source) = anchors[right - 1];
            let (to, end) = anchors[right];
            let mapped = source + ((center - from) as u64 * (end - source) as u64 / (to - from) as u64) as usize;
            mapped.saturating_sub(hop).min(last_input)
        };
        let begin = expected.saturating_sub(search);
        let end = (expected + search).min(last_input);
        let mut best = (f64::NEG_INFINITY, expected);
        // Match the existing overlap before crossfading the next grain. Grain samples
        // play at their original rate, so changing duration does not resample pitch.
        for start in (begin..=end).step_by(stride) {
            let mut cross = 0.0_f64;
            let mut existing_power = 0.0_f64;
            let mut new_power = 0.0_f64;
            for i in (0..overlap).step_by(stride * 4) {
                for channel in 0..2 {
                    let existing = f64::from(output[offset + i][channel]);
                    let new = f64::from(input[start + i][channel]);
                    cross += existing * new;
                    existing_power += existing * existing;
                    new_power += new * new;
                }
            }
            let distance = start.abs_diff(expected) as f64 / search.max(1) as f64;
            let score = cross / (existing_power * new_power).sqrt().max(1e-12) - distance * 0.05;
            if score > best.0 {
                best = (score, start);
            }
        }
        for i in 0..length {
            let weight = (i as f32 / overlap.max(1) as f32).min(1.0);
            let new = input[best.1 + i];
            for (sample, new) in output[offset + i].iter_mut().zip(new) {
                *sample = *sample * (1.0 - weight) + new * weight;
            }
        }
        offset += hop;
    }
    Ok(Clip::new(clip.rate, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_sustained_voices_chords_and_drifting_drums_remain_audible() -> Result<()> {
        let rate = 8_000;
        let tone = |frequencies: &[f64], amplitudes: &[f64]| {
            Clip::new(rate, (0..2 * rate).map(|frame| {
                let at = f64::from(frame) / f64::from(rate);
                let sample = frequencies.iter().zip(amplitudes).map(|(hz, level)| {
                    (at * std::f64::consts::TAU * hz).sin() * level
                }).sum::<f64>() as f32;
                [sample; 2]
            }).collect())
        };
        let power = |clip: &Clip, hz: f64| {
            let [a, b] = clip.frames[1600..6400].iter().enumerate().fold([0.0; 2], |sum, (frame, sample)| {
                let phase = std::f64::consts::TAU * hz * frame as f64 / f64::from(rate);
                [sum[0] + f64::from(sample[0]) * phase.cos(), sum[1] + f64::from(sample[0]) * phase.sin()]
            });
            a * a + b * b
        };
        let voiced = fit(&tone(&[180.0, 360.0, 540.0], &[0.45, 0.21, 0.12]), (0.0, 2.0), 1.6, 24_000)?;
        let chord = fit(&tone(&[220.0, 277.0, 330.0], &[0.24; 3]), (0.0, 2.0), 2.4, 24_000)?;
        for (clip, frequencies) in [(&voiced, &[180.0, 360.0, 540.0][..]), (&chord, &[220.0, 277.0, 330.0][..])] {
            for &frequency in frequencies {
                assert!(power(clip, frequency) > power(clip, frequency + 20.0) * 2.0,
                    "pitch changed near {frequency} Hz");
            }
        }
        let pulses = [0.0, 0.46, 0.96, 1.51];
        let drums = Clip::new(rate, (0..16_800).map(|frame| {
            let at = f64::from(frame) / f64::from(rate);
            let sample = pulses.iter().filter_map(|onset| {
                let dt = at - onset;
                (0.0..0.04).contains(&dt).then(|| (-75.0 * dt).exp() * (dt * 600.0).cos())
            }).sum::<f64>() as f32 * 0.7;
            [sample; 2]
        }).collect());
        let markers = [
            Marker { source_seconds: 0.46, target_beat: 1.0 },
            Marker { source_seconds: 0.96, target_beat: 2.0 },
            Marker { source_seconds: 1.51, target_beat: 3.0 },
        ];
        let warped = fit_warp(&drums, (0.0, 2.1), 2.0, 120.0, &markers, 24_000)?;
        assert_eq!(warped.frames.len(), 16_000);
        for at in [0.0_f64, 0.5, 1.0, 1.5] {
            assert!(warped.peak_between((at - 0.08).max(0.0), at + 0.08) > 0.1,
                "lost the hit near {at} seconds");
        }
        Ok(())
    }

    #[test]
    fn stretch_changes_duration_without_halving_pitch() -> Result<()> {
        let rate = 8_000;
        let input = Clip::new(
            rate,
            (0..rate)
                .map(|frame| {
                    let phase = std::f32::consts::TAU * 250.0 * frame as f32 / rate as f32;
                    [phase.sin() * 0.5; 2]
                })
                .collect(),
        );
        let output = fit(&input, (0.0, 1.0), 1.8, 20_000)?;
        assert_eq!(output.frames.len(), 14_400);
        let crossings = output.frames[800..8_800]
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        assert!(
            (245..=255).contains(&crossings),
            "Detected {crossings} cycles"
        );
        Ok(())
    }
}
