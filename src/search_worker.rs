use crate::{audio::Clip, background, files::Result, subprocess::Process};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

const SCRIPT: &str = include_str!("../workers/audio_analysis/clap_worker.py");
const REVISION: &str = "8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a";
pub const MODEL: &str = "clap-8fa0f1c-mono48k-repeatpad-w2x1-8x4-v1";
pub const DIMENSIONS: usize = 512;
pub const BATCH: usize = 8;

#[derive(Deserialize)]
struct Reply {
    ready: Option<bool>,
    revision: Option<String>,
    vectors: Option<Vec<Vec<f32>>>,
    error: Option<String>,
}

pub struct Worker {
    python: PathBuf,
    _process: Process,
    input: ChildStdin,
    replies: Receiver<String>,
}

impl Worker {
    fn spawn(python: &Path, setup: bool) -> Result<Self> {
        let mut command = Command::new(python);
        command.args(["-u", "-c", SCRIPT]);
        if setup {
            command.arg("--setup");
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut process = Process::spawn(&mut command)
            .map_err(|error| format!("Could not start the CLAP Python environment. {error}"))?;
        let input = process.child.stdin.take().unwrap();
        let output = process.child.stdout.take().unwrap();
        let (send, replies) = mpsc::sync_channel(2);
        std::thread::spawn(move || {
            for line in BufReader::new(output)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let worker = Self {
            python: python.to_owned(),
            _process: process,
            input,
            replies,
        };
        let reply = worker.receive(if setup { 900 } else { 60 }, true)?;
        if reply.ready != Some(true) || reply.revision.as_deref() != Some(REVISION) {
            return Err("Unexpected CLAP worker version.".into());
        }
        Ok(worker)
    }

    fn receive(&self, seconds: u64, cancellable: bool) -> Result<Reply> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            if cancellable {
                background::check()?;
            }
            match self.replies.recv_timeout(Duration::from_millis(25)) {
                Ok(line) => {
                    let reply: Reply = serde_json::from_str(&line)?;
                    if let Some(error) = reply.error {
                        return Err(error.into());
                    }
                    return Ok(reply);
                }
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => (),
                Err(mpsc::RecvTimeoutError::Timeout) => return Err("CLAP worker timed out.".into()),
                Err(_) => return Err("CLAP worker exited before returning a result.".into()),
            }
        }
    }

    pub fn setup(python: &Path) -> Result<()> {
        background::stage("Downloading pinned CLAP model")?;
        Self::spawn(python, true)?;
        Ok(())
    }

    pub fn with<T>(
        slot: &mut Option<Self>,
        python: &Path,
        work: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        if slot.as_ref().is_none_or(|worker| worker.python != python) {
            background::stage("Loading local CLAP model")?;
            *slot = Some(Self::spawn(python, false)?);
        }
        let result = work(slot.as_mut().unwrap());
        if result.is_err() {
            *slot = None;
        }
        result
    }

    fn vectors(&mut self, request: Value, count: usize) -> Result<Vec<Vec<f32>>> {
        background::check()?;
        serde_json::to_writer(&mut self.input, &request)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        // Finish the bounded batch before yielding so a search can reuse the warm model.
        let vectors = self
            .receive(60, false)?
            .vectors
            .ok_or("CLAP returned no embeddings.")?;
        if vectors.len() != count
            || vectors.iter().any(|vector| {
                vector.len() != DIMENSIONS
                    || vector.iter().any(|value| !value.is_finite())
                    || !(0.98..=1.02)
                        .contains(&vector.iter().map(|value| value * value).sum::<f32>())
            })
        {
            return Err("CLAP returned invalid embeddings.".into());
        }
        Ok(vectors)
    }

    pub fn text(&mut self, text: &str) -> Result<Vec<f32>> {
        Ok(self.vectors(json!({ "text": text }), 1)?.pop().unwrap())
    }

    pub fn audio(&mut self, clip: &Clip, ranges: &[(f64, f64)]) -> Result<Vec<Vec<f32>>> {
        let mut file = tempfile::NamedTempFile::new()?;
        let mut lengths = Vec::with_capacity(ranges.len());
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            for &range in ranges {
                background::check()?;
                let frames = clip.region(range).ok_or("Empty passage.")?;
                lengths.push(frames.len());
                for block in frames.chunks(4096) {
                    background::check()?;
                    for [left, right] in block {
                        writer.write_all(&((left + right) * 0.5).to_le_bytes())?;
                    }
                }
            }
            writer.flush()?;
        }
        self.vectors(
            json!({ "path": file.path(), "rate": clip.rate, "lengths": lengths }),
            ranges.len(),
        )
    }
}
