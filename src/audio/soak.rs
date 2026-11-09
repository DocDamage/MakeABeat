use super::*;
use crate::{audition, files, library_index, tempo};
use cpal::Sample;
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
use windows::Win32::System::Threading::THREAD_PRIORITY_TIME_CRITICAL;

const PILOT_HZ: f32 = 6_000.0;
const REFERENCE_HZ: f32 = 4_700.0;
const PILOT_LEVEL: f32 = 0.004;

#[derive(Default)]
struct Loopback {
    windows: AtomicU64,
    weak: AtomicU64,
    weak_audition: AtomicU64,
    weak_reference: AtomicU64,
    capture_gaps: AtomicU64,
    recent_level: AtomicU64,
    threshold: AtomicU64,
    thread_priority: AtomicI32,
    failed: AtomicBool,
}

fn phase(frame: usize, rate: u32, hz: f32) -> f32 {
    TAU * (frame as f32 * hz / rate as f32).fract()
}

struct Signal {
    basis: Vec<[(f32, f32); 2]>,
    index: usize,
    sine: [f32; 2],
    cosine: [f32; 2],
}

impl Signal {
    fn new(rate: u32) -> Self {
        Self {
            basis: (0..rate as usize / 100)
                .map(|i| {
                    [
                        phase(i, rate, PILOT_HZ).sin_cos(),
                        phase(i, rate, REFERENCE_HZ).sin_cos(),
                    ]
                })
                .collect(),
            index: 0,
            sine: [0.0; 2],
            cosine: [0.0; 2],
        }
    }

    fn sample(&mut self, sample: f32) -> Option<[f32; 2]> {
        for (band, (sin, cos)) in self.basis[self.index].into_iter().enumerate() {
            self.sine[band] += sample * sin;
            self.cosine[band] += sample * cos;
        }
        self.index += 1;
        if self.index < self.basis.len() {
            return None;
        }
        let level = std::array::from_fn(|band| {
            self.sine[band].hypot(self.cosine[band]) * 2.0 / self.basis.len() as f32
        });
        self.index = 0;
        self.sine = [0.0; 2];
        self.cosine = [0.0; 2];
        Some(level)
    }
}

fn pack([a, b]: [f32; 2]) -> u64 {
    u64::from(a.to_bits()) | (u64::from(b.to_bits()) << 32)
}

fn unpack(bits: u64) -> [f32; 2] {
    [
        f32::from_bits(bits as u32),
        f32::from_bits((bits >> 32) as u32),
    ]
}

struct Diagnostics {
    start: Instant,
    events: mpsc::SyncSender<Event>,
}

enum Event {
    RenderGap(f64, f64),
    CaptureGap(f64, f64),
    Weak(f64, [f32; 2]),
    Overrun {
        at: f64,
        elapsed_ns: u64,
        frames: usize,
        commands: Duration,
        mix: Duration,
        cycles: u64,
        normal: u64,
    },
}

impl Event {
    fn print(self) {
        match self {
            Self::RenderGap(at, gap) => {
                println!("Render callback gap at {at:.3}s: {:.3} ms", gap * 1e3)
            }
            Self::CaptureGap(at, excess) => println!(
                "Capture timestamp gap at {at:.3}s: {:.3} ms beyond expected",
                excess * 1e3
            ),
            Self::Weak(at, [audition, reference]) => println!(
                "Weak loopback window at {at:.3}s: audition {audition:.5}, reference {reference:.5}"
            ),
            Self::Overrun {
                at,
                elapsed_ns,
                frames,
                commands,
                mix,
                cycles,
                normal,
            } => println!(
                "Render overrun at {at:.3}s: {:.3} ms for {frames} frames, commands {:.3} ms, mix {:.3} ms, {cycles} thread cycles (smoothed normal {normal})",
                elapsed_ns as f64 / 1e6,
                commands.as_secs_f64() * 1e3,
                mix.as_secs_f64() * 1e3,
            ),
        }
    }
}

pub(super) struct RenderProbe {
    active: AtomicBool,
    weak_audition: AtomicU64,
    weak_reference: AtomicU64,
    callback_gaps: AtomicU64,
    typical_cycles: AtomicU64,
    thread_priority: AtomicI32,
    diagnostics: Arc<Diagnostics>,
}

pub(super) struct RenderState {
    signal: Signal,
    previous: Option<(Instant, usize)>,
}

pub(super) struct CallbackMeasurement {
    pub started: Instant,
    pub frames: usize,
    pub rate: u32,
    pub commands_elapsed: Duration,
    pub mix_elapsed: Duration,
    pub elapsed_ns: u64,
    pub cycles_start: u64,
}

pub(super) fn thread_cycles() -> u64 {
    use windows::Win32::System::{
        Threading::GetCurrentThread, WindowsProgramming::QueryThreadCycleTime,
    };
    let mut cycles = 0;
    let _ = unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
    cycles
}

impl RenderState {
    pub(super) fn new(rate: u32) -> Self {
        Self {
            signal: Signal::new(rate),
            previous: None,
        }
    }

    pub(super) fn add_reference(
        &mut self,
        probe: &RenderProbe,
        left: f32,
        right: f32,
    ) -> (f32, f32) {
        // This reference bypasses the audition loop and its end fade, while the
        // original pilot and drums still exercise the production mixer.
        let reference = PILOT_LEVEL * self.signal.basis[self.signal.index][1].0;
        let mixed = (left + reference, right + reference);
        if let Some([audition, reference]) = self.signal.sample(mixed.0)
            && probe.active.load(Relaxed)
        {
            if audition < PILOT_LEVEL * 0.25 {
                probe.weak_audition.fetch_add(1, Relaxed);
            }
            if reference < PILOT_LEVEL * 0.25 {
                probe.weak_reference.fetch_add(1, Relaxed);
            }
        }
        mixed
    }

    pub(super) fn callback(&mut self, probe: &RenderProbe, measurement: CallbackMeasurement) {
        let CallbackMeasurement {
            started: start,
            frames,
            rate,
            commands_elapsed: commands,
            mix_elapsed: mixed,
            elapsed_ns,
            cycles_start,
        } = measurement;
        let previous = self.previous.replace((start, frames));
        if previous.is_none() {
            probe
                .thread_priority
                .store(current_thread_priority(), Relaxed);
        }
        if !probe.active.load(Relaxed) {
            return;
        }
        let cycles = thread_cycles().saturating_sub(cycles_start);
        let at = start
            .saturating_duration_since(probe.diagnostics.start)
            .as_secs_f64();
        if let Some((previous, frames)) = previous {
            let gap = start.duration_since(previous).as_secs_f64();
            if gap > frames as f64 / f64::from(rate) + 0.005 {
                let seen = probe.callback_gaps.fetch_add(1, Relaxed);
                if seen < 12 {
                    let _ = probe.diagnostics.events.try_send(Event::RenderGap(at, gap));
                }
            }
        }
        if elapsed_ns * u64::from(rate) > frames as u64 * 1_000_000_000 {
            let _ = probe.diagnostics.events.try_send(Event::Overrun {
                at,
                elapsed_ns,
                frames,
                commands,
                mix: mixed - commands,
                cycles,
                normal: probe.typical_cycles.load(Relaxed),
            });
        } else {
            let baseline = probe.typical_cycles.load(Relaxed);
            probe
                .typical_cycles
                .store((31 * baseline + cycles) / 32, Relaxed);
        }
    }
}

fn current_thread_priority() -> i32 {
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadPriority};
    unsafe { GetThreadPriority(GetCurrentThread()) }
}

fn capture_loopback<T: SizedSample + Copy>(
    device: &cpal::Device,
    config: &StreamConfig,
    stats: &Arc<Loopback>,
    diagnostics: Arc<Diagnostics>,
) -> Result<Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    let mut signal = Signal::new(rate);
    let stats = Arc::clone(stats);
    let errors = Arc::clone(&stats);
    let mut previous: Option<(cpal::StreamInstant, usize)> = None;
    device.build_input_stream(
        config,
        move |data: &[T], info| {
            if previous.is_none() {
                crate::platform::promote_audio_thread();
                stats
                    .thread_priority
                    .store(current_thread_priority(), Relaxed);
            }
            let threshold = unpack(stats.threshold.load(Relaxed));
            let capture = info.timestamp().capture;
            if let Some((last, frames)) = previous
                && threshold[0] > 0.0
                && let Some(gap) = capture.duration_since(&last)
            {
                let excess = gap.as_secs_f64() - frames as f64 / f64::from(rate);
                if excess > 0.005 && stats.capture_gaps.fetch_add(1, Relaxed) < 12 {
                    let _ = diagnostics.events.try_send(Event::CaptureGap(
                        diagnostics.start.elapsed().as_secs_f64(),
                        excess,
                    ));
                }
            }
            previous = Some((capture, data.len() / channels));
            for frame in data.chunks_exact(channels) {
                if let Some(level) = signal.sample(f32::from_sample(frame[0])) {
                    stats.recent_level.store(pack(level), Relaxed);
                    if threshold[0] > 0.0 {
                        stats.windows.fetch_add(1, Relaxed);
                        let [audition, reference] =
                            std::array::from_fn(|i| level[i] < threshold[i]);
                        if audition && stats.weak_audition.fetch_add(1, Relaxed) < 12 {
                            let _ = diagnostics.events.try_send(Event::Weak(
                                diagnostics.start.elapsed().as_secs_f64(),
                                level,
                            ));
                        }
                        if reference {
                            stats.weak_reference.fetch_add(1, Relaxed);
                        }
                        if audition && reference {
                            stats.weak.fetch_add(1, Relaxed);
                        }
                    }
                }
            }
        },
        move |error| {
            eprintln!("Loopback stream error: {error}");
            errors.failed.store(true, Relaxed);
        },
        None,
    )
}

#[derive(Default)]
struct Workload {
    cycles: AtomicUsize,
    passages: AtomicUsize,
    indexing_ms: AtomicU64,
}

fn workload(path: &Path, stop: Receiver<()>, progress: Arc<Workload>) -> files::Result<()> {
    let samples = path.join("samples");
    std::fs::create_dir(&samples)?;
    let mut index = library_index::Index::open(&path.join("index"))?;
    index.add_folder(&samples, "")?;
    let clap = std::env::var_os("LOST_SAMPLES_SEARCH_PYTHON").map(std::path::PathBuf::from);
    let interval = Duration::from_secs(if clap.is_some() { 2 } else { 15 });
    let mut worker = None;
    let mut completed = 0;
    let mut studio = Studio {
        bpm: 120,
        bars: 4,
        ..Studio::default()
    };
    for bar in &mut studio.hits[..4] {
        for step in [0, 4, 8, 12] {
            bar[0][step] = 2;
            bar[7][step] = 1;
        }
    }
    loop {
        let export = samples.join(format!("beat-{completed}.wav"));
        studio.bpm = 112 + (completed % 16) as u16;
        for step in 0..11 {
            // Every render must have new audio so the index cannot reuse earlier embeddings.
            studio.hits[0][6][step] = ((completed >> step) & 1) as u8;
        }
        files::export_wav(&export, studio.clone(), None, files::WavOptions::default())?;
        let clip = Arc::new(files::decode_path(&export)?);
        tempo::beat_grid(&clip, studio.bpm)?;
        let fit = audition::Cache::default().prepare(
            clip.clone(),
            (0.0, 6.0),
            None,
            Some(120),
            Some(3),
        )?;
        assert!(fit.fitted.is_some(), "{:?}", fit.status);
        files::save_project(
            &path.join("soak.flip"),
            &files::Project {
                studio: studio.clone(),
                ..files::Project::default()
            },
            Some(&clip),
        )?;
        index.scan()?;
        if let Some(python) = &clap {
            let started = Instant::now();
            index.index(&mut worker, python)?;
            let summary = index.summary()?;
            if summary.failed != 0
                || summary.pending != 0
                || summary.passages <= progress.passages.load(Relaxed)
            {
                return Err(format!(
                    "CLAP did not index the new render: {} pending, {} failed, {} total passages",
                    summary.pending, summary.failed, summary.passages
                )
                .into());
            }
            progress
                .indexing_ms
                .fetch_add(started.elapsed().as_millis() as u64, Relaxed);
            progress.passages.store(summary.passages, Relaxed);
        }
        completed += 1;
        progress.cycles.store(completed, Relaxed);
        if !matches!(
            stop.recv_timeout(interval),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            return Ok(());
        }
    }
}

#[test]
#[ignore = "30-minute Windows playback and WASAPI loopback run"]
fn windows_audio_dropout_soak() -> files::Result<()> {
    let seconds = std::env::var("LOST_SAMPLES_SOAK_SECONDS")
        .ok()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(1_800);
    assert!(seconds > 0);
    let output = Output::open(&OutputSettings::default())?;
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No loopback endpoint")?;
    let format = device.default_output_config()?;
    let rate = output.sample_rate;
    assert_eq!(format.sample_rate().0, rate);
    assert_eq!(rate % 100, 0, "Loopback needs whole 10 ms sample blocks");
    let stats = Arc::new(Loopback::default());
    let (events, received) = mpsc::sync_channel(64);
    let diagnostics = Arc::new(Diagnostics {
        start: Instant::now(),
        events,
    });
    assert!(
        output
            .meters
            .soak_probe
            .set(RenderProbe {
                active: AtomicBool::new(false),
                weak_audition: AtomicU64::new(0),
                weak_reference: AtomicU64::new(0),
                callback_gaps: AtomicU64::new(0),
                typical_cycles: AtomicU64::new(0),
                thread_priority: AtomicI32::new(i32::MAX),
                diagnostics: diagnostics.clone(),
            })
            .is_ok()
    );
    let render = output.meters.soak_probe.get().unwrap();
    let config = format.config();
    let stream = match format.sample_format() {
        SampleFormat::F32 => capture_loopback::<f32>(&device, &config, &stats, diagnostics)?,
        SampleFormat::I16 => capture_loopback::<i16>(&device, &config, &stats, diagnostics)?,
        other => return Err(format!("Unsupported loopback sample format: {other}").into()),
    };
    stream.play()?;

    let mut studio = Studio {
        bpm: 120,
        drum_level: 0.015,
        ..Studio::default()
    };
    for step in [0, 4, 8, 12] {
        studio.hits[0][0][step] = 2;
        studio.hits[0][7][step] = 1;
    }
    let pilot = Arc::new(Clip::new(
        rate,
        (0..rate as usize * 2)
            .map(|i| [phase(i % (rate / 100) as usize, rate, PILOT_HZ).sin() * PILOT_LEVEL; 2])
            .collect(),
    ));
    output.send(Command::Settings(Box::new(studio)));
    output.send(Command::Pattern(true));
    output.send(Command::BeatAuditionGain(1.0));
    output.queue_beat_audition(pilot, (0.0, 2.0), Some(1));
    let warmup = Instant::now();
    let mut calibration = [[0.0_f32; 75]; 2];
    let mut index = 0;
    while index < calibration[0].len() {
        if output.meters.beat_audition() == 2 {
            let levels = unpack(stats.recent_level.load(Relaxed));
            for (band, level) in levels.into_iter().enumerate() {
                calibration[band][index] = level;
            }
            index += 1;
        }
        assert!(
            warmup.elapsed() < Duration::from_secs(8),
            "No loopback pilot. Check output mute, volume, and device."
        );
        thread::sleep(Duration::from_millis(20));
    }
    let level = calibration.map(|mut band| {
        band.sort_unstable_by(f32::total_cmp);
        band[band.len() / 2]
    });
    for (thread, priority) in [
        ("render", render.thread_priority.load(Relaxed)),
        ("loopback capture", stats.thread_priority.load(Relaxed)),
    ] {
        assert_eq!(
            priority, THREAD_PRIORITY_TIME_CRITICAL.0,
            "{thread} thread priority"
        );
    }
    assert!(
        level.into_iter().all(|sample| sample > PILOT_LEVEL / 100.0),
        "No stable loopback pilots. Check output mute, volume, and device."
    );
    let directory = tempfile::tempdir()?;
    let path = directory.path().to_owned();
    let (stop, end) = mpsc::channel();
    let progress = Arc::new(Workload::default());
    let workload_progress = progress.clone();
    let background = thread::spawn(move || {
        workload(&path, end, workload_progress).map_err(|error| error.to_string())
    });
    stats
        .threshold
        .store(pack(level.map(|sample| sample * 0.25)), Relaxed);
    output.meters.callback_peak_ns.store(0, Relaxed);
    output.meters.callback_over_budget.store(0, Relaxed);
    render.active.store(true, Relaxed);
    println!(
        "Running {seconds}s on {} at {rate} Hz, observed buffer {} frames, pilots {:?}, CLAP {}, render/capture priorities {}/{}",
        output.device_name,
        output.meters.callback_frames.load(Relaxed),
        level,
        std::env::var_os("LOST_SAMPLES_SEARCH_PYTHON").is_some(),
        render.thread_priority.load(Relaxed),
        stats.thread_priority.load(Relaxed),
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) && !background.is_finished() {
        let remaining = Duration::from_secs(seconds).saturating_sub(start.elapsed());
        thread::sleep(remaining.min(Duration::from_secs(30)));
        received.try_iter().for_each(Event::print);
        println!(
            "Elapsed {:.0}s, windows {}, weak both {}, audition {}, reference {}, capture gaps {}, callback peak {:.3} ms, cycles {}, CLAP passages {}",
            start.elapsed().as_secs_f64(),
            stats.windows.load(Relaxed),
            stats.weak.load(Relaxed),
            stats.weak_audition.load(Relaxed),
            stats.weak_reference.load(Relaxed),
            stats.capture_gaps.load(Relaxed),
            output.meters.callback_peak_ns.load(Relaxed) as f64 / 1e6,
            progress.cycles.load(Relaxed),
            progress.passages.load(Relaxed)
        );
    }
    stats.threshold.store(0, Relaxed);
    render.active.store(false, Relaxed);
    output.send(Command::StopAll);
    let _ = stop.send(());
    background.join().expect("Workload thread panicked")?;
    received.try_iter().for_each(Event::print);
    let over_budget = output.meters.callback_over_budget.load(Relaxed);
    println!(
        "Completed {} cycles with {} CLAP passages in {:.2}s of indexing. {over_budget} over-budget render callbacks, {} render callback gaps, {} weak audition / {} weak reference windows before WASAPI.",
        progress.cycles.load(Relaxed),
        progress.passages.load(Relaxed),
        progress.indexing_ms.load(Relaxed) as f64 / 1e3,
        render.callback_gaps.load(Relaxed),
        render.weak_audition.load(Relaxed),
        render.weak_reference.load(Relaxed)
    );
    assert!(start.elapsed() >= Duration::from_secs(seconds));
    assert!(!output.meters.failed() && !stats.failed.load(Relaxed));
    assert!(
        stats.windows.load(Relaxed) >= seconds * 99,
        "Incomplete loopback capture"
    );
    assert_eq!(
        stats.capture_gaps.load(Relaxed),
        0,
        "Gaps in WASAPI loopback timestamps"
    );
    assert_eq!(
        stats.weak_reference.load(Relaxed),
        0,
        "Missing or attenuated continuous loopback reference"
    );
    assert_eq!(over_budget, 0, "Output callback exceeded its buffer budget");
    Ok(())
}
