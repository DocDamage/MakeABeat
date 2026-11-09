use crate::{
    audio::Clip,
    background,
    files::{self, Result},
    model::{
        beat_map::{Beat, MAX_BEATS, Provenance},
        snapshot::SourceId,
    },
};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const VERSION: u16 = 1;
const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 64;
pub const MAX_SECONDS: f64 = 180.0;
pub const CHECKPOINT_BYTES: usize = 8_451_101;
const CHECKPOINT_HASH: &str = "6074be2c4d490c5f6101fcc374a1ec72ae93456e23bb6019783b849f5dc7d47b";
const CHECKPOINT_URL: &str =
    "https://cloud.cp.jku.at/public.php/dav/files/7ik4RrBKTS273gp/small0.ckpt";
const SCRIPT: &str = include_str!("../workers/audio_analysis/beat_this_worker.py");

#[derive(Clone)]
pub enum Model {
    Pulse,
    BeatThis(PathBuf),
}

impl Model {
    fn identity(&self) -> (&'static str, &'static str) {
        match self {
            Self::Pulse => ("lost-samples-pulse", "1"),
            Self::BeatThis(_) => ("beat-this", "1.1.0-small0-v1"),
        }
    }

    fn command(&self, checkpoint: &Path) -> Result<Command> {
        match self {
            Self::Pulse => {
                let mut command = Command::new(std::env::current_exe()?);
                command.arg("--audio-analysis-worker");
                Ok(command)
            }
            Self::BeatThis(python) => {
                let mut command = Command::new(python);
                command.args(["-I", "-u", "-c", SCRIPT]).arg(checkpoint);
                Ok(command)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobKey {
    pub job_id: u64,
    pub project_revision: u64,
    pub source_id: Option<SourceId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Identity {
    project_revision: u64,
    source_fingerprint: String,
    start_frame: usize,
    end_frame: usize,
    model_id: String,
    model_version: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    bpm_hint: u16,
}

#[derive(Serialize, Deserialize)]
struct Request {
    version: u16,
    #[serde(rename = "type")]
    kind: String,
    job_id: u64,
    source_id: Option<SourceId>,
    #[serde(flatten)]
    identity: Identity,
    operation: String,
    sample_rate: u32,
    input_path: PathBuf,
    output_dir: PathBuf,
    settings: Settings,
    timeout_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Cancel {
    version: u16,
    #[serde(rename = "type")]
    kind: String,
    job_id: u64,
}

#[derive(Serialize, Deserialize)]
struct Message {
    version: u16,
    job_id: u64,
    #[serde(flatten)]
    body: Body,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Body {
    Progress {
        stage: String,
        completed: usize,
        total: usize,
    },
    Result {
        #[serde(flatten)]
        identity: Identity,
        beats: Vec<Beat>,
    },
    Failure {
        code: String,
        message: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub beats: Vec<Beat>,
    pub provenance: Provenance,
}

pub fn fingerprint(clip: &Clip) -> Result<String> {
    fingerprint_frames(clip.rate, &clip.frames)
}

pub fn fingerprint_frames(rate: u32, frames: &[crate::audio::Frame]) -> Result<String> {
    let mut digest = Context::new(&SHA256);
    digest.update(b"lost-samples-stereo-f32le-v1");
    digest.update(&rate.to_le_bytes());
    let mut bytes = [0_u8; 4096 * 8];
    for block in frames.chunks(4096) {
        background::check()?;
        for (frame, slot) in block.iter().zip(bytes.chunks_exact_mut(8)) {
            slot[..4].copy_from_slice(&frame[0].to_le_bytes());
            slot[4..].copy_from_slice(&frame[1].to_le_bytes());
        }
        digest.update(&bytes[..block.len() * 8]);
    }
    Ok(hex(digest.finish().as_ref()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn storage() -> Option<PathBuf> {
    files::app_data_dir().map(|path| path.join("audio-analysis-v1"))
}

fn checkpoint_path() -> PathBuf {
    storage()
        .map(|dir| dir.join("beat-this-small0.ckpt"))
        .unwrap_or_default()
}

pub fn checkpoint_present() -> bool {
    checkpoint_path()
        .metadata()
        .is_ok_and(|metadata| metadata.len() == CHECKPOINT_BYTES as u64)
}

fn verify_checkpoint(path: &Path) -> Result<()> {
    let mut bytes = Vec::with_capacity(CHECKPOINT_BYTES);
    File::open(path)?
        .take(CHECKPOINT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() != CHECKPOINT_BYTES
        || hex(ring::digest::digest(&SHA256, &bytes).as_ref()) != CHECKPOINT_HASH
    {
        return Err(
            "Beat This! weights failed the SHA-256 check. Download the verified weights again."
                .into(),
        );
    }
    Ok(())
}

pub fn download_checkpoint() -> Result<()> {
    let path = checkpoint_path();
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or("Application data storage is unavailable.")?;
    fs::create_dir_all(directory)?;
    if verify_checkpoint(&path).is_ok() {
        return Ok(());
    }
    background::stage("Downloading Beat This! weights")?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(45))
        .build();
    let response = agent.get(CHECKPOINT_URL).call()?;
    let mut reader = response.into_reader().take(CHECKPOINT_BYTES as u64 + 1);
    let mut pending = tempfile::NamedTempFile::new_in(directory)?;
    let mut block = [0_u8; 32 * 1024];
    let mut total = 0;
    loop {
        background::progress(total, CHECKPOINT_BYTES)?;
        let size = reader.read(&mut block)?;
        if size == 0 {
            break;
        }
        pending.write_all(&block[..size])?;
        total += size;
    }
    pending.flush()?;
    verify_checkpoint(pending.path())?;
    background::check()?;
    pending.persist(path)?;
    Ok(())
}

pub fn analyze(
    model: &Model,
    key: JobKey,
    clip: &Clip,
    range: (f64, f64),
    bpm_hint: u16,
) -> Result<Proposal> {
    if !(range.0.is_finite()
        && range.1.is_finite()
        && range.0 >= 0.0
        && range.1 <= clip.duration()
        && (0.2..=MAX_SECONDS).contains(&(range.1 - range.0)))
    {
        return Err("Select between 0.2 seconds and 3 minutes of source audio.".into());
    }
    background::stage("Identifying source audio")?;
    let (model_id, model_version) = model.identity();
    let start_frame = (range.0 * f64::from(clip.rate)).round() as usize;
    let end_frame = (range.1 * f64::from(clip.rate)).round() as usize;
    let fingerprint = fingerprint(clip)?;
    let provenance = Provenance {
        model: model_id.into(),
        version: model_version.into(),
        fingerprint: fingerprint.clone(),
        range: (
            start_frame as f64 / f64::from(clip.rate),
            end_frame as f64 / f64::from(clip.rate),
        ),
    };
    let cache_key =
        format!("{fingerprint}-{start_frame}-{end_frame}-{model_id}-{model_version}-{bpm_hint}");
    let cache = storage().map(|dir| dir.join("beat-cache").join(format!("{cache_key}.json")));
    if let Some(proposal) = cache
        .as_deref()
        .and_then(|path| read_cache(path, &provenance))
    {
        return Ok(proposal);
    }
    let checkpoint = checkpoint_path();
    if matches!(model, Model::BeatThis(_)) {
        verify_checkpoint(&checkpoint).map_err(|error| {
            format!("Beat This! weights are unavailable. Use Download weights in Beat map. {error}")
        })?;
    }
    let work = tempfile::tempdir()?;
    let request = Request {
        version: VERSION,
        kind: "run".into(),
        job_id: key.job_id,
        source_id: key.source_id,
        identity: Identity {
            project_revision: key.project_revision,
            source_fingerprint: fingerprint,
            start_frame,
            end_frame,
            model_id: model_id.into(),
            model_version: model_version.into(),
        },
        operation: "beats".into(),
        sample_rate: clip.rate,
        input_path: work.path().join("input.wav"),
        output_dir: work.path().to_owned(),
        settings: Settings { bpm_hint },
        timeout_ms: 120_000,
    };
    background::stage("Writing worker audio")?;
    files::export_region(&request.input_path, clip, provenance.range)?;
    let mut beats = supervise(model.command(&checkpoint)?, &request)?;
    let span = provenance.range.1 - provenance.range.0;
    if !valid_beats(&beats, 0.0, span + 0.001) {
        return Err("The worker returned an invalid beat map.".into());
    }
    beats.retain(|beat| beat.seconds <= span);
    for beat in &mut beats {
        beat.seconds += provenance.range.0;
    }
    if beats.is_empty() {
        return Err(
            "No beats found. Try the fallback, a clearer passage, or add manual beats.".into(),
        );
    }
    let proposal = Proposal { beats, provenance };
    background::check()?;
    if let Some(path) = cache {
        let _ = write_cache(&path, &proposal);
    }
    Ok(proposal)
}

pub fn probe(python: &Path) -> Result<()> {
    let work = tempfile::tempdir()?;
    let model = Model::BeatThis(python.to_owned());
    let (model_id, model_version) = model.identity();
    let request = Request {
        version: VERSION,
        kind: "run".into(),
        job_id: 0,
        source_id: None,
        identity: Identity {
            project_revision: 0,
            source_fingerprint: String::new(),
            start_frame: 0,
            end_frame: 0,
            model_id: model_id.into(),
            model_version: model_version.into(),
        },
        operation: "probe".into(),
        sample_rate: 0,
        input_path: PathBuf::new(),
        output_dir: work.path().to_owned(),
        settings: Settings { bpm_hint: 120 },
        timeout_ms: 30_000,
    };
    supervise(model.command(&checkpoint_path())?, &request)?;
    Ok(())
}

fn valid_beats(beats: &[Beat], start: f64, end: f64) -> bool {
    beats.len() <= MAX_BEATS
        && beats
            .windows(2)
            .all(|pair| pair[0].seconds < pair[1].seconds)
        && beats.iter().all(|beat| {
            (start..=end).contains(&beat.seconds)
                && !beat.anchor
                && beat
                    .confidence
                    .is_none_or(|value| (0.0..=1.0).contains(&value))
        })
}

fn read_cache(path: &Path, provenance: &Provenance) -> Option<Proposal> {
    let input = File::open(path).ok()?.take(MAX_MESSAGE as u64 + 1);
    let proposal: Proposal = serde_json::from_reader(input).ok()?;
    (proposal.provenance == *provenance
        && !proposal.beats.is_empty()
        && valid_beats(
            &proposal.beats,
            provenance.range.0,
            provenance.range.1 + 1e-8,
        ))
    .then_some(proposal)
}

fn write_cache(path: &Path, proposal: &Proposal) -> Result<()> {
    let dir = path.parent().ok_or("Cache directory unavailable.")?;
    fs::create_dir_all(dir)?;
    let mut output = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut output, proposal)?;
    output.flush()?;
    output.persist(path)?;
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            (entry.file_type().ok()?.is_file()
                && entry.path().extension().is_some_and(|ext| ext == "json"))
            .then(|| {
                (
                    entry.metadata().ok().and_then(|meta| meta.modified().ok()),
                    entry.path(),
                )
            })
        })
        .collect();
    entries.sort_unstable();
    for (_, old) in entries
        .iter()
        .filter(|(_, old)| old != path)
        .take(entries.len().saturating_sub(MAX_CACHE_ENTRIES))
    {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

struct Worker {
    child: Child,
    #[cfg(windows)]
    _job: Option<std::os::windows::io::OwnedHandle>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(windows)]
pub(crate) fn bind_worker_tree(child: &Child) -> Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::{
        Win32::{
            Foundation::HANDLE,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        },
        core::PCWSTR,
    };
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let owner = OwnedHandle::from_raw_handle(job.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
        AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()))?;
        Ok(owner)
    }
}

fn supervise(mut command: Command, request: &Request) -> Result<Vec<Beat>> {
    background::stage("Starting local analysis worker")?;
    let log = request.output_dir.join("worker.log");
    let python = request.identity.model_id == "beat-this";
    if python {
        // The Windows venv launcher can buffer redirected stdin until EOF.
        // Use file transport and supervise the Python process tree directly.
        let path = request.output_dir.join("request.json");
        serde_json::to_writer(File::create(&path)?, request)?;
        command.arg(path);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(File::create(&log)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut worker = Worker {
        child: command
            .spawn()
            .map_err(|error| format!("Could not start local analysis worker. {error}"))?,
        #[cfg(windows)]
        _job: None,
    };
    #[cfg(windows)]
    if python {
        worker._job =
            Some(bind_worker_tree(&worker.child).map_err(|error| {
                format!("Could not supervise the Python process tree. {error}")
            })?);
    }
    let mut input = worker
        .child
        .stdin
        .take()
        .ok_or("Missing worker input pipe.")?;
    if !python {
        serde_json::to_writer(&mut input, request)?;
        input.write_all(b"\n")?;
    }
    let mut input = (!python).then_some(input);
    let output = worker
        .child
        .stdout
        .take()
        .ok_or("Missing worker output pipe.")?;
    let (tx, rx) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        loop {
            let result = read_message::<Message>(&mut reader);
            let done = !matches!(result, Ok(Some(_)));
            if tx.send(result).is_err() || done {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_millis(request.timeout_ms);
    let mut terminal = None;
    let mut contacted = false;
    loop {
        if let Err(error) = background::check() {
            if contacted && let Some(input) = input.take() {
                signal_cancel(input, request.job_id, &mut worker.child);
            }
            return Err(error.into());
        }
        if Instant::now() >= deadline {
            if contacted && let Some(input) = input.take() {
                signal_cancel(input, request.job_id, &mut worker.child);
            }
            return Err("Local analysis timed out.".into());
        }
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(Ok(Some(message))) => {
                if message.version != VERSION
                    || message.job_id != request.job_id
                    || terminal.is_some()
                {
                    return Err("Local analysis returned an unexpected protocol message.".into());
                }
                contacted = true;
                match message.body {
                    Body::Progress {
                        stage,
                        completed,
                        total,
                    } => {
                        let stage = match stage.as_str() {
                            "Reading source" => "Reading worker audio",
                            "Following transients" => "Following transients",
                            "Loading checkpoint" => "Loading verified checkpoint",
                            "Tracking beats" => "Tracking beats",
                            _ => "Running local analysis",
                        };
                        background::stage(stage)?;
                        background::progress(completed.min(total), total)?;
                    }
                    Body::Failure { code, message } => {
                        return Err(format!("{message} ({code})").into());
                    }
                    Body::Result { identity, beats } if identity == request.identity => {
                        terminal = Some(beats);
                        // Release the cancellation listener before waiting for worker shutdown.
                        drop(input.take());
                    }
                    Body::Result { .. } => {
                        return Err("Analysis result does not match its request.".into());
                    }
                }
            }
            Ok(Err(error)) => return Err(error.into()),
            Ok(Ok(None)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(exit) = worker.child.try_wait()? {
                    if exit.success()
                        && let Some(beats) = terminal
                    {
                        return Ok(beats);
                    }
                    return Err(format!(
                        "Local analysis worker ended without a usable result ({exit})."
                    )
                    .into());
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn signal_cancel(mut input: impl Write, job_id: u64, child: &mut Child) {
    let message = Cancel {
        version: VERSION,
        kind: "cancel".into(),
        job_id,
    };
    if serde_json::to_writer(&mut input, &message).is_ok() && input.write_all(b"\n").is_ok() {
        drop(input);
        let deadline = Instant::now() + Duration::from_millis(120);
        while Instant::now() < deadline && child.try_wait().ok().flatten().is_none() {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

fn read_message<T: for<'a> Deserialize<'a>>(input: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = Vec::new();
    let length = input
        .take(MAX_MESSAGE as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if length == 0 {
        return Ok(None);
    }
    if length > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Local analysis message is too large.",
        ));
    }
    serde_json::from_slice(&line).map(Some).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid analysis protocol message: {error}"),
        )
    })
}

pub fn entry() -> Result<()> {
    let request: Request =
        read_message(&mut io::stdin().lock())?.ok_or("Missing analysis request.")?;
    let send = |body| -> Result<()> {
        let mut output = io::stdout().lock();
        serde_json::to_writer(
            &mut output,
            &Message {
                version: VERSION,
                job_id: request.job_id,
                body,
            },
        )?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    };
    let model = Model::Pulse.identity();
    if request.version != VERSION
        || request.kind != "run"
        || request.operation != "beats"
        || request.identity.model_id != model.0
        || request.identity.model_version != model.1
    {
        return send(Body::Failure {
            code: "invalid_request".into(),
            message: "Unsupported pulse worker request.".into(),
        });
    }
    let control = background::Control::default();
    let listener = control.clone();
    let job_id = request.job_id;
    std::thread::spawn(move || {
        if let Ok(Some(cancel)) = read_message::<Cancel>(&mut io::stdin().lock())
            && cancel.version == VERSION
            && cancel.kind == "cancel"
            && cancel.job_id == job_id
        {
            listener.cancel();
        }
    });
    send(Body::Progress {
        stage: "Reading source".into(),
        completed: 1,
        total: 3,
    })?;
    let result = control.run(|| {
        files::decode_path(&request.input_path).and_then(|clip| {
            send(Body::Progress {
                stage: "Following transients".into(),
                completed: 2,
                total: 3,
            })?;
            crate::tempo::beat_grid(&clip, request.settings.bpm_hint)
        })
    });
    match result {
        Ok(beats) => send(Body::Result {
            identity: request.identity.clone(),
            beats,
        }),
        Err(error) => send(Body::Failure {
            code: "inference_failure".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an isolated Python environment and a network checkpoint download"]
    fn beat_this_setup_and_analysis_with_verified_weights() -> Result<()> {
        let python = std::env::var_os("LOST_SAMPLES_BEAT_PYTHON")
            .ok_or("Set LOST_SAMPLES_BEAT_PYTHON first.")?;
        let python = PathBuf::from(python);
        probe(&python).map_err(|error| format!("Environment probe failed. {error}"))?;
        download_checkpoint().map_err(|error| format!("Checkpoint setup failed. {error}"))?;
        verify_checkpoint(&checkpoint_path())?;
        let fixture = std::env::var_os("LOST_SAMPLES_BEAT_FIXTURE")
            .ok_or("Set LOST_SAMPLES_BEAT_FIXTURE first.")?;
        let clip = files::decode_path(Path::new(&fixture))?;
        let proposal = analyze(
            &Model::BeatThis(python),
            JobKey {
                job_id: 1,
                project_revision: 2,
                source_id: None,
            },
            &clip,
            (0.0, clip.duration()),
            120,
        )?;
        assert!(proposal.beats.len() > 4);
        assert!(proposal.beats.iter().any(|beat| beat.downbeat));
        Ok(())
    }
}
