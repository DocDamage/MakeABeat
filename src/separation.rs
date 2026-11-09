use crate::{
    audio::Clip,
    files::{self, Result},
    subprocess::{self, Capture},
};
use std::{path::Path, process::Command, time::Duration};

pub mod sam;

pub const STEMS: [&str; 4] = ["vocals", "drums", "bass", "other"];

pub fn separate(
    program: Option<&Path>,
    clip: &Clip,
    range: (f64, f64),
    limit: usize,
) -> Result<Vec<(&'static str, Clip)>> {
    let work = tempfile::tempdir()?;
    let input = work.path().join("selection.wav");
    crate::background::stage("Preparing stem separation input")?;
    files::export_region(&input, clip, range)?;
    crate::background::stage("Separating audio in Demucs")?;
    let mut command = Command::new(program.unwrap_or_else(|| Path::new("demucs")));
    command
        .arg("-n")
        .arg("htdemucs")
        .arg("-o")
        .arg(work.path())
        .arg("--filename")
        .arg("{stem}.{ext}")
        .arg(&input);
    let subprocess::Output { status, stderr, .. } = subprocess::run(
        &mut command,
        "Demucs",
        Duration::from_secs(900),
        Capture {
            stderr: 2500,
            ..Capture::default()
        },
    )?;
    if !status.success() {
        return Err(format!(
            "Demucs exited with {status}. {}",
            stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default()
        )
        .into());
    }
    let output = work.path().join("htdemucs");
    let mut remaining = limit;
    let mut stems = Vec::new();
    for stem in STEMS {
        crate::background::check()?;
        let clip = files::decode_path(&output.join(format!("{stem}.wav")))?;
        remaining = remaining
            .checked_sub(clip.frames.len())
            .ok_or("The separated stems would exceed the project audio limit.")?;
        stems.push((stem, clip));
    }
    Ok(stems)
}
