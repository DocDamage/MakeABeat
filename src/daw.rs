use crate::{
    audio::Clip,
    files::{self, RenderPart, Result},
    model::{ChopInput, DRUMS, NamedSample, PIANO_HIGH, PIANO_LOW, STEPS, Studio, piano_note},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Delivery {
    pub written: Vec<PathBuf>,
    pub failed: Vec<String>,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub mix: bool,
    pub sample_bus: bool,
    pub drums_bus: bool,
    pub individual_drums: bool,
    pub separate_chops: bool,
    pub midi: bool,
    pub midi_mode: crate::transcription::midi::Mode,
    pub raw: bool,
    pub processed: bool,
    pub dry_stems: bool,
    pub wav: files::WavOptions,
}

impl Default for Options {
    fn default() -> Self {
        Self { mix: true, sample_bus: true, drums_bus: true, individual_drums: true,
            separate_chops: false, midi: true, midi_mode: Default::default(), raw: true, processed: false, dry_stems: false,
            wav: Default::default() }
    }
}

#[cfg(test)]
pub fn export(
    targets: &[PathBuf],
    title: &str,
    studio: Studio,
    clip: Option<Arc<Clip>>,
    samples: &[NamedSample],
) -> Result<Delivery> {
    export_with(targets, title, studio, clip, samples, Options::default())
}

pub fn export_with(
    targets: &[PathBuf], title: &str, studio: Studio, clip: Option<Arc<Clip>>,
    samples: &[NamedSample], options: Options,
) -> Result<Delivery> {
    let staging = tempfile::Builder::new().prefix("Lost-Samples-").tempdir()?;
    let root = staging.path();
    let name = format!(
        "{} - {}",
        filename(title),
        staging
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Lost-Samples")
    );
    let sequence = Arc::new(
        studio
            .render_sequence()
            .ok_or("Reduce the song length before exporting.")?,
    );
    let has_hits = sequence.has_hits(&studio);
    let has_pattern = has_hits || studio.active_loop().is_some() || !studio.sections.is_empty();
    if has_pattern {
        if options.mix {
            files::render_wav_with(&root.join("01 Mix.wav"), studio.clone(), clip.clone(),
                sequence.clone(), RenderPart::Mix, options.wav)?;
        }
        let stems = root.join("Stems");
        if options.sample_bus || options.drums_bus || options.individual_drums || options.separate_chops {
            fs::create_dir(&stems)?;
        }
        let mut stem_studio = studio.clone();
        let mut stem_sequence = sequence.as_ref().clone();
        if options.dry_stems {
            stem_studio.delay = 0.0;
            stem_studio.reverb = 0.0;
            stem_studio.filter = 0.0;
            for automation in std::iter::once(&mut stem_sequence.automation)
                .chain(stem_sequence.section_automation.iter_mut().map(|(_, automation)| automation))
            {
                for target in [crate::model::AutomationTarget::LowPass, crate::model::AutomationTarget::Delay, crate::model::AutomationTarget::Reverb] {
                    automation.set_points(target, Vec::new());
                }
            }
        }
        let stem_sequence = Arc::new(stem_sequence);
        let render = |name: &str, part: RenderPart| {
            files::render_wav_with(&stems.join(name), stem_studio.clone(), clip.clone(),
                stem_sequence.clone(), part, options.wav)
        };
        if options.sample_bus { render("01 Sample.wav", RenderPart::Sample)?; }
        if options.drums_bus {
            files::render_wav_with(&root.join("02 Drums bus.wav"), stem_studio.clone(), clip.clone(),
                stem_sequence.clone(), RenderPart::Drums, options.wav)?;
        }
        if options.individual_drums {
            for (index, drum) in DRUMS.iter().enumerate() {
                if !sequence.bars.iter().any(|bar| bar.drums[index].iter().any(|&hit| hit != 0)) { continue; }
                render(&format!("{:02} {drum}.wav", index + 2), RenderPart::Drum(index))?;
            }
        }
        if options.separate_chops {
            if sequence.has_pad_steps(&studio) {
                render("Pad chops.wav", RenderPart::Pads)?;
            }
            if sequence.has_key_steps(&studio) {
                render("Keys.wav", RenderPart::Keys)?;
            }
        }
        if has_hits && options.midi {
            let midi = root.join("MIDI");
            fs::create_dir(&midi)?;
            let name = if studio.sections.is_empty() {
                "Full pattern.mid"
            } else {
                "Full song.mid"
            };
            files::export_midi_sequence_mode(&midi.join(name), &studio, &sequence, options.midi_mode)?;
            let mut drum_notes = sequence.as_ref().clone();
            for bar in &mut drum_notes.bars {
                bar.pads.fill(None);
                bar.keys.fill(None);
            }
            drum_notes.performance.clear();
            if drum_notes.has_hits(&studio) {
                files::export_midi_sequence_mode(&midi.join("Drums.mid"), &studio, &drum_notes, options.midi_mode)?;
            }
            let mut chops = sequence.as_ref().clone();
            for bar in &mut chops.bars {
                bar.drums = Default::default();
                bar.keys.fill(None);
            }
            chops
                .performance
                .retain(|event| matches!(event.input, ChopInput::Pad(_)));
            if chops.has_chops(&studio) {
                files::export_midi_sequence_mode(&midi.join("Chops.mid"), &studio, &chops, options.midi_mode)?;
            }
            if sequence.has_key_steps(&studio) {
                let mut keys = sequence.as_ref().clone();
                for bar in &mut keys.bars {
                    bar.drums = Default::default();
                    bar.pads.fill(None);
                }
                keys.performance
                    .retain(|event| matches!(event.input, ChopInput::Key(_)));
                files::export_midi_sequence_mode(&midi.join("Keys.mid"), &studio, &keys, options.midi_mode)?;
            }
        }
    }
    if options.raw && let Some(clip) = &clip {
        let cuts = root.join("Samples");
        fs::create_dir(&cuts)?;
        if let Some(range) = studio.loop_range()
            && let Some(source) = clip.source(studio.loop_source)
        {
            files::export_region(&cuts.join("Loop.wav"), source, range)?;
        }
        for index in 0..STEPS {
            if let Some(source) = clip.source(studio.pad_sources[index])
                && let Some(range) = studio.slice(index, source.duration())
            {
                files::export_region(&cuts.join(format!("Pad {:02}.wav", index + 1)), source, range)?;
            }
        }
        for (index, sample) in samples.iter().enumerate() {
            if let Some(source) = clip.source(sample.source)
                && source.region(sample.range()).is_some()
            {
                files::export_region(
                    &cuts.join(format!(
                        "Cut {:02} {}.wav",
                        index + 1,
                        filename(&sample.name)
                    )),
                    source,
                    sample.range(),
                )?;
            }
        }
        let keys = cuts.join("Keys");
        for note in PIANO_LOW..=PIANO_HIGH {
            let used = sequence
                .bars
                .iter()
                .flat_map(|bar| bar.keys.iter())
                .any(|step| *step == Some(note))
                || sequence
                    .performance
                    .iter()
                    .any(|event| event.input == ChopInput::Key(note));
            if (used || studio.key_chops.contains_key(&note))
                && let Some(source) = clip.source(studio.key_source(note))
                && let Some(range) = studio.key_range(note, source.duration())
                && source.region(range).is_some()
            {
                fs::create_dir_all(&keys)?;
                files::export_region(
                    &keys.join(format!("{note:03} {}.wav", piano_note(note))),
                    source,
                    range,
                )?;
            }
        }
        if !has_pattern && fs::read_dir(&cuts)?.next().is_none() {
            files::export_region(&cuts.join("Source.wav"), clip, (0.0, clip.duration()))?;
            for (index, source) in clip.sources.iter().take(studio.sources.len()).enumerate() {
                let title = filename(&studio.sources[index].title);
                files::export_region(&cuts.join(format!("Source {:02} {title}.wav", index + 2)), source, (0.0, source.duration()))?;
            }
        }
    }
    if options.processed && let Some(clip) = &clip {
        let cuts = root.join("Processed cuts");
        fs::create_dir(&cuts)?;
        for pad in 0..STEPS {
            if let Some(source) = clip.source(studio.pad_sources[pad])
                && studio.slice(pad, source.duration()).is_some()
            {
                files::export_pad(&cuts.join(format!("Pad {:02}.wav", pad + 1)), clip, &studio, pad, options.wav)?;
            }
        }
        for note in PIANO_LOW..=PIANO_HIGH {
            let used = sequence.bars.iter().any(|bar| bar.keys.contains(&Some(note)))
                || sequence.performance.iter().any(|event| event.input == ChopInput::Key(note));
            if (used || studio.key_chops.contains_key(&note))
                && let Some(source) = clip.source(studio.key_source(note))
                && studio.key_range(note, source.duration()).is_some()
            {
                files::export_key(&cuts.join(format!("Key {note:03}.wav")), clip, &studio, note, options.wav)?;
            }
        }
    }
    let markers: Vec<_> = studio.sections.iter().enumerate().filter_map(|(index, section)| {
        let range = studio.section_range(index)?;
        Some(serde_json::json!({
            "section": section.kind.label(), "pattern": studio.patterns[section.pattern].name,
            "first_bar": range.start / crate::model::TICKS_PER_BAR + 1,
            "bars": range.len() / crate::model::TICKS_PER_BAR as usize,
        }))
    }).collect();
    let pads: Vec<_> = (0..STEPS).filter_map(|pad| {
        let start = studio.pads[pad]?;
        Some(serde_json::json!({
            "pad": pad + 1, "name": studio.pad_names[pad], "source": studio.pad_sources[pad],
            "start_seconds": start, "end_seconds": studio.pad_ends[pad], "midi_note": 24 + pad,
            "choke_group": studio.choke_groups.pads[pad],
        }))
    }).collect();
    let source_names: Vec<_> = std::iter::once(title).chain(studio.sources.iter().map(|record| record.title.as_str())).collect();
    let manifest = serde_json::json!({
        "name": title, "bpm": studio.bpm, "bars": sequence.bars.len(), "time_signature": [4, 4],
        "wav_rate": options.wav.rate, "wav_bits": options.wav.bits, "tail_seconds": options.wav.tail_seconds,
        "dry_stems": options.dry_stems, "sections": markers, "sources": source_names, "pads": pads,
        "midi_channels": {"pads": 1, "keys": 2, "drums": 10}, "drum_notes": crate::midi_out::DRUM_NOTES,
        "drum_choke_groups": studio.choke_groups.drums,
        "sample_layers": studio.instruments,
    });
    serde_json::to_writer_pretty(fs::File::create(root.join("Manifest.json"))?, &manifest)?;
    let mut first_bar = 1;
    let mut sections = String::new();
    for section in &studio.sections {
        let pattern = &studio.patterns[section.pattern];
        let length = section.bars(pattern.length);
        let detail = if section.span.is_some() {
            format!(
                " · {length} bars from pattern bar {}",
                section.offset(pattern.length) + 1
            )
        } else {
            format!(" × {}", section.repeats)
        };
        sections.push_str(&format!(
            "Bars {first_bar}–{}: {} · {}{detail}\r\n",
            first_bar + length - 1,
            section.kind.label(),
            pattern.name,
        ));
        first_bar += length;
    }
    let automation = if !sequence.automation.is_empty() || !sequence.section_automation.is_empty() {
        "Song automation is rendered into the WAV mix and stems.\r\n"
    } else {
        ""
    };
    let choke = if studio.choke_groups.is_empty() {
        ""
    } else {
        "Choke groups are rendered in the WAV files. MIDI exports contain trigger notes and timing.\r\n"
    };
    let bends = if studio.uses_transcription() && options.midi {
        match options.midi_mode {
            crate::transcription::midi::Mode::PerNote => "Bent notes use independent MIDI channels 3–9 and 11–16. Enable those channels on the destination instrument.\r\n",
            crate::transcription::midi::Mode::Quantized => "MIDI uses semitone pitches. The WAV stems preserve the original bends.\r\n",
        }
    } else { "" };
    fs::write(
        root.join("Import guide.txt"),
        format!(
            "{title}\r\nTempo {} BPM | 4/4 | {} bar(s)\r\n{sections}\r\nImport the WAV files in Stems at bar 1 to mix the parts. They are aligned, {} Hz, {}-bit stereo.\r\nThe drums bus is an alternative to the individual drum stems. The mix is the full stereo render.\r\n{automation}{choke}Import the MIDI at bar 1. Drums use GM notes on channel 10, pad chops use channel 1 starting at note 24, and ordinary keys use channel 2.\r\n{bends}Samples contains the source-rate loop, pad slices, and named cuts available in the session.\r\nSamples/Keys contains piano chops labeled by their MIDI note numbers. These are raw cuts.\r\n",
            studio.bpm,
            sequence.bars.len(), options.wav.rate, options.wav.bits
        ),
    )?;
    let mut delivery = Delivery {
        written: Vec::new(),
        failed: Vec::new(),
    };
    crate::background::stage("Copying pack to destinations")?;
    let mut staged = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        crate::background::progress(index, targets.len())?;
        let result = (|| -> Result<tempfile::TempDir> {
            let pending = tempfile::Builder::new()
                .prefix(".lost-samples-")
                .tempdir_in(target)?;
            copy_tree(root, pending.path())?;
            Ok(pending)
        })();
        match result {
            Ok(pending) => staged.push((target, pending)),
            Err(error) => delivery.failed.push(format!("{} ({error})", target.display())),
        }
    }
    if staged.is_empty() && !delivery.failed.is_empty() {
        return Err(delivery.failed.join("\n").into());
    }
    crate::background::publish()?;
    for (target, pending) in staged {
        let final_path = target.join(&name);
        match fs::rename(pending.path(), &final_path) {
            Ok(()) => delivery.written.push(final_path),
            Err(error) => delivery.failed.push(format!("{} ({error})", target.display())),
        }
    }
    Ok(delivery)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from)? {
        crate::background::check()?;
        let entry = entry?;
        let destination = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir(&destination)?;
            copy_tree(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

fn filename(value: &str) -> String {
    let sanitized = value
        .chars()
        .take(64)
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim().trim_matches('.');
    if sanitized.is_empty() {
        "Untitled".into()
    } else {
        sanitized.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_song_exports_sections_and_reuses_edited_patterns() -> Result<()> {
        use crate::{
            audio::{PadMode, RENDER_RATE},
            model::{
                AutomationPoint, AutomationTarget, PlayedChop, Section, SectionKind, SectionSpan,
                TICKS_PER_BAR,
            },
        };
        let target = tempfile::tempdir()?;
        let path = target.path().join("song.flip");
        let clip = Clip::new(8_000, vec![[0.4; 2]; 800]);
        let mut studio = Studio {
            bpm: 240,
            ..Studio::default()
        };
        studio.pads[0] = Some(0.0);
        studio.hits[0][0][0] = 1;
        studio.save_pattern("Opening", false);
        studio.save_pattern("Verse beat", true);
        let hit = |tick, velocity| PlayedChop {
            id: Default::default(),
            tick,
            input: ChopInput::Pad(0),
            velocity,
            mode: PadMode::Slice,
            length: None,
            sound: Default::default(),
            expression: None,
        };
        studio.performance.push(hit(77, 75));
        studio.save_pattern("Hook beat", true);
        studio.bars = 2;
        studio
            .performance
            .extend([hit(109, 101), hit(TICKS_PER_BAR + 169, 111)]);
        studio.load_pattern(1);
        assert_eq!(studio.performance, vec![hit(77, 75)]);
        studio.performance[0].velocity = 88;
        studio.sync_pattern();
        studio.sections = vec![
            Section::new(SectionKind::Intro, 0, 1),
            Section::new(SectionKind::Verse, 1, 1),
            Section {
                span: Some(SectionSpan { offset: 1, bars: 2 }),
                ..Section::new(SectionKind::Hook, 2, 2)
            },
            Section::new(SectionKind::Verse, 1, 1),
        ];
        studio.automation.set_points(
            AutomationTarget::MasterVolume,
            vec![
                AutomationPoint {
                    tick: 0,
                    percent: 0, curve: Default::default() },
                AutomationPoint {
                    tick: 5 * TICKS_PER_BAR,
                    percent: 100, curve: Default::default() },
            ],
        );
        studio.automation.set_points(
            AutomationTarget::SampleVolume,
            vec![AutomationPoint {
                tick: 0,
                percent: 90, curve: Default::default() }],
        );
        studio.sections[2].automation.set_points(
            AutomationTarget::SampleVolume,
            vec![
                AutomationPoint {
                    tick: 0,
                    percent: 20, curve: Default::default() },
                AutomationPoint {
                    tick: 2 * TICKS_PER_BAR,
                    percent: 60, curve: Default::default() },
            ],
        );
        files::save_project(
            &path,
            &crate::model::project::Project {
                record: None,
                studio: studio.clone(),
                samples: Vec::new(),
                ..crate::model::project::Project::default()
            },
            Some(&clip),
        )?;
        let (project, clip) = files::open_project(&path)?;
        assert_eq!(project.studio, studio);
        let output = export(
            &[target.path().into()],
            "My song",
            project.studio,
            clip.map(Arc::new),
            &[],
        )?;
        let folder = &output.written[0];
        for wav in [
            "01 Mix.wav",
            "02 Drums bus.wav",
            "Stems/01 Sample.wav",
            "Stems/02 Kick.wav",
        ] {
            assert_eq!(
                hound::WavReader::open(folder.join(wav))?.duration(),
                5 * RENDER_RATE
            );
        }
        let sample = hound::WavReader::open(folder.join("Stems/01 Sample.wav"))?
            .into_samples::<i32>()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let beats = [(1, 77, 88), (2, 169, 111), (3, 109, 101), (4, 77, 88)];
        for (bar, tick, _) in beats {
            let frame = (bar * RENDER_RATE + tick * RENDER_RATE / TICKS_PER_BAR) as usize;
            assert_eq!(sample[2 * (frame - 1)], 0);
            assert!(sample[2 * (frame + 100)] > 200_000);
        }
        for (bar, tick, velocity) in beats {
            let frame = bar * RENDER_RATE + tick * RENDER_RATE / TICKS_PER_BAR + 100;
            let song_seconds = frame as f32 / RENDER_RATE as f32;
            let volume = if (2..4).contains(&bar) {
                0.20 + (song_seconds - 2.0) * 0.20
            } else {
                0.90
            };
            let expected = 0.4
                * (f32::from(velocity) / 127.0).powf(1.4)
                * (song_seconds / 5.0)
                * volume
                * 8_388_607.0;
            assert!((sample[2 * frame as usize] as f32 - expected).abs() < 12.0);
        }
        let frame_at = |bar| (bar * RENDER_RATE + 77 * RENDER_RATE / TICKS_PER_BAR + 100) as usize;
        let earlier = sample[2 * frame_at(1)];
        let later = sample[2 * frame_at(4)];
        assert!(later > 3 * earlier);
        let mut mix = hound::WavReader::open(folder.join("01 Mix.wav"))?;
        mix.seek(frame_at(4) as u32)?;
        assert_eq!(mix.samples::<i32>().next().transpose()?, Some(later));
        assert!(
            sample[..2 * RENDER_RATE as usize]
                .iter()
                .all(|&frame| frame == 0)
        );
        let midi = fs::read(folder.join("MIDI/Full song.mid"))?;
        let mut stream = midi[29..].iter().copied();
        let mut tick = 0;
        let mut pads = Vec::new();
        loop {
            let mut delta = 0_u32;
            loop {
                let byte = stream.next().unwrap();
                delta = (delta << 7) + u32::from(byte & 0x7f);
                if byte < 128 {
                    break;
                }
            }
            tick += delta;
            let status = stream.next().unwrap();
            if status == 0xff {
                break;
            }
            let note = stream.next().unwrap();
            let velocity = stream.next().unwrap();
            if status == 0x90 && note == 24 {
                pads.push((tick, velocity));
            }
        }
        assert_eq!(
            pads,
            beats.map(|(bar, tick, velocity)| (bar * TICKS_PER_BAR + tick, velocity))
        );
        assert_eq!(tick, 5 * TICKS_PER_BAR);
        let guide = fs::read_to_string(folder.join("Import guide.txt"))?;
        assert!(guide.contains("Bars 3–4: Hook · Hook beat · 2 bars from pattern bar 2"));
        assert!(guide.contains("Song automation is rendered into the WAV mix and stems."));
        Ok(())
    }

    #[test]
    fn delivers_identical_aligned_files_and_reports_unavailable_folders() -> Result<()> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let clip = Arc::new(Clip::new(24_000, vec![[0.3, -0.2]; 24_000]));
        let mut studio = Studio {
            bpm: 120,
            ..Studio::default()
        };
        studio.pads[0] = Some(0.0);
        studio.chops[0][0] = Some(0);
        studio.hits[0][0][0] = 1;
        studio
            .key_chops
            .insert(108, crate::model::KeyChop::Cut(0.25, 0.75));
        studio.key_steps[0][4] = Some(108);
        studio.key_steps[0][8] = Some(60);
        studio.performance.extend([
            crate::model::PlayedChop {
                id: Default::default(),
                tick: 87,
                input: ChopInput::Pad(0),
                velocity: 88,
                mode: crate::audio::PadMode::Gate,
                length: Some(33),
                sound: Default::default(),
            expression: None,
            },
            crate::model::PlayedChop {
                id: Default::default(),
                tick: 139,
                input: ChopInput::Key(108),
                velocity: 105,
                mode: crate::audio::PadMode::Slice,
                length: None,
                sound: Default::default(),
            expression: None,
            },
        ]);
        let delivery = export(
            &[
                first.path().into(),
                first.path().join("missing"),
                second.path().into(),
            ],
            "My / flip",
            studio,
            Some(clip),
            &[NamedSample {
                source: 0,
                name: "Hit / one".into(),
                start: 0.25,
                end: 0.75,
            }],
        )?;
        assert_eq!(delivery.written.len(), 2);
        assert_eq!(delivery.failed.len(), 1);
        let [a, b] = delivery.written.as_slice() else {
            unreachable!()
        };
        let read_both = |path: &str| -> Result<Vec<u8>> {
            let bytes = fs::read(a.join(path))?;
            assert_eq!(bytes, fs::read(b.join(path))?);
            Ok(bytes)
        };
        for path in [
            "01 Mix.wav",
            "02 Drums bus.wav",
            "Stems/01 Sample.wav",
            &format!("Stems/02 {}.wav", DRUMS[0]),
        ] {
            let bytes = read_both(path)?;
            let mut wav = hound::WavReader::new(std::io::Cursor::new(bytes))?;
            assert_eq!(wav.spec().sample_rate, 48_000);
            assert_eq!(wav.spec().bits_per_sample, 24);
            assert_eq!(wav.spec().channels, 2);
            assert_eq!(wav.duration(), 96_000);
            assert!(
                wav.samples::<i32>()
                    .any(|value| value.unwrap_or_default() != 0)
            );
        }
        for path in ["MIDI/Full pattern.mid", "MIDI/Chops.mid", "MIDI/Drums.mid"] {
            assert!(read_both(path)?.starts_with(b"MThd"));
        }
        let cut = read_both("Samples/Cut 01 Hit _ one.wav")?;
        let wav = hound::WavReader::new(std::io::Cursor::new(cut))?;
        assert_eq!(wav.spec().sample_rate, 24_000);
        assert_eq!(wav.duration(), 12_000);
        assert_eq!(
            read_both("Samples/Keys/108 C8.wav")?,
            read_both("Samples/Cut 01 Hit _ one.wav")?
        );
        assert_eq!(
            read_both("Samples/Keys/060 C4.wav")?,
            read_both("Samples/Pad 01.wav")?
        );
        let midi = read_both("MIDI/Keys.mid")?;
        assert!(
            midi.windows(5)
                .any(|event| event == [0x81, 0x0b, 0x91, 108, 105])
        );
        assert!(midi.windows(3).any(|event| event == [0x91, 108, 112]));
        assert!(midi.windows(3).any(|event| event == [0x91, 60, 112]));
        let midi = read_both("MIDI/Chops.mid")?;
        assert!(midi.windows(4).any(|event| event == [32, 0x90, 24, 88]));
        assert!(midi.windows(4).any(|event| event == [33, 0x80, 24, 0]));
        assert!(
            !read_both("MIDI/Drums.mid")?
                .windows(3)
                .any(|event| event == [0x90, 24, 88])
        );
        assert!(String::from_utf8(read_both("Import guide.txt")?)?.contains("120 BPM"));
        Ok(())
    }
}
