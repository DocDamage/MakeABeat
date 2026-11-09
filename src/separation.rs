use crate::{
    audio::Clip,
    files::{self, Result},
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

pub const STEMS: [&str; 4] = ["vocals", "drums", "bass", "other"];

pub fn separate(
    program: Option<&Path>,
    clip: &Clip,
    range: (f64, f64),
    limit: usize,
) -> Result<Vec<(&'static str, Clip)>> {
    let work = tempfile::tempdir()?;
    let input = work.path().join("selection.wav");
    let log = work.path().join("demucs.log");
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
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|error| format!(
        "Could not start Demucs. Install it locally and select its executable in Stem separation. {error}"
    ))?;
    let status = loop {
        if let Err(error) = crate::background::check() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    };
    if !status.success() {
        let mut log = File::open(log)?;
        let start = log.metadata()?.len().saturating_sub(2500);
        log.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        log.take(2500).read_to_end(&mut bytes)?;
        let message = String::from_utf8_lossy(&bytes);
        return Err(format!(
            "Demucs exited with {status}. {}",
            message
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
