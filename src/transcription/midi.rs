use crate::{
    files::Result,
    model::melody::{BEND_RANGE, Expression, cents_at},
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    PerNote,
    Quantized,
}

pub const CHANNELS: [u8; 13] = [2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15];
const RANGE_CC: [(u8, u8); 6] = [(101, 0), (100, 0), (6, 12), (38, 0), (101, 127), (100, 127)];

pub type Event = (u32, u8, [u8; 3]);

pub struct Note {
    pub tick: u32,
    pub end: u32,
    pub pitch: u8,
    pub velocity: u8,
    pub expression: Arc<Expression>,
}

fn wheel(cents: f64) -> u16 {
    (8192.0 + cents * (8192.0 / f64::from(BEND_RANGE)))
        .round()
        .clamp(0.0, 16_383.0) as u16
}

fn bend(channel: u8, value: u16) -> [u8; 3] {
    [0xe0 | channel, (value & 127) as u8, (value >> 7) as u8]
}

pub fn append(events: &mut Vec<Event>, mut notes: Vec<Note>) -> Result<()> {
    notes.sort_by_key(|note| (note.tick, note.pitch, note.end));
    let mut available = [0_u32; CHANNELS.len()];
    let mut configured = [false; CHANNELS.len()];
    for note in notes {
        crate::background::check()?;
        let Some(slot) = available.iter().position(|&end| end <= note.tick) else {
            return Err("More than 13 bent notes overlap. Export MIDI with Quantized pitch, or render WAV to keep the bends.".into());
        };
        let channel = CHANNELS[slot];
        available[slot] = note.end;
        if !std::mem::replace(&mut configured[slot], true) {
            for (index, (controller, value)) in RANGE_CC.into_iter().enumerate() {
                events.push((0, 2 + index as u8, [0xb0 | channel, controller, value]));
            }
        }
        let points = &note.expression.bends;
        let length = note.end - note.tick;
        let mut last = wheel(cents_at(points, 0.0));
        events.push((note.tick, 8, bend(channel, last)));
        events.push((note.tick, 10, [0x90 | channel, note.pitch, note.velocity]));
        let mut start = 0;
        for end in points
            .iter()
            .map(|point| point.tick)
            .filter(|&tick| tick > 0)
            .take_while(|&tick| tick < length)
            .chain(std::iter::once(length.saturating_sub(1)))
        {
            if end <= start {
                continue;
            }
            let target = wheel(cents_at(points, f64::from(end)));
            if target != last {
                for offset in (start + 12..end).step_by(12).chain(std::iter::once(end)) {
                    let value = wheel(cents_at(points, f64::from(offset)));
                    if value != last {
                        events.push((note.tick + offset, 8, bend(channel, value)));
                        last = value;
                    }
                }
            }
            start = end;
        }
        events.push((note.end, 0, [0x80 | channel, note.pitch, 0]));
        events.push((note.end, 1, bend(channel, 8192)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::tests::note;
    use crate::{
        audio::RENDER_RATE,
        files,
        model::{Studio, TICKS_PER_BAR},
    };

    fn packets(file: &[u8]) -> Vec<(u32, [u8; 3])> {
        let mut reader = file[29..].iter().copied();
        let mut output = Vec::new();
        let mut tick = 0;
        loop {
            let mut delta = 0;
            loop {
                let byte = reader.next().unwrap();
                delta = (delta << 7) + u32::from(byte & 0x7f);
                if byte < 128 {
                    break;
                }
            }
            tick += delta;
            let status = reader.next().unwrap();
            if status == 0xff {
                break;
            }
            output.push((
                tick,
                [status, reader.next().unwrap(), reader.next().unwrap()],
            ));
        }
        output
    }

    #[test]
    fn exports_channel_isolated_bends_with_setup_resets_and_a_capacity_fallback() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("notes.mid");
        let mut studio = Studio {
            bpm: 120,
            ..Studio::default()
        };
        studio.performance = vec![
            note(0, 240, 60, 1200),
            note(120, 200, 60, -600),
            note(240, 100, 65, 33),
            note(400, 55, 67, 0),
        ];
        files::export_midi_mode(&path, &studio, Mode::PerNote)?;
        if let Some(destination) = std::env::var_os("LOST_SAMPLES_PITCH_MIDI_CHECK") {
            std::fs::copy(&path, destination)?;
        }
        let midi = packets(&std::fs::read(&path)?);
        let start: Vec<_> = midi
            .iter()
            .filter(|(tick, packet)| *tick == 0 && packet[0] == 0xb2)
            .map(|(_, packet)| (packet[1], packet[2]))
            .collect();
        assert_eq!(start, RANGE_CC);
        assert!(midi.contains(&(0, [0x92, 60, 100])));
        assert!(midi.contains(&(120, [0x93, 60, 100])));
        let transition: Vec<_> = midi
            .iter()
            .filter(|(tick, packet)| *tick == 240 && packet[0] & 15 == 2)
            .map(|(_, packet)| *packet)
            .collect();
        assert_eq!(
            transition,
            [
                [0x82, 60, 0],
                bend(2, 8192),
                bend(2, wheel(33.0)),
                [0x92, 65, 100]
            ]
        );
        assert!(midi.contains(&(400, [0x91, 67, 100])));
        studio.performance = (48..62)
            .map(|key| note(0, TICKS_PER_BAR / 2, key, 100))
            .collect();
        assert!(
            files::export_midi_mode(&path, &studio, Mode::PerNote)
                .unwrap_err()
                .to_string()
                .contains("13 bent")
        );
        assert_eq!(packets(&std::fs::read(&path)?), midi);
        files::export_midi_mode(&path, &studio, Mode::Quantized)?;
        let midi = packets(&std::fs::read(&path)?);
        assert!(
            midi.iter()
                .all(|(_, packet)| matches!(packet[0], 0x81 | 0x91))
        );
        assert!(midi.contains(&(0, [0x91, 49, 100])));
        Ok(())
    }

    #[test]
    fn persisted_bends_render_at_the_bent_pitch_without_source_audio() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("notes.flip");
        let wav = temp.path().join("notes.wav");
        let mut studio = Studio {
            bpm: 120,
            ..Studio::default()
        };
        studio.performance.push(note(0, 480, 69, 1200));
        let project = crate::model::project::Project {
            studio,
            ..crate::model::project::Project::default()
        };
        files::save_project(&path, &project, None)?;
        assert_eq!(&std::fs::read(&path)?[..8], b"LSFLIP08");
        let (restored, clip) = files::open_project(&path)?;
        assert!(restored == project && clip.is_none());
        files::export_wav(
            &wav,
            restored.studio,
            None,
            files::WavOptions {
                bits: 32,
                ..Default::default()
            },
        )?;
        let rendered = files::decode_path(&wav)?;
        let segment = &rendered.frames[(RENDER_RATE / 10) as usize..(RENDER_RATE / 4) as usize];
        let crossings = segment
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        assert!(
            (130..=133).contains(&crossings),
            "{crossings} crossings for 880 Hz"
        );
        assert_eq!(rendered.frames[(RENDER_RATE * 3 / 4) as usize], [0.0; 2]);
        Ok(())
    }
}
