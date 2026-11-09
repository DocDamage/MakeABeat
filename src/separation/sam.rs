use crate::{
    audio::Clip,
    background,
    files::{self, Result},
    subprocess::{self, Capture},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

pub const MODEL: &str = "facebook/sam-audio-small";
pub const REVISION: &str = "20b65f56888142eebe7c37448c6f6b3b32600e9b";
pub const MAX_PROMPT: usize = 200;
const SCRIPT: &str = include_str!("../../workers/audio_analysis/sam_audio_probe.py");
const RATE: u32 = 48_000;
pub const MAX_SECONDS: usize = 6;
const NAMES: [&str; 3] = ["original.wav", "target.wav", "residual.wav"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Original,
    Target,
    Residual,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub rate: u32,
}

impl Span {
    pub fn new(clip: &Clip, range: (f64, f64)) -> Result<Self> {
        let frames = clip
            .region(range)
            .ok_or("Select a non-empty audio passage.")?;
        let rate = clip.rate;
        if frames.len() < (rate as usize).div_ceil(10) || frames.len() > rate as usize * MAX_SECONDS
        {
            return Err("Select between 0.10 and 6 seconds of audio.".into());
        }
        let start = (range.0 * f64::from(rate))
            .round()
            .clamp(0.0, clip.frames.len() as f64) as usize;
        Ok(Self {
            start,
            end: start + frames.len(),
            rate,
        })
    }

    fn seconds(self) -> (f64, f64) {
        let rate = f64::from(self.rate);
        (self.start as f64 / rate, self.end as f64 / rate)
    }

    fn output_frames(self) -> usize {
        ((self.end - self.start) as u64 * u64::from(RATE)).div_ceil(u64::from(self.rate)) as usize
    }
}

pub fn local_setup() -> (Option<PathBuf>, Option<PathBuf>) {
    let Some(root) = files::app_data_dir().map(|path| path.join("sam-audio-phase8")) else {
        return (None, None);
    };
    let python = root.join("venv").join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    let ffmpeg = root.join("ffmpeg-8.1.2-full_build-shared/bin");
    (
        python.is_file().then_some(python),
        ffmpeg.is_dir().then_some(ffmpeg),
    )
}

pub fn extract(
    python: &Path,
    ffmpeg: Option<&Path>,
    clip: &Clip,
    span: Span,
    prompt: &str,
) -> Result<[Arc<Clip>; 3]> {
    let work = tempfile::tempdir()?;
    let input = work.path().join("input.wav");
    let output = work.path().join("output");
    files::export_region(&input, clip, span.seconds())?;
    background::stage("Loading SAM Audio and extracting")?;
    let mut command = Command::new(python);
    command
        .args(["-u", "-c", SCRIPT, "--"])
        .arg(input)
        .arg(prompt)
        .arg(&output)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("XFORMERS_FORCE_DISABLE_TRITON", "1");
    if let Some(ffmpeg) = ffmpeg {
        let path = std::env::var_os("PATH").unwrap_or_default();
        command.env(
            "PATH",
            std::env::join_paths(
                std::iter::once(ffmpeg.to_path_buf()).chain(std::env::split_paths(&path)),
            )?,
        );
    }
    let result = subprocess::run(
        &mut command,
        "SAM Audio",
        Duration::from_secs(120),
        Capture {
            stderr: 4096,
            ..Capture::default()
        },
    )?;
    if !result.status.success() {
        return Err(worker_error(&result.stderr).into());
    }
    background::stage("Validating original, target, and residual")?;
    read_outputs(&output, span.output_frames())
}

fn worker_error(log: &str) -> String {
    let detail = log
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let hint = if log.contains("LocalEntryNotFoundError") || log.contains("couldn't connect") {
        "Cache the approved SAM Audio Small checkpoint and T5 encoder using the setup guide."
    } else if log.contains("OutOfMemoryError") {
        "SAM Audio ran out of GPU memory. Try a shorter passage or free GPU memory."
    } else if log.contains("torchcodec") || log.contains("FFmpeg") {
        "Check TorchCodec and select the shared FFmpeg bin folder in SAM Audio setup."
    } else {
        "Check the SAM Audio Python and CUDA setup."
    };
    format!(
        "SAM Audio could not extract this passage. {hint} {}",
        detail.chars().take(350).collect::<String>()
    )
}

fn read_outputs(directory: &Path, expected: usize) -> Result<[Arc<Clip>; 3]> {
    let read = |name| -> Result<Arc<Clip>> {
        background::check()?;
        let path = directory.join(name);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_file() || meta.len() > (expected * 4 + 4096) as u64 {
            return Err("SAM Audio returned an invalid WAV file.".into());
        }
        let mut wav = hound::WavReader::open(path)?;
        let spec = wav.spec();
        if spec.channels != 1
            || spec.sample_rate != RATE
            || spec.bits_per_sample != 32
            || spec.sample_format != hound::SampleFormat::Float
            || wav.duration() as usize != expected
        {
            return Err("SAM Audio outputs do not match the selection length or format.".into());
        }
        let mut frames = Vec::with_capacity(expected);
        for sample in wav.samples::<f32>() {
            let sample = sample?;
            if !sample.is_finite() {
                return Err("SAM Audio returned non-finite samples.".into());
            }
            frames.push([sample.clamp(-1.0, 1.0); 2]);
        }
        Ok(Arc::new(Clip::new(RATE, frames)))
    };
    let [original, target, residual] = NAMES;
    Ok([read(original)?, read(target)?, read(residual)?])
}
