use crate::audio::{Clip, Command, Mixer, RENDER_RATE, noise};
use crate::model::{Library, NamedSample, Record, STEPS, Sequence, Source, Studio, project::Project};
use serde::Deserialize;
use std::{
    error::Error,
    f32::consts::TAU,
    fs::File,
    io::{self, BufReader, BufWriter, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    errors::Error as AudioError,
    formats::FormatOptions,
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
pub mod recovery;
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg", "m4a", "aac"];
const MAX_FRAMES: usize = 25_000_000;
const MAX_DOWNLOAD: usize = 30 * 1024 * 1024;
pub const MAX_PROJECT_FRAMES: usize = MAX_FRAMES * 2;
pub const MAX_PROJECT_METADATA: usize = 8 * 1024 * 1024;

pub fn title_for(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub fn local_record(path: PathBuf) -> Record {
    Record {
        title: title_for(&path),
        artist: "Local audio".into(),
        genre: "Other".into(),
        source: Source::File(path),
        release: None,
    }
}

pub fn decode_path(path: &Path) -> Result<Clip> {
    decode(
        Box::new(File::open(path)?),
        path.extension().and_then(|ext| ext.to_str()),
    )
}

fn decode(source: Box<dyn MediaSource>, extension: Option<&str>) -> Result<Clip> {
    crate::background::stage("Decoding audio")?;
    let mut format = probe_audio(source, extension)?;
    let track = format.default_track().ok_or("No audio track found")?;
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut rate = 0;
    let mut samples = Vec::new();
    let mut scratch: Option<SampleBuffer<f32>> = None;
    loop {
        crate::background::check()?;
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(AudioError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(AudioError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if rate != 0 && rate != spec.rate {
            return Err("Audio changes sample rate mid-file".into());
        }
        rate = spec.rate;
        if channels == 0 || samples.len() + decoded.frames() > MAX_FRAMES {
            return Err("Audio is too large or has an invalid channel layout".into());
        }
        if scratch
            .as_ref()
            .is_none_or(|buffer| buffer.capacity() < decoded.capacity() * channels)
        {
            scratch = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
        }
        let buffer = scratch.as_mut().expect("audio buffer allocated");
        buffer.copy_interleaved_ref(decoded);
        samples.extend(
            buffer
                .samples()
                .chunks_exact(channels)
                .map(|frame| [clean(frame[0]), clean(frame[usize::from(channels > 1)])]),
        );
    }
    if samples.is_empty() || rate == 0 {
        return Err("The file contained no decodable audio".into());
    }
    Ok(Clip::new(rate, samples))
}

fn probe_audio(
    source: Box<dyn MediaSource>,
    extension: Option<&str>,
) -> Result<Box<dyn symphonia::core::formats::FormatReader>> {
    let mut hint = Hint::new();
    if let Some(extension) = extension {
        hint.with_extension(extension);
    }
    Ok(symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(source, Default::default()),
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?
        .format)
}

pub fn audio_duration(path: &Path) -> Result<f64> {
    let format = probe_audio(
        Box::new(File::open(path)?),
        path.extension().and_then(|ext| ext.to_str()),
    )?;
    let codec = &format.default_track().ok_or("No audio track found")?.codec_params;
    if let (Some(frames), Some(rate)) = (codec.n_frames, codec.sample_rate)
        && frames > 0 && rate > 0
    {
        return Ok(frames as f64 / f64::from(rate));
    }
    drop(format);
    Ok(decode_path(path)?.duration())
}

fn clean(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

const DEMOS: [(&str, &str, u16); 12] = [
    ("Blue Hour", "Jazz", 87),
    ("Pocket Change", "Funk", 103),
    ("After the Rain", "Soul", 79),
    ("Night Window", "Ambient", 68),
    ("Late Checkout", "Lounge", 94),
    ("Soft Focus", "Synth", 110),
    ("Brake Lights", "Rock", 108),
    ("Avenue B", "Hip Hop", 92),
    ("Last Picture", "Soundtrack", 76),
    ("Sunday Paper", "Blues", 83),
    ("Plastic Mirror", "Disco", 119),
    ("Side Street", "Fusion", 101),
];

pub fn demos() -> Vec<Record> {
    DEMOS
        .iter()
        .enumerate()
        .map(|(i, (title, genre, _))| Record {
            title: (*title).into(),
            artist: "Lost Samples • Original demo".into(),
            genre: (*genre).into(),
            source: Source::Demo(i),
            release: None,
        })
        .collect()
}

pub fn load(record: &Record) -> Result<(Clip, u16)> {
    match &record.source {
        Source::File(path) => Ok((decode_path(path)?, 90)),
        Source::YouTube { id } => Ok((crate::youtube::load(id)?.1, 90)),
        Source::Demo(id) if *id < DEMOS.len() => Ok((demo(*id), DEMOS[*id].2)),
        Source::Demo(_) => Err("Unknown demo recording".into()),
        Source::Commons { media, .. } => Ok((download(media)?, 90)),
        Source::Generated(_) | Source::Extracted(_) => {
            Err("Open the project to load its generated audio.".into())
        }
    }
}

fn demo(id: usize) -> Clip {
    let rate = 32_000;
    let beat = 60.0 / f32::from(DEMOS[id].2);
    let duration = 32.0 * beat;
    let mut seed = 1 + id as u32 * 461;
    let roots = [0, -3, -5, -2, 0, 2, -3, -5];
    let color = [
        0.60, 0.83, 0.66, 0.42, 0.57, 0.92, 1.0, 0.46, 0.58, 0.75, 0.86, 0.70,
    ][id];
    let lead_line = [12, 15, 19, 22, 19, 17, 14, 10];
    let mut filtered = 0.0;
    let frames = (0..(duration * rate as f32).round() as usize)
        .map(|i| {
            let time = i as f32 / rate as f32;
            let in_beats = time / beat;
            let bar = (in_beats / 4.0) as usize % roots.len();
            let root = 45 + roots[bar] + id as i32 % 5;
            let cycle = (in_beats * 0.5).fract() * beat * 2.0;
            let pulse = (in_beats * 2.0).fract() * beat * 0.5;
            let wobble = 0.0022 * (time * 0.72 * TAU).sin() + 0.00035 * (time * 5.3 * TAU).sin();
            let note = |pitch: i32, t: f32| {
                let hz = 440.0 * 2.0_f32.powf((pitch - 69) as f32 / 12.0);
                TAU * hz * (t + wobble)
            };
            let bass_time = in_beats.fract() * beat;
            let bass_phase = note(root - 12 + 7 * ((in_beats as usize % 4 == 3) as i32), time);
            let bass = (bass_phase.sin() + 0.22 * (bass_phase * 2.0).sin())
                * 0.23
                * (bass_time * 55.0).min(1.0)
                * (-bass_time / 0.31).exp();
            let third = if (bar + id).is_multiple_of(3) { 4 } else { 3 };
            let mut chord = [0.0; 2];
            for (voice, interval) in [0, third, 7, 10, 14].into_iter().enumerate() {
                let age = (cycle - 0.009 * voice as f32).max(0.0);
                let phase = note(root + interval, age);
                let harmonic = phase.sin()
                    + color * 0.26 * (phase * 2.0).sin()
                    + color * 0.11 * (phase * 3.001).sin();
                let envelope = (age * 95.0).min(1.0)
                    * (-age / (0.30 + (1.0 - color) * 0.65)).exp()
                    * ((2.0 * beat - cycle) * 22.0).clamp(0.0, 1.0);
                let pan = (voice as f32 - 2.0) * 0.10;
                chord[0] += harmonic * envelope * (0.087 - pan * 0.087);
                chord[1] += harmonic * envelope * (0.087 + pan * 0.087);
            }
            let lead_index = ((in_beats * 2.0) as usize + bar + id) % lead_line.len();
            let lead_phase = note(root + lead_line[lead_index], pulse);
            let lead = if lead_index == 1 || lead_index == 6 {
                0.0
            } else {
                (lead_phase.sin() + color * 0.23 * (lead_phase * 2.005).sin())
                    * (pulse * 110.0).min(1.0)
                    * (-pulse / 0.13).exp()
                    * ((0.5 * beat - pulse) * 40.0).clamp(0.0, 1.0)
                    * 0.11
            };
            let texture = noise(&mut seed) * (1.0 - color) * 0.026;
            filtered += (texture - filtered) * 0.23;
            let end_fade = ((duration - time) * 45.0).clamp(0.0, 1.0);
            [
                (bass + chord[0] + lead * 0.86 + filtered) * end_fade,
                (bass + chord[1] + lead + filtered * 0.85) * end_fade,
            ]
        })
        .collect();
    Clip::new(rate, frames)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent("LostSamplesDesktop/0.1 (native audio crate browser)")
        .timeout(Duration::from_secs(24))
        .build()
}

#[derive(Deserialize)]
struct SearchReply {
    query: Option<SearchQuery>,
}
#[derive(Deserialize)]
struct SearchQuery {
    pages: std::collections::BTreeMap<String, CommonsFile>,
}
#[derive(Deserialize)]
struct CommonsFile {
    title: String,
    imageinfo: Vec<CommonsMedia>,
}
#[derive(Deserialize)]
struct CommonsMedia {
    url: String,
    descriptionurl: String,
    size: usize,
    duration: Option<f64>,
}

pub fn online_records(genre: &str) -> Result<Vec<Record>> {
    crate::background::stage("Searching Wikimedia Commons")?;
    let keywords = match genre {
        "All" => "78 rpm music recording",
        "Jazz" => "jazz 78 rpm",
        "Rock" => "rock guitar music",
        "Blues" => "blues 78 rpm",
        "Classical" => "classical piano recording",
        "World" => "traditional folk music recording",
        "Library" => "production library music",
        "Synth" => "synthesizer instrumental music",
        "Other" => "vintage music recording",
        other => return search_commons(genre, &format!("{other} music recording")),
    };
    search_commons(genre, keywords)
}

fn search_commons(genre: &str, keywords: &str) -> Result<Vec<Record>> {
    let reply: SearchReply = agent()
        .get("https://commons.wikimedia.org/w/api.php")
        .query("action", "query")
        .query("format", "json")
        .query("generator", "search")
        .query("gsrnamespace", "6")
        .query("gsrlimit", "30")
        .query("gsrsearch", &format!("{keywords} filetype:audio"))
        .query("prop", "imageinfo")
        .query("iiprop", "url|size|mime")
        .call()?
        .into_json()?;
    let records = reply
        .query
        .into_iter()
        .flat_map(|query| query.pages.into_values())
        .filter_map(|file| {
            let media = file.imageinfo.into_iter().next()?;
            let ext = file.title.rsplit('.').next()?.to_ascii_lowercase();
            if !AUDIO_EXTENSIONS.contains(&ext.as_str())
                || media.size > MAX_DOWNLOAD
                || !media.duration.is_some_and(|t| (12.0..=490.0).contains(&t))
            {
                return None;
            }
            let title = file.title.strip_prefix("File:").unwrap_or(&file.title);
            Some(Record {
                title: title_for(Path::new(title)).replace('_', " "),
                artist: "Wikimedia Commons".into(),
                genre: if genre == "All" { "Other" } else { genre }.into(),
                source: Source::Commons {
                    media: media.url,
                    page: media.descriptionurl,
                },
                release: None,
            })
        })
        .collect::<Vec<_>>();
    if records.is_empty() {
        return Err("No matching Commons recordings. Try All, Jazz, Blues or Classical.".into());
    }
    Ok(records)
}

fn download(url: &str) -> Result<Clip> {
    crate::background::stage("Downloading source audio")?;
    if !url.starts_with("https://upload.wikimedia.org/wikipedia/commons/") {
        return Err("Invalid Commons audio URL".into());
    }
    let extension = url
        .split('?')
        .next()
        .and_then(|path| path.rsplit('.').next());
    let mut bytes = Vec::new();
    agent()
        .get(url)
        .call()?
        .into_reader()
        .take(MAX_DOWNLOAD as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_DOWNLOAD {
        return Err("Recording exceeds the download limit".into());
    }
    decode(Box::new(Cursor::new(bytes)), extension)
}

pub fn publish_export(path: &Path, render: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let directory = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let staging = tempfile::tempdir_in(directory)?;
    let output = staging.path().join("export");
    render(&output)?;
    crate::background::publish()?;
    std::fs::rename(output, path)?;
    Ok(())
}

pub fn write_atomically(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    write(file.as_file_mut())?;
    file.as_file().sync_all()?;
    crate::background::check()?;
    file.persist(path)?;
    Ok(())
}

pub fn save_project(path: &Path, project: &Project, clip: Option<&Clip>) -> Result<()> {
    use crate::model::project_version::ProjectVersion;
    write_atomically(path, |file| {
        let json = serde_json::to_vec(project)?;
        if json.len() > MAX_PROJECT_METADATA {
            return Err("This project has too much pattern data to save.".into());
        }
        let mut writer = BufWriter::new(file);
        let extras = project.studio.sources.len();
        if extras >= crate::model::MAX_SOURCES
            || extras > clip.map_or(0, |clip| clip.sources.len())
        {
            return Err("Project source audio is incomplete.".into());
        }
        let has_snapshots = !project.snapshots.is_empty() || !project.source_ids.is_empty();
        if has_snapshots {
            validate_snapshot_sources(project, extras + usize::from(clip.is_some()))?;
        }
        let version = if project.uses_extraction() {
            ProjectVersion::Extraction
        } else if project.studio.uses_transcription()
            || project.snapshots.iter().any(|snapshot| snapshot.uses_transcription())
        {
            ProjectVersion::Transcription
        } else if project.studio.uses_grooves()
            || project.snapshots.iter().any(|snapshot| snapshot.uses_grooves())
        {
            ProjectVersion::Grooves
        } else if project.studio.uses_beat_maps()
            || project.snapshots.iter().any(|snapshot| snapshot.uses_beat_maps())
        {
            ProjectVersion::BeatMaps
        } else if project.studio.uses_instruments()
            || project.snapshots.iter().any(|snapshot| snapshot.uses_instruments())
        {
            ProjectVersion::Instruments
        } else if has_snapshots {
            ProjectVersion::Snapshots
        } else if !project.studio.choke_groups.is_empty() {
            ProjectVersion::ChokeGroups
        } else if extras == 0 {
            ProjectVersion::SingleSource
        } else {
            ProjectVersion::MultipleSources
        };
        writer.write_all(version.marker())?;
        writer.write_all(&(json.len() as u32).to_le_bytes())?;
        writer.write_all(&json)?;
        write_project_clip(&mut writer, clip)?;
        if let Some(clip) = clip {
            for source in clip.sources.iter().take(extras) {
                write_project_clip(&mut writer, Some(source))?;
            }
        }
        writer.flush()?;
        Ok(())
    })
}

fn write_project_clip(writer: &mut impl Write, clip: Option<&Clip>) -> Result<()> {
    let (rate, count) = clip.map_or((0, 0), |clip| (clip.rate, clip.frames.len() as u32));
    writer.write_all(&rate.to_le_bytes())?;
    writer.write_all(&count.to_le_bytes())?;
    if let Some(clip) = clip {
        crate::background::stage("Writing embedded audio")?;
        for (index, frame) in clip.frames.iter().enumerate() {
            if index.is_multiple_of(8192) {
                crate::background::progress(index, clip.frames.len())?;
            }
            for sample in frame {
                writer.write_all(&sample.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

fn read_project_clip(reader: &mut impl Read, remaining: &mut usize) -> Result<Option<Clip>> {
    let rate = read_u32(reader)?;
    let count = read_u32(reader)? as usize;
    if count > MAX_FRAMES || count > *remaining || (count != 0 && !(1_000..=384_000).contains(&rate)) {
        return Err("Invalid project audio".into());
    }
    if count == 0 {
        return Ok(None);
    }
    *remaining -= count;
    let mut frames = Vec::new();
    for index in 0..count {
        if index % 8192 == 0 { crate::background::check()?; }
        let left = clean(f32::from_bits(read_u32(reader)?));
        let right = clean(f32::from_bits(read_u32(reader)?));
        frames.push([left, right]);
    }
    Ok(Some(Clip::new(rate, frames)))
}

pub fn open_project(path: &Path) -> Result<(Project, Option<Clip>)> {
    use crate::model::project_version::ProjectVersion;
    let mut reader = BufReader::new(File::open(path)?);
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    let version = ProjectVersion::read(magic)?;
    let json_size = read_u32(&mut reader)? as usize;
    if json_size > MAX_PROJECT_METADATA {
        return Err("Invalid project metadata".into());
    }
    let mut json = vec![0; json_size];
    reader.read_exact(&mut json)?;
    let mut project: Project = serde_json::from_slice(&json)?;
    let mut remaining = MAX_PROJECT_FRAMES;
    let mut clip = read_project_clip(&mut reader, &mut remaining)?;
    let extras = project.studio.sources.len();
    if !version.accepts_sources(extras) {
        return Err("Invalid project source list.".into());
    }
    if extras > 0 {
        let root = clip.as_mut().ok_or("Missing primary project audio.")?;
        for _ in 0..extras {
            let audio = read_project_clip(&mut reader, &mut remaining)?.ok_or("Missing project source audio")?;
            root.sources.push(Arc::new(audio));
        }
    }
    if version >= ProjectVersion::Snapshots
        && (!project.snapshots.is_empty() || !project.source_ids.is_empty())
    {
        validate_snapshot_sources(&project, extras + usize::from(clip.is_some()))?;
    } else if !project.snapshots.is_empty() || !project.source_ids.is_empty() {
        return Err("Snapshot data requires project format LSFLIP04.".into());
    }
    if version < ProjectVersion::Instruments
        && (project.studio.uses_instruments() || project.snapshots.iter().any(|snapshot| snapshot.uses_instruments()))
    {
        return Err("Sampler controls and event identities require project format LSFLIP05.".into());
    }
    if version < ProjectVersion::BeatMaps
        && (project.studio.uses_beat_maps() || project.snapshots.iter().any(|snapshot| snapshot.uses_beat_maps()))
    {
        return Err("Beat maps require project format LSFLIP06.".into());
    }
    if version < ProjectVersion::Grooves
        && (project.studio.uses_grooves() || project.snapshots.iter().any(|snapshot| snapshot.uses_grooves()))
    {
        return Err("Grooves and per-hit timing require project format LSFLIP07.".into());
    }
    if version < ProjectVersion::Transcription
        && (project.studio.uses_transcription() || project.snapshots.iter().any(|snapshot| snapshot.uses_transcription()))
    {
        return Err("Transcribed notes and pitch curves require project format LSFLIP08.".into());
    }
    if version < ProjectVersion::Extraction && project.uses_extraction() {
        return Err("Extracted sources require project format LSFLIP09.".into());
    }
    validate_loaded_project(&mut project, clip.as_ref());
    Ok((project, clip))
}

fn validate_loaded_project(project: &mut Project, clip: Option<&Clip>) {
    project.studio.validate_media(clip);
    project.studio.validate_source_analysis(clip);
    project.studio.validate_song();
    NamedSample::validate_sources(&mut project.samples, |index| {
        clip?.source(index).map(Clip::duration)
    });
}

fn validate_snapshot_sources(project: &Project, count: usize) -> Result<()> {
    use crate::model::snapshot::{MAX_SNAPSHOT_BYTES, MAX_SNAPSHOTS, valid_source_ids};
    if project.snapshots.len() > MAX_SNAPSHOTS
        || serde_json::to_vec(&project.snapshots)?.len() > MAX_SNAPSHOT_BYTES
        || !valid_source_ids(&project.source_ids, count)
        || project.snapshots.iter().any(|snapshot| {
            !valid_source_ids(&snapshot.source_ids, snapshot.source_ids.len())
                || !snapshot.source_ids.iter().all(|id| project.source_ids.contains(id))
        })
    {
        return Err("Invalid snapshot data or missing snapshot sources.".into());
    }
    Ok(())
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

#[derive(Clone, Copy)]
pub struct WavOptions {
    pub rate: u32,
    pub bits: u16,
    pub tail_seconds: f32,
}

impl Default for WavOptions {
    fn default() -> Self {
        Self { rate: RENDER_RATE, bits: 24, tail_seconds: 0.0 }
    }
}

pub fn export_wav(
    path: &Path, studio: Studio, clip: Option<Arc<Clip>>, options: WavOptions,
) -> Result<()> {
    let sequence = studio
        .render_sequence()
        .ok_or("Reduce the song length before exporting.")?;
    render_wav_with(path, studio, clip, Arc::new(sequence), RenderPart::Mix, options)
}

#[derive(Clone, Copy)]
pub enum RenderPart {
    Mix,
    Sample,
    Drums,
    Drum(usize),
    Pads,
    Keys,
}

fn render_frames(studio: &Studio, sequence: &Sequence, rate: u32) -> usize {
    (f64::from(rate) * 240.0 * sequence.bars.len() as f64 / f64::from(studio.bpm))
        .round() as usize
}

pub fn bounce_mix(studio: Studio, clip: Option<Arc<Clip>>, sequence: Sequence, limit: usize) -> Result<Clip> {
    bounce_window(studio, clip, sequence, None, limit)
}

pub fn bounce_window(
    studio: Studio,
    clip: Option<Arc<Clip>>,
    sequence: Sequence,
    bars: Option<std::ops::Range<usize>>,
    limit: usize,
) -> Result<Clip> {
    let sequence = Arc::new(sequence);
    let frames = if let Some(bars) = &bars {
        if bars.is_empty() || bars.end > sequence.bars.len() {
            return Err("The comparison bar is outside one of the patterns.".into());
        }
        (f64::from(RENDER_RATE) * 240.0 * bars.len() as f64 / f64::from(studio.bpm)).round() as usize
    } else {
        render_frames(&studio, &sequence, RENDER_RATE)
    };
    if frames > limit.min(MAX_FRAMES) {
        return Err("The mix is too long for the remaining project audio space.".into());
    }
    let mut mixer = Mixer::new(RENDER_RATE);
    mixer.settings = (&studio).into();
    mixer.command(Command::Clip(clip));
    mixer.command(match bars {
        Some(bars) => Command::SongWindow(sequence, 1, bars, false),
        None => Command::Song(sequence, 1),
    });
    crate::background::stage("Rendering audio")?;
    let mut output = Vec::with_capacity(frames);
    for frame in 0..frames {
        if frame.is_multiple_of(4096) {
            crate::background::progress(frame, frames)?;
        }
        output.push(mixer.next_frame());
    }
    Ok(Clip::new(RENDER_RATE, output))
}

pub(crate) fn render_wav_with(
    path: &Path,
    studio: Studio,
    clip: Option<Arc<Clip>>,
    sequence: Arc<Sequence>,
    part: RenderPart,
    options: WavOptions,
) -> Result<()> {
    let WavOptions { rate, bits, tail_seconds } = options;
    if ![44_100, 48_000, 96_000].contains(&rate)
        || ![16, 24, 32].contains(&bits)
        || !(0.0..=10.0).contains(&tail_seconds)
    {
        return Err("Invalid WAV export format.".into());
    }
    write_atomically(path, |file| {
        let frames = render_frames(&studio, &sequence, rate);
        let tail = (f64::from(rate) * f64::from(tail_seconds)).round() as usize;
        let mut mixer = Mixer::new(rate);
        use crate::audio::StemRoute;
        mixer.stem_route = match part {
            RenderPart::Drum(track) => StemRoute::Drum(track),
            RenderPart::Pads => StemRoute::Pads,
            RenderPart::Keys => StemRoute::Keys,
            _ => StemRoute::All,
        };
        mixer.settings = (&studio).into();
        mixer.command(Command::Clip(clip));
        mixer.command(Command::Song(sequence, 1));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: if bits == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
        };
        let mut writer = hound::WavWriter::new(BufWriter::new(file), spec)?;
        let scale = ((1_i64 << (bits - 1)) - 1) as f32;
        crate::background::stage("Rendering audio")?;
        for index in 0..frames + tail {
            if index.is_multiple_of(4096) {
                crate::background::progress(index, frames + tail)?;
            }
            if index == frames {
                mixer.render_tail();
            }
            let frame = match part {
                RenderPart::Mix => mixer.next_frame(),
                RenderPart::Sample | RenderPart::Pads | RenderPart::Keys => mixer.next_stems().0,
                RenderPart::Drums | RenderPart::Drum(_) => mixer.next_stems().1,
            };
            for sample in frame {
                if bits == 32 {
                    writer.write_sample(sample)?;
                } else {
                    writer.write_sample((sample.clamp(-1.0, 1.0) * scale).round() as i32)?;
                }
            }
        }
        writer.finalize()?;
        Ok(())
    })
}

pub fn export_cut(
    path: &Path, clip: &Clip, range: (f64, f64),
    controls: Option<(crate::model::SampleControls, f64)>, options: WavOptions,
) -> Result<()> {
    if ![16, 24, 32].contains(&options.bits) || !(1_000..=384_000).contains(&options.rate) {
        return Err("Invalid cut export format.".into());
    }
    let input = clip.region(range).ok_or("Select a non-empty audio region.")?;
    let (controls, speed) = controls.map_or((None, 1.0), |(controls, speed)| (Some(controls), speed));
    let tuning = controls.map_or(0.0, |controls| f64::from(controls.tune));
    let scale = speed * 2.0_f64.powf(tuning / 12.0);
    let estimated = ((range.1 - range.0) * f64::from(options.rate) / scale).ceil();
    if !estimated.is_finite() || !(1.0..=(MAX_PROJECT_FRAMES as f64)).contains(&estimated) {
        return Err("The processed cut exceeds the WAV export limit.".into());
    }
    let estimated = estimated as usize;
    if let Some(controls) = controls {
        write_cut_frames(path, options, estimated, crate::audio::cut_frames(clip, range, controls, speed, options.rate))
    } else if options.rate == clip.rate {
        write_cut_frames(path, options, input.len(), input.iter().copied())
    } else {
        let ratio = f64::from(clip.rate) / f64::from(options.rate);
        let frames = (0..estimated).map(|frame| {
            let index = frame as f64 * ratio;
            let left = (index.floor() as usize).min(input.len() - 1);
            let right = (left + 1).min(input.len() - 1);
            let mix = (index - index.floor()) as f32;
            std::array::from_fn(|channel| input[left][channel] * (1.0 - mix) + input[right][channel] * mix)
        });
        write_cut_frames(path, options, estimated, frames)
    }
}

pub fn export_pad(path: &Path, clip: &Clip, studio: &Studio, pad: usize, options: WavOptions) -> Result<()> {
    let velocity = studio.pad_layers(pad).preview_velocity();
    let frames = crate::audio::pad_frames(clip, studio, pad, velocity, options.rate)
        .ok_or("The pad has no playable layers.")?;
    write_cut_frames(path, options, 0, frames)
}

pub fn export_key(path: &Path, clip: &Clip, studio: &Studio, note: u8, options: WavOptions) -> Result<()> {
    let velocity = studio.key_layers(note).preview_velocity();
    let frames = crate::audio::key_frames(clip, studio, note, velocity, options.rate)
        .ok_or("The key has no playable layers.")?;
    write_cut_frames(path, options, 0, frames)
}

fn write_cut_frames(
    path: &Path, options: WavOptions, estimated: usize, frames: impl Iterator<Item = crate::audio::Frame>,
) -> Result<()> {
    if ![16, 24, 32].contains(&options.bits) || !(1_000..=384_000).contains(&options.rate) {
        return Err("Invalid cut export format.".into());
    }
    write_atomically(path, |file| {
        let spec = hound::WavSpec {
            channels: 2, sample_rate: options.rate, bits_per_sample: options.bits,
            sample_format: if options.bits == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
        };
        let mut writer = hound::WavWriter::new(BufWriter::new(file), spec)?;
        let scale = ((1_i64 << (options.bits - 1)) - 1) as f32;
        crate::background::stage("Writing cut audio")?;
        for (index, frame) in frames.enumerate() {
            if index >= MAX_PROJECT_FRAMES {
                return Err("The processed cut exceeds the WAV export limit.".into());
            }
            if index.is_multiple_of(4096) {
                if estimated == 0 {
                    crate::background::check()?;
                } else {
                    crate::background::progress(index, estimated)?;
                }
            }
            for value in frame {
                if options.bits == 32 {
                    writer.write_sample(value)?;
                } else {
                    writer.write_sample((value.clamp(-1.0, 1.0) * scale).round() as i32)?;
                }
            }
        }
        writer.finalize()?;
        Ok(())
    })
}

pub fn export_region(path: &Path, clip: &Clip, range: (f64, f64)) -> Result<()> {
    let frames = clip
        .region(range)
        .ok_or("Select a non-empty audio region to export.")?;
    write_atomically(path, |file| {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: clip.rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(BufWriter::new(file), spec)?;
        crate::background::stage("Writing source audio")?;
        for (index, frame) in frames.iter().enumerate() {
            if index.is_multiple_of(4096) {
                crate::background::progress(index, frames.len())?;
            }
            for &sample in frame {
                writer.write_sample((sample * 8_388_607.0).round() as i32)?;
            }
        }
        writer.finalize()?;
        Ok(())
    })
}

#[cfg(test)]
pub fn export_midi(path: &Path, studio: &Studio) -> Result<()> {
    export_midi_mode(path, studio, Default::default())
}

pub fn export_midi_mode(path: &Path, studio: &Studio, mode: crate::transcription::midi::Mode) -> Result<()> {
    let sequence = studio
        .render_sequence()
        .ok_or("Reduce the song length before exporting.")?;
    export_midi_sequence_mode(path, studio, &sequence, mode)
}

pub fn export_midi_sequence_mode(path: &Path, studio: &Studio, sequence: &Sequence, mode: crate::transcription::midi::Mode) -> Result<()> {
    use crate::transcription::midi::{self, Mode};
    use crate::model::events::{Cell, ResolvedEvent};
    let tempo = (60_000_000 / u32::from(studio.bpm)).to_be_bytes();
    let mut track = vec![0, 0xff, 0x51, 3, tempo[1], tempo[2], tempo[3]];
    let mut notes = std::collections::BTreeMap::new();
    let mut bent = Vec::new();
    let end = sequence.ticks();
    let mut add = |event: ResolvedEvent| {
        if event.tick < u64::from(end) && event.assigned(studio) {
            let (part, mut pitch, velocity) = event.midi();
            let tick = event.tick as u32;
            let length = (event.midi_ticks as u32).min(end - tick).max(1);
            if let Some(expression) = event.expression.filter(|expression| expression.has_bends(length)) {
                if mode == Mode::PerNote {
                    bent.push(midi::Note { tick, end: tick + length, pitch, velocity, expression });
                    return;
                }
                let cents = crate::model::melody::cents_at(&expression.bends, 0.0);
                pitch = (i16::from(pitch) + (cents / 100.0).round() as i16).clamp(0, 127) as u8;
            }
            let channel = crate::midi_out::CHANNELS[part];
            notes.insert((channel, pitch, tick), (velocity, length));
        }
    };
    crate::background::stage("Writing MIDI notes")?;
    for (bar, data) in sequence.bars.iter().enumerate() {
        crate::background::progress(bar, sequence.bars.len())?;
        let cycle = sequence.bar_cycles.get(bar).copied().unwrap_or_default();
        for step in 0..STEPS {
            let absolute = (bar * STEPS + step) as u64;
            let column = Cell::column(&data.drums, data.pads[step], data.keys[step], data.metadata(), step);
            for (row, cell) in column.into_iter().enumerate() {
                if let Some(cell) = cell {
                    cell.resolve(studio.step_timing(), row, absolute, cycle, false).for_each(&mut add);
                }
            }
        }
    }
    for note in &sequence.performance {
        add(ResolvedEvent::recorded(note, u64::from(note.tick)));
    }
    let mut events = Vec::with_capacity(notes.len() * 2);
    let mut notes = notes.into_iter().peekable();
    while let Some(((channel, note, tick), (velocity, length))) = notes.next() {
        let mut release = (tick + length).min(end);
        if let Some(&((next_channel, next_note, next_tick), _)) = notes.peek()
            && (channel, note) == (next_channel, next_note)
        {
            release = release.min(next_tick);
        }
        events.push((tick, 10, [0x90 | channel, note, velocity]));
        events.push((release, 0, [0x80 | channel, note, 0]));
    }
    midi::append(&mut events, bent)?;
    events.sort_unstable();
    let mut last = 0;
    for (tick, _, packet) in events {
        midi_delta(&mut track, tick - last);
        track.extend(packet);
        last = tick;
    }
    midi_delta(&mut track, end - last);
    track.extend([0xff, 0x2f, 0]);
    write_atomically(path, |file| {
        file.write_all(b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk")?;
        file.write_all(&(track.len() as u32).to_be_bytes())?;
        file.write_all(&track)?;
        Ok(())
    })
}

fn midi_delta(track: &mut Vec<u8>, mut delta: u32) {
    let mut bytes = [0; 4];
    let mut index = 3;
    bytes[index] = (delta & 0x7f) as u8;
    while delta >= 128 {
        delta >>= 7;
        index -= 1;
        bytes[index] = (delta as u8 & 0x7f) | 0x80;
    }
    track.extend_from_slice(&bytes[index..]);
}

pub fn app_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(base.join("Lost Samples Desktop"))
}

fn library_path() -> Option<PathBuf> {
    Some(app_data_dir()?.join("crate.json"))
}

pub struct LibraryRecovery {
    pub message: String,
    backup: Option<Vec<u8>>,
}

impl LibraryRecovery {
    pub fn has_backup(&self) -> bool {
        self.backup.is_some()
    }

    pub fn restore(&self) -> Result<Option<PathBuf>> {
        let path = library_path().ok_or("No app data directory is available")?;
        self.restore_at(&path)
    }

    fn restore_at(&self, path: &Path) -> Result<Option<PathBuf>> {
        let backup = self.backup.as_deref().ok_or("No readable crate backup")?;
        let _lock = lock_library(path)?;
        if read_optional(&path.with_extension("json.bak"))?.as_deref() != Some(backup) {
            return Err("The crate backup changed. Restart to load the current backup.".into());
        }
        let preserved = if let Some(bytes) = read_optional(path)? {
            if serde_json::from_slice::<Library>(&bytes).is_ok() {
                return Err("The crate is now readable. Restart to load it.".into());
            }
            let mut file = tempfile::Builder::new()
                .prefix("crate.damaged-")
                .suffix(".json")
                .tempfile_in(path.parent().ok_or("No app data directory is available")?)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            Some(file.into_temp_path().keep()?)
        } else {
            None
        };
        write_atomically(path, |file| Ok(file.write_all(backup)?))?;
        Ok(preserved)
    }
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn lock_library(path: &Path) -> Result<File> {
    let lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    lock.try_lock()?;
    Ok(lock)
}

fn load_library_file(path: &Path) -> Result<Option<(Library, Vec<u8>)>> {
    read_optional(path)?
        .map(|bytes| Ok((serde_json::from_slice(&bytes)?, bytes)))
        .transpose()
}

pub fn read_library() -> (Library, Option<LibraryRecovery>) {
    match library_path() {
        Some(path) => read_library_at(&path),
        None => (
            Library::default(),
            Some(LibraryRecovery {
                message: "No app data directory is available. Crate changes are session-only.".into(),
                backup: None,
            }),
        ),
    }
}

fn read_library_at(path: &Path) -> (Library, Option<LibraryRecovery>) {
    let primary = match load_library_file(path) {
        Ok(Some((library, _))) => return (library, None),
        other => other,
    };
    let backup_path = path.with_extension("json.bak");
    let backup = load_library_file(&backup_path);
    if matches!((&primary, &backup), (Ok(None), Ok(None))) {
        return (Library::default(), None);
    }

    let problem = match primary {
        Err(error) => format!("Could not load {} ({error}).", path.display()),
        Ok(_) => format!("{} is missing.", path.display()),
    };
    let (library, backup, detail) = match backup {
        Ok(Some((library, bytes))) => {
            (library, Some(bytes), "Using the last good backup.".into())
        }
        Ok(None) => (
            Library::default(),
            None,
            format!("Backup missing at {}.", backup_path.display()),
        ),
        Err(error) => (
            Library::default(),
            None,
            format!("Could not load {} ({error}).", backup_path.display()),
        ),
    };
    (
        library,
        Some(LibraryRecovery {
            message: format!("{problem} {detail} Crate changes are session-only until recovery."),
            backup,
        }),
    )
}

pub fn write_library(library: &Library) -> Result<()> {
    let path = library_path().ok_or("No app data directory is available")?;
    write_library_at(&path, library)
}

fn write_library_at(path: &Path, library: &Library) -> Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let _lock = lock_library(path)?;
    let backup = path.with_extension("json.bak");
    let previous = load_library_file(path)
        .map_err(|error| format!("The crate could not be read. Restart to recover it ({error})."))?;
    if previous.is_none() && read_optional(&backup)?.is_some() {
        return Err("The crate is missing and a backup exists. Restart to recover it.".into());
    }
    let bytes = serde_json::to_vec_pretty(library)?;
    let last_good = previous.as_ref().map_or(bytes.as_slice(), |(_, raw)| raw);
    write_atomically(&backup, |file| Ok(file.write_all(last_good)?))?;
    write_atomically(path, |file| Ok(file.write_all(&bytes)?))
}

pub fn export_json(path: &Path, library: &Library) -> Result<()> {
    write_atomically(path, |file| {
        serde_json::to_writer_pretty(file, library)?;
        Ok(())
    })
}

pub fn export_csv(path: &Path, library: &Library) -> Result<()> {
    write_atomically(path, |file| {
        writeln!(
            file,
            "Title,Artist,Genre,Collection,BPM,Loop In,Loop Out,Source"
        )?;
        for saved in &library.records {
            let quote = |value: &str| format!("\"{}\"", value.replace('"', "\"\""));
            let source = match &saved.record.source {
                Source::Demo(_) => "Original demo".into(),
                Source::File(path) => path.display().to_string(),
                Source::YouTube { id } => crate::youtube::watch_url(id).unwrap_or_default(),
                Source::Commons { page, .. } => page.clone(),
                Source::Generated(_) | Source::Extracted(_) => "Embedded project audio".into(),
            };
            writeln!(
                file,
                "{},{},{},{},{},{},{},{}",
                quote(&saved.record.title),
                quote(&saved.record.artist),
                quote(&saved.record.genre),
                quote(saved.collection.as_deref().unwrap_or("")),
                saved.studio.bpm,
                saved
                    .studio
                    .loop_in
                    .map_or(String::new(), |n| format!("{n:.3}")),
                saved
                    .studio
                    .loop_out
                    .map_or(String::new(), |n| format!("{n:.3}")),
                quote(&source)
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_crates_keep_their_interface_preferences_and_defaults() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("crate.json");
        std::fs::write(&path, br#"{"records":[],"collections":[],"heard":7}"#)?;
        let (library, recovery) = read_library_at(&path);
        assert!(recovery.is_none());
        assert_eq!(library.heard, 7);
        assert_eq!(library.interface, Default::default());

        std::fs::write(
            &path,
            br#"{"records":[],"collections":[],"heard":7,"interface":{"zoom":1.25,"high_contrast":true,"show_pads":false}}"#,
        )?;
        let (library, recovery) = read_library_at(&path);
        assert!(recovery.is_none());
        assert_eq!(
            library.interface,
            crate::model::preferences::Preferences {
                zoom: 1.25,
                high_contrast: true,
                show_pads: false,
                ..Default::default()
            }
        );
        write_library_at(&path, &library)?;
        let (reopened, recovery) = read_library_at(&path);
        assert!(recovery.is_none());
        assert_eq!(reopened.interface, library.interface);
        Ok(())
    }

    #[test]
    fn damaged_crate_preserves_its_bytes_and_restores_the_previous_good_save() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("crate.json");
        assert!(read_library_at(&path).1.is_none());

        let mut library = Library {
            heard: 12,
            ..Library::default()
        };
        write_library_at(&path, &library)?;
        let first = std::fs::read(&path)?;
        assert_eq!(std::fs::read(path.with_extension("json.bak"))?, first);
        library.heard = 34;
        write_library_at(&path, &library)?;
        assert_eq!(read_library_at(&path).0.heard, 34);

        let damaged = b"{ damaged crate \xff";
        std::fs::write(&path, damaged)?;
        let (library, recovery) = read_library_at(&path);
        let recovery = recovery.expect("damaged crate should require recovery");
        assert_eq!(library.heard, 12);
        assert!(recovery.has_backup());
        assert!(write_library_at(&path, &Library::default()).is_err());
        assert_eq!(std::fs::read(&path)?, damaged);

        let preserved = recovery.restore_at(&path)?.expect("damaged crate copy");
        assert_eq!(std::fs::read(preserved)?, damaged);
        assert_eq!(std::fs::read(&path)?, first);
        assert!(read_library_at(&path).1.is_none());
        assert!(recovery.restore_at(&path).is_err());
        Ok(())
    }

    #[test]
    fn missing_crate_requires_recovery_if_a_backup_exists() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("crate.json");
        let backup = path.with_extension("json.bak");
        std::fs::write(&backup, b"invalid backup")?;
        let (_, recovery) = read_library_at(&path);
        assert!(!recovery.expect("backup exists").has_backup());
        assert!(write_library_at(&path, &Library::default()).is_err());
        assert_eq!(std::fs::read(&backup)?, b"invalid backup");

        let library = Library {
            heard: 7,
            ..Library::default()
        };
        export_json(&backup, &library)?;
        let (library, recovery) = read_library_at(&path);
        assert_eq!(library.heard, 7);
        assert!(recovery.expect("backup exists").restore_at(&path)?.is_none());
        assert_eq!(read_library_at(&path).0.heard, 7);
        Ok(())
    }

    #[test]
    fn crate_read_error_does_not_allow_saving_over_the_path() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("crate.json");
        std::fs::create_dir(&path)?;
        let (_, recovery) = read_library_at(&path);
        assert!(!recovery.expect("crate read failed").has_backup());
        assert!(write_library_at(&path, &Library::default()).is_err());
        assert!(path.is_dir());
        assert!(!path.with_extension("json.bak").exists());
        Ok(())
    }

    #[test]
    fn cancelled_export_preserves_existing_destination_and_retry_can_replace_it() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("export.wav");
        std::fs::write(&path, b"existing")?;
        let control = crate::background::Control::default();
        let result = control.run(|| publish_export(&path, |staged| {
            std::fs::write(staged, b"new render")?;
            control.cancel();
            Ok(())
        }));
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path)?, b"existing");
        assert_eq!(std::fs::read_dir(folder.path())?.count(), 1);

        crate::background::Control::default().run(|| publish_export(&path, |staged| {
            std::fs::write(staged, b"new render")?;
            Ok(())
        }))?;
        assert_eq!(std::fs::read(&path)?, b"new render");
        Ok(())
    }

    #[test]
    fn wav_export_uses_the_selected_rate_format_and_tail_length() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("mix.wav");
        let mut studio = Studio { bpm: 120, ..Studio::default() };
        studio.hits[0][0][15] = 2;
        studio.delay = 0.8;
        export_wav(&path, studio, None, WavOptions { rate: 44_100, bits: 32, tail_seconds: 0.5 })?;
        let mut wav = hound::WavReader::open(path)?;
        assert_eq!(wav.spec().sample_rate, 44_100);
        assert_eq!(wav.spec().sample_format, hound::SampleFormat::Float);
        assert_eq!(wav.duration(), 110_250);
        assert!(wav.samples::<f32>().skip(176_400).any(|sample| sample.is_ok_and(|sample| sample.abs() > 0.001)));
        Ok(())
    }

    #[test]
    fn baked_groove_reopens_and_matches_wav_and_midi_hit_time() -> Result<()> {
        use crate::model::groove::{Provenance, Slot, Template, Transfer};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("groove.flip");
        let wav = directory.path().join("groove.wav");
        let midi = directory.path().join("groove.mid");
        let clip = Clip::new(RENDER_RATE, vec![[0.4; 2]; 64]);
        let mut studio = Studio { bpm: 120, swing: 0.5, ..Studio::default() };
        studio.hits[0][0][1] = 1;
        studio.hits[0][0][3] = 1;
        studio.drum_sources[0] = Some(0);
        studio.lane_offsets[2] = -3;
        let mut template = Template {
            name: "Test break".into(), bars: 1, slots: vec![Slot { offset: 0, accent: 1.0 }; STEPS],
            source: Provenance { title: "Original break".into(), fingerprint: "test".into(), range: (1.0, 3.0) },
        };
        template.slots[1] = Slot { offset: -18, accent: 0.5 };
        template.slots[3] = Slot { offset: 20, accent: 1.0 };
        let transfer = Transfer { velocity: 100, ..Default::default() };
        let baked = transfer.bake(studio.active_sequence(), &template);
        studio.step_feel[0].clone_from(&baked.bars[0].feel);
        studio.hits[0] = baked.bars[0].drums;
        studio.grooves.push(template);
        studio.save_pattern("With groove", false);
        let project = Project { studio, ..Project::default() };
        save_project(&path, &project, Some(&clip))?;
        assert_eq!(&std::fs::read(&path)?[..8], b"LSFLIP07");
        let (reopened, clip) = open_project(&path)?;
        assert!(reopened == project);
        let studio = reopened.studio;
        export_midi(&midi, &studio)?;
        let midi = std::fs::read(midi)?;
        assert_eq!(&midi[29..34], &[0x81, 1, 0x99, crate::midi_out::DRUM_NOTES[0], 45]);
        export_wav(&wav, studio, clip.map(Arc::new), WavOptions { bits: 32, ..Default::default() })?;
        let wav = decode_path(&wav)?;
        let first = wav.frames.iter().position(|frame| frame[0].abs() > 0.001).unwrap();
        assert_eq!(first, 129 * 50);
        Ok(())
    }

    #[test]
    fn hit_sound_overrides_are_local_to_the_trigger_and_persist() -> Result<()> {
        use crate::model::instrument::HitSound;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("hit-sound.flip");
        let clip = Clip::new(RENDER_RATE, (0..24_000).map(|frame| {
            [frame as f32 / RENDER_RATE as f32; 2]
        }).collect());
        let mut studio = Studio { bpm: 120, ..Studio::default() };
        studio.pads[0] = Some(0.0);
        studio.pad_ends[0] = Some(0.5);
        studio.pad_controls[0].attack_ms = 0;
        for step in [0, 4, 8] {
            studio.chops[0][step] = Some(0);
        }
        studio.set_step_sound(0, 0, 0, HitSound {
            pitch_cents: 1200, start_percent: 50, cutoff_hz: None,
        });
        studio.set_step_sound(0, 0, 8, HitSound {
            pitch_cents: 0, start_percent: 50, cutoff_hz: Some(20),
        });
        studio.identify_working_events();
        let project = Project { studio, ..Project::default() };
        save_project(&path, &project, Some(&clip))?;
        let (saved, audio) = open_project(&path)?;
        assert!(saved == project);
        let sequence = saved.studio.active_sequence();
        let mix = bounce_mix(saved.studio, audio.map(Arc::new), sequence, MAX_FRAMES)?;
        assert!((mix.frames[100][0] - (0.25 + 200.0 / 48_000.0)).abs() < 1e-5);
        assert!((mix.frames[24_100][0] - 100.0 / 48_000.0).abs() < 1e-5);
        assert!((0.03..0.07).contains(&mix.frames[48_100][0]));
        Ok(())
    }

    #[test]
    fn layers_reopen_and_render_inclusive_velocity_switches() -> Result<()> {
        use crate::model::instrument::{Layer, Layers, VelocityRange};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("layered.flip");
        let mut clip = Clip::new(8_000, vec![[0.6, 0.0]; 4_000]);
        clip.sources.push(Arc::new(Clip::new(16_000, vec![[0.0, 0.4]; 8_000])));
        let mut studio = Studio::default();
        studio.sources.push(local_record("second.wav".into()));
        studio.pads[0] = Some(0.0);
        let layer = Layer {
            source: 1,
            start: 0.1,
            end: 0.3,
            controls: Default::default(),
            velocity: VelocityRange { min: 64, max: 127 },
        };
        studio.instruments.pads.insert(0, Layers {
            base: VelocityRange { min: 1, max: 63 },
            extra: [Some(layer), Some(layer), None],
        });
        let project = Project { studio, ..Project::default() };
        save_project(&path, &project, Some(&clip))?;
        assert_eq!(&std::fs::read(&path)?[..8], b"LSFLIP05");
        let (reopened, clip) = open_project(&path)?;
        let clip = clip.unwrap();
        assert!(project == reopened);
        let low = crate::audio::pad_frames(&clip, &reopened.studio, 0, 63, 48_000).unwrap().nth(200).unwrap();
        let high = crate::audio::pad_frames(&clip, &reopened.studio, 0, 64, 48_000).unwrap().nth(200).unwrap();
        assert!(low[0] > 0.0 && low[1] == 0.0);
        let expected = 0.8 * (64.0_f32 / 127.0).powf(1.4);
        assert!(high[0] == 0.0 && (high[1] - expected).abs() < 1e-6);
        let wav = directory.path().join("layered.wav");
        export_pad(&wav, &clip, &reopened.studio, 0, WavOptions { bits: 32, ..WavOptions::default() })?;
        let samples = hound::WavReader::open(wav)?.samples::<f32>().skip(400).take(2)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(samples, [0.0, 0.8]);
        Ok(())
    }

    #[test]
    fn snapshots_reopen_and_resolve_audio_after_source_order_changes() -> Result<()> {
        use crate::model::snapshot::{NamedSnapshot, SourceId};
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("snapshots.flip");
        let mut studio = Studio { bpm: 120, ..Studio::default() };
        studio.sources = vec![local_record("second.wav".into()), local_record("third.wav".into())];
        studio.pads[0] = Some(0.0);
        studio.pad_sources[0] = 1;
        studio.chops[0][0] = Some(0);
        let ids = vec![SourceId(1), SourceId(2), SourceId(3)];
        let saved = NamedSnapshot::new("Before".into(), 1, ids.clone(), studio.clone(), Vec::new());
        let mut clip = Clip::new(8000, vec![[0.0; 2]; 800]);
        clip.sources.push(Arc::new(Clip::new(8000, vec![[-0.25; 2]; 800])));
        clip.sources.push(Arc::new(Clip::new(8000, vec![[0.25; 2]; 800])));
        // The current registry and embedded audio are in a different order.
        let mut source_ids = ids;
        source_ids.swap(1, 2);
        studio.sources.swap(0, 1);
        clip.sources.swap(0, 1);
        let project = Project {
            studio,
            source_ids,
            snapshots: Arc::new(vec![saved]),
            ..Project::default()
        };
        save_project(&path, &project, Some(&clip))?;
        assert_eq!(&std::fs::read(&path)?[..8], b"LSFLIP04");
        let (loaded, media) = open_project(&path)?;
        let (restored, _) = loaded.snapshots[0]
            .resolve(&loaded.studio.sources, &loaded.source_ids)
            .ok_or("Snapshot source missing")?;
        assert_eq!(restored.pad_sources[0], 2);
        assert!(loaded == project);
        let media = Arc::new(media.ok_or("Missing embedded audio")?);
        let bounced = bounce_mix(restored.clone(), Some(media), restored.active_sequence(), MAX_FRAMES)?;
        assert!(bounced.frames[100][0] < 0.0);
        Ok(())
    }

    #[test]
    fn choke_groups_survive_reopen_and_apply_to_isolated_stems() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let project_path = directory.path().join("choke.flip");
        let stem = directory.path().join("first-drum.wav");
        let mut studio = Studio { bpm: 120, ..Studio::default() };
        studio.choke_groups.pads[0] = 1;
        studio.choke_groups.drums[..2].fill(1);
        studio.pads[0] = Some(0.0);
        studio.chops[0][0] = Some(0);
        studio.hits[0][0][0] = 1;
        studio.hits[0][1][1] = 1;
        studio.drum_sources[..2].fill(Some(0));
        let clip = Clip::new(RENDER_RATE, vec![[0.2; 2]; 24_000]);
        let project = Project { studio, ..Project::default() };
        save_project(&project_path, &project, Some(&clip))?;
        assert_eq!(&std::fs::read(&project_path)?[..8], b"LSFLIP03");
        let (reopened, clip) = open_project(&project_path)?;
        assert!(reopened == project);
        let clip = clip.map(Arc::new);
        let studio = reopened.studio;
        let sequence = studio.active_sequence();
        let mix = bounce_mix(studio.clone(), clip.clone(), sequence.clone(), MAX_FRAMES)?;
        render_wav_with(
            &stem,
            studio,
            clip,
            Arc::new(sequence),
            RenderPart::Drum(0),
            WavOptions { bits: 32, ..WavOptions::default() },
        )?;
        let frames = hound::WavReader::open(stem)?.samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // The simultaneous drum wins over the pad. The second drum then chokes
        // the first, including when only the first drum's output is routed.
        assert!((frames[200] - mix.frames[100][0]).abs() < 1e-6);
        assert!((frames[2 * 6000] - frames[200]).abs() < 1e-6);
        assert!((frames[2 * 6120] - frames[200] * 0.5).abs() < 1e-6);
        assert_eq!(frames[2 * 6240], 0.0);
        assert!(mix.frames[6240][0] > 0.0);
        Ok(())
    }

    #[test]
    fn legacy_projects_keep_their_midi_exports() -> Result<()> {
        for stem in ["single-source", "multi-source"] {
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy");
            let path = fixtures.join(format!("{stem}.flip"));
            let (project, clip) = open_project(&path)?;
            assert!(clip.is_some());
            let temp = tempfile::tempdir()?;
            let midi = temp.path().join("render.mid");
            export_midi(&midi, &project.studio)?;
            assert_eq!(
                std::fs::read(midi)?,
                std::fs::read(fixtures.join(format!("{stem}.mid")))?,
                "{stem}",
            );
            #[cfg(windows)]
            {
                let wav = temp.path().join("render.wav");
                export_wav(&wav, project.studio, clip.map(Arc::new), WavOptions {
                    bits: 16,
                    ..WavOptions::default()
                })?;
                let hash = std::fs::read(wav)?.into_iter().fold(0xcbf29ce484222325_u64, |acc, byte| {
                    (acc ^ u64::from(byte)).wrapping_mul(0x100000001b3)
                });
                let expected = std::fs::read_to_string(fixtures.join(format!("{stem}.wav.fnv")))?;
                assert_eq!(format!("{hash:016x}"), expected.trim(), "{stem}");
            }
        }
        Ok(())
    }

    #[test]
    fn multi_source_project_renders_each_pad_from_its_embedded_recording() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let project_path = directory.path().join("two-sources.flip");
        let output_path = directory.path().join("mix.wav");
        let mut media = Clip::new(8_000, vec![[0.25; 2]; 4_000]);
        media.sources.push(Arc::new(Clip::new(16_000, vec![[-0.35; 2]; 8_000])));
        let mut studio = Studio { bpm: 120, ..Studio::default() };
        studio.sources.push(local_record("secondary.wav".into()));
        studio.pads[0] = Some(0.0);
        studio.pad_ends[0] = Some(0.25);
        studio.pads[1] = Some(0.0);
        studio.pad_ends[1] = Some(0.25);
        studio.pad_sources[1] = 1;
        studio.chops[0][0] = Some(0);
        studio.chops[0][8] = Some(1);
        let project = Project {
            record: Some(local_record("primary.wav".into())),
            studio,
            samples: vec![NamedSample { source: 1, name: "Second cut".into(), start: 0.1, end: 0.3 }],
            ..Project::default()
        };
        save_project(&project_path, &project, Some(&media))?;
        let (loaded, media) = open_project(&project_path)?;
        assert_eq!(loaded.samples, project.samples);
        assert_eq!(loaded.studio.pad_sources, project.studio.pad_sources);
        let media = media.unwrap();
        assert_eq!(media.source(1).unwrap().rate, 16_000);
        export_wav(&output_path, loaded.studio, Some(Arc::new(media)), WavOptions::default())?;
        let mix = decode_path(&output_path)?;
        assert!((mix.frames[512][0] - 0.25).abs() < 1e-4);
        assert!((mix.frames[48_512][0] + 0.35).abs() < 1e-4);
        Ok(())
    }

    #[test]
    fn exports_the_selected_frames_at_the_source_rate() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("sample.wav");
        let clip = Clip::new(44_100, (0..40).map(|i| [i as f32 / 40.0, 0.0]).collect());
        export_region(&path, &clip, (8.0 / 44_100.0, 12.0 / 44_100.0))?;
        let exported = decode_path(&path)?;
        assert_eq!(exported.rate, 44_100);
        assert_eq!(exported.frames.len(), 4);
        assert!((exported.frames[0][0] - 0.2).abs() < 1e-6);
        assert!((exported.frames[3][0] - 0.275).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn project_round_trip_embeds_audio_and_preserves_the_flip() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("test.flip");
        let record = local_record(directory.path().join("missing-original.wav"));
        let clip = Clip::new(RENDER_RATE, vec![[0.25, -0.20]; 48_000]);
        let mut studio = Studio {
            bpm: 120,
            bars: 2,
            loop_in: Some(0.125),
            loop_out: Some(0.875),
            looping: true,
            ..Studio::default()
        };
        studio.auto_chop();
        studio.apply_drum_template(1, &crate::model::drum_templates::TEMPLATES[0], false);
        studio.chops[1][2] = Some(15);
        studio
            .key_chops
            .insert(108, crate::model::KeyChop::Cut(0.41, 0.64));
        studio.key_steps[1][3] = Some(108);
        studio.performance.push(crate::model::PlayedChop {
            id: Default::default(),
            tick: 1977,
            input: crate::model::ChopInput::Key(108),
            velocity: 105,
            mode: crate::audio::PadMode::Gate,
            length: Some(49),
            sound: Default::default(),
            expression: None,
        });
        studio.key_chops.insert(60, crate::model::KeyChop::Muted);
        studio.key_controls.insert(
            108,
            crate::model::SampleControls {
                reverse: true,
                ..crate::model::SampleControls::default()
            },
        );
        studio.pad_controls[15].tune = -7.0;
        let samples = vec![
            NamedSample {
                source: 0,
                name: "Intro".into(),
                start: 0.125,
                end: 0.375,
            },
            NamedSample {
                source: 0,
                name: "Turnaround".into(),
                start: 0.5,
                end: 0.875,
            },
        ];
        save_project(
            &path,
            &Project {
                record: Some(record.clone()),
                studio: studio.clone(),
                samples: samples.clone(),
                ..Project::default()
            },
            Some(&clip),
        )?;
        let (reopened, audio) = open_project(&path)?;
        assert_eq!(reopened.record, Some(record));
        assert_eq!(reopened.studio, studio);
        assert_eq!(reopened.samples, samples);
        let audio = audio.ok_or("Missing embedded audio")?;
        assert_eq!(audio.rate, RENDER_RATE);
        assert_eq!(audio.frames, clip.frames);
        Ok(())
    }

    #[test]
    fn wav_and_midi_export_preserve_chops_and_drums() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let wav = directory.path().join("test.wav");
        let midi = directory.path().join("test.mid");
        let clip = Clip::new(RENDER_RATE, vec![[0.25, -0.20]; 48_000]);
        let mut studio = Studio {
            bpm: 120,
            swing: 1.0,
            ..Studio::default()
        };
        studio.pads[0] = Some(0.0);
        studio.chops[0][0] = Some(0);
        studio.hits[0][0][15] = 2;
        export_wav(&wav, studio.clone(), Some(Arc::new(clip)), WavOptions::default())?;
        let decoded = decode_path(&wav)?;
        assert_eq!(decoded.rate, RENDER_RATE);
        assert_eq!(decoded.frames.len(), 96_000);
        assert!((decoded.frames[256][0] - 0.25).abs() < 1e-5);
        assert!(
            decoded.frames[94_000..]
                .iter()
                .any(|frame| frame[0].abs() > 0.05)
        );

        export_midi(&midi, &studio)?;
        let midi = std::fs::read(midi)?;
        assert_eq!(&midi[..4], b"MThd");
        assert_eq!(&midi[14..18], b"MTrk");
        assert!(midi.windows(3).any(|bytes| bytes == [0x90, 24, 112]));
        assert!(midi.windows(3).any(|bytes| bytes == [0x99, 36, 127]));
        assert!(midi.ends_with(&[0xff, 0x2f, 0]));
        Ok(())
    }

    #[test]
    fn active_sample_loop_renders_even_without_sequenced_chops() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let wav = directory.path().join("loop.wav");
        let clip = Clip::new(RENDER_RATE, vec![[0.30; 2]; 24_000]);
        let studio = Studio {
            bpm: 120,
            loop_in: Some(0.1),
            loop_out: Some(0.4),
            looping: true,
            ..Studio::default()
        };
        export_wav(&wav, studio, Some(Arc::new(clip)), WavOptions::default())?;
        let decoded = decode_path(&wav)?;
        assert!(decoded.frames[50_000][0] > 0.29);
        Ok(())
    }

    #[test]
    #[ignore = "downloads one public Commons recording"]
    fn commons_discovery_returns_playable_audio() -> Result<()> {
        let records = online_records("All")?;
        let (clip, _) = load(&records[0])?;
        assert!(clip.duration() >= 12.0);
        assert!(clip.peaks.iter().any(|&peak| peak > 0.02));
        Ok(())
    }
}
