use crate::{
    audio::Clip,
    files::Result,
    model::{
        STEPS, TICKS_PER_STEP,
        beat_map::{Beat, BeatMap},
        groove::{MAX_BARS, MAX_OFFSET, Provenance, Slot, Template},
    },
};

/// Select complete four-beat bars inside the cut, ignoring a pickup.
pub fn loop_beats(map: &BeatMap, range: (f64, f64), limit: usize) -> Option<&[Beat]> {
    let start = map
        .beats
        .iter()
        .position(|beat| beat.downbeat && beat.seconds >= range.0 - 0.001)?;
    let count = map.beats[start..].partition_point(|beat| beat.seconds <= range.1 + 0.001);
    let bars = (count.saturating_sub(1) / 4).min(limit);
    (bars > 0).then(|| &map.beats[start..=start + bars * 4])
}

pub fn extract(clip: &Clip, beats: &[Beat], title: String) -> Result<Template> {
    let bars = beats.len().saturating_sub(1) / 4;
    if !(1..=MAX_BARS).contains(&bars) || beats.len() != bars * 4 + 1 {
        return Err("Select a complete bar between downbeats in the beat map.".into());
    }
    let range = (beats[0].seconds, beats[beats.len() - 1].seconds);
    let straight_step = (range.1 - range.0) / (bars * STEPS) as f64;
    let mut evidence = Vec::with_capacity(bars * STEPS);
    for step in 0..bars * STEPS {
        let quarter = step / 4;
        let division = step % 4;
        let interval = beats[quarter + 1].seconds - beats[quarter].seconds;
        let mapped = beats[quarter].seconds + interval * division as f64 / 4.0;
        let straight = range.0 + step as f64 * straight_step;
        let found = onset(clip, mapped, (interval * 0.105).min(0.080));
        let anchor = if division == 0 {
            Some(mapped)
        } else {
            found.map(|(at, _)| at)
        };
        let offset = anchor.map_or(0, |at| {
            ((at - straight) / straight_step * f64::from(TICKS_PER_STEP))
                .round()
                .clamp(-f64::from(MAX_OFFSET), f64::from(MAX_OFFSET)) as i8
        });
        evidence.push((offset, found.map(|(_, peak)| peak)));
    }
    let (sum, count) = evidence
        .iter()
        .filter_map(|(_, peak)| *peak)
        .fold((0.0, 0), |(sum, count), peak| (sum + peak, count + 1));
    let mean = sum / count.max(1) as f32;
    let slots = evidence
        .into_iter()
        .map(|(offset, peak)| Slot {
            offset,
            accent: peak.map_or(1.0, |value| (value / mean).min(4.0)),
        })
        .collect();
    Ok(Template {
        name: "Extracted groove".into(),
        bars,
        slots,
        source: Provenance {
            title,
            range,
            fingerprint: crate::analysis_worker::fingerprint(clip)?,
        },
    })
}

fn onset(clip: &Clip, center: f64, radius: f64) -> Option<(f64, f32)> {
    const HOP: f64 = 0.001;
    let steps = (radius / HOP) as i32;
    let mut best = None;
    for step in -steps..=steps {
        let at = center + f64::from(step) * HOP;
        if at < 0.0 || at >= clip.duration() {
            continue;
        }
        let after = clip.peak_between(at, at + HOP);
        let before = clip.peak_between((at - 0.004).max(0.0), at);
        let score = (after - before) / (1.0 + f32::from(step.unsigned_abs() as u16) * 0.012);
        if score >= 0.025 && best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((at, score));
        }
    }
    best.map(|(at, _)| (at, clip.peak_between(at, at + 0.016)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ChopInput, PlayedChop, Studio, drum_velocity, events::Cell, groove::Transfer,
    };

    #[test]
    fn extracts_corrected_beat_and_subdivision_timing_and_relative_accents() {
        let rate = 48_000;
        let mut frames = vec![[0.0; 2]; rate * 3];
        let offsets = [0, 13, -11, 0, 19, 0, -23, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let weights = [
            1.0, 1.5, 0.5, 1.0, 1.0, 1.0, 0.5, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];
        let seconds =
            |step: usize, offset: i32| 0.25 + (step as f64 + f64::from(offset) / 120.0) * 0.125;
        for (step, (&offset, &weight)) in offsets.iter().zip(&weights).enumerate() {
            let at = (seconds(step, offset) * rate as f64).round() as usize;
            frames[at..at + 16].fill([weight * 0.4; 2]);
        }
        let beats = [0, 4, 8, 12, 16]
            .map(|step| Beat::manual(seconds(step, offsets.get(step).copied().unwrap_or(0))));
        let template =
            extract(&Clip::new(rate as u32, frames), &beats, "Test break".into()).unwrap();
        for ((slot, offset), weight) in template.slots.iter().zip(offsets).zip(weights) {
            assert!(
                (i32::from(slot.offset) - offset).abs() <= 2,
                "{slot:?} vs {offset}"
            );
            assert!((slot.accent - weight).abs() <= 0.04, "{slot:?} vs {weight}");
        }
    }

    #[test]
    fn transfers_only_selected_lanes_and_uses_swing_once() {
        let mut studio = Studio {
            swing: 0.5,
            ..Studio::default()
        };
        studio.chops[0][1] = Some(0);
        studio.hits[0][0][1] = 1;
        studio.hits[0][1][1] = 1;
        studio.lane_offsets[2] = -3;
        studio.performance.push(PlayedChop {
            tick: 124,
            input: ChopInput::Pad(0),
            velocity: 80,
            id: Default::default(),
            mode: crate::audio::PadMode::Gate,
            length: Some(200),
            sound: Default::default(),
            expression: None,
        });
        let original = studio.active_sequence();
        let mut template = Template {
            name: "Test".into(),
            bars: 1,
            slots: vec![
                Slot {
                    offset: 0,
                    accent: 1.0
                };
                STEPS
            ],
            source: Provenance {
                title: "Break".into(),
                range: (0.0, 2.0),
                fingerprint: "test".into(),
            },
        };
        template.slots[1] = Slot {
            offset: -18,
            accent: 0.5,
        };
        let mut transfer = Transfer {
            timing: 0,
            velocity: 0,
            recorded: true,
            lanes: [true; crate::model::events::LANES],
        };
        assert_eq!(transfer.bake(original.clone(), &template), original);
        transfer.timing = 100;
        transfer.velocity = 100;
        transfer.lanes.fill(false);
        transfer.lanes[0] = true;
        transfer.lanes[2] = true;
        transfer.recorded = false;
        let baked = transfer.bake(original.clone(), &template);
        let bar = &baked.bars[0];
        assert_eq!(bar.drums[1], original.bars[0].drums[1]);
        assert_eq!(drum_velocity(bar.drums[0][1]), 45);
        assert_eq!(baked.performance, original.performance);
        let column = Cell::column(&bar.drums, bar.pads[1], bar.keys[1], bar.metadata(), 1);
        let drum = column[2]
            .unwrap()
            .resolve(&studio, 2, 1, 0, false)
            .next()
            .unwrap();
        assert_eq!((drum.tick, drum.velocity), (129, 45));
        transfer.velocity = 0;
        let timing_only = transfer.bake(original.clone(), &template);
        assert_eq!(timing_only.bars[0].drums, original.bars[0].drums);
        transfer.velocity = 100;
        transfer.timing = 0;
        assert!(
            transfer.bake(original.clone(), &template).bars[0]
                .feel
                .values()
                .all(|hit| hit.offset == 0)
        );
        transfer.timing = 100;
        transfer.recorded = true;
        let recorded = &transfer.bake(original.clone(), &template).performance[0];
        assert_eq!(
            (recorded.tick, recorded.velocity, recorded.length),
            (106, 40, Some(200))
        );
    }
}
