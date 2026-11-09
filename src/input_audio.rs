use crate::audio::{Clip, Frame};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed},
    mpsc::{Receiver, SyncSender, sync_channel},
};

pub fn devices() -> Vec<String> {
    let mut names = cpal::default_host()
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|device| device.name().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

#[derive(Default)]
struct Buffer {
    frames: Mutex<Vec<Frame>>,
    count: AtomicUsize,
    peak: AtomicU32,
    failed: AtomicBool,
    monitor: AtomicBool,
}

pub struct Recording {
    stream: Stream,
    buffer: Arc<Buffer>,
    pub name: String,
    rate: u32,
    limit: usize,
}

impl Recording {
    pub fn start(name: Option<&str>, available: usize) -> Result<(Self, Monitor), String> {
        let host = cpal::default_host();
        let device = match name {
            Some(name) => host
                .input_devices()
                .map_err(|error| error.to_string())?
                .find(|device| device.name().ok().as_deref() == Some(name)),
            None => host.default_input_device(),
        }
        .ok_or("The selected input device is unavailable.")?;
        let name = device.name().unwrap_or_else(|_| "Audio input".into());
        let format = device
            .default_input_config()
            .map_err(|error| error.to_string())?;
        let config = format.config();
        let rate = config.sample_rate.0;
        let limit = available.min(rate as usize * 300);
        if limit == 0 {
            return Err("There is no audio space left in this project.".into());
        }
        let buffer = Arc::new(Buffer::default());
        let (send, receive) = sync_channel(2048);
        let stream = match format.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, &buffer, limit, send),
            SampleFormat::F64 => build::<f64>(&device, &config, &buffer, limit, send),
            SampleFormat::I16 => build::<i16>(&device, &config, &buffer, limit, send),
            SampleFormat::I32 => build::<i32>(&device, &config, &buffer, limit, send),
            SampleFormat::U16 => build::<u16>(&device, &config, &buffer, limit, send),
            _ => return Err("This input device uses an unsupported sample format.".into()),
        }
        .map_err(|error| error.to_string())?;
        stream.play().map_err(|error| error.to_string())?;
        let monitor = Monitor {
            receive,
            rate: f64::from(rate),
            phase: 1.0,
            frame: [0.0; 2],
        };
        Ok((
            Self {
                stream,
                buffer,
                name,
                rate,
                limit,
            },
            monitor,
        ))
    }

    pub fn monitor(&self, enabled: bool) {
        self.buffer.monitor.store(enabled, Relaxed);
    }

    pub fn seconds(&self) -> f64 {
        self.buffer.count.load(Relaxed) as f64 / f64::from(self.rate)
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.buffer.peak.load(Relaxed))
    }

    pub fn stop_reason(&self) -> Option<&'static str> {
        if self.buffer.failed.load(Relaxed) {
            Some("Input disconnected. The available recording was kept.")
        } else if self.buffer.count.load(Relaxed) >= self.limit {
            Some("Recording reached the project or five-minute limit.")
        } else {
            None
        }
    }

    pub fn finish(self) -> Option<Clip> {
        let Self {
            stream,
            buffer,
            rate,
            ..
        } = self;
        drop(stream);
        let frames = std::mem::take(&mut *buffer.frames.lock().ok()?);
        (!frames.is_empty()).then(|| Clip::new(rate, frames))
    }
}

fn build<T: SizedSample>(
    device: &cpal::Device,
    config: &StreamConfig,
    buffer: &Arc<Buffer>,
    limit: usize,
    monitor: SyncSender<Frame>,
) -> Result<Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels).max(1);
    let error_buffer = buffer.clone();
    let buffer = buffer.clone();
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            let Ok(mut frames) = buffer.frames.lock() else {
                return;
            };
            let remaining = limit - frames.len();
            let listening = buffer.monitor.load(Relaxed);
            let mut peak = 0.0_f32;
            for samples in input.chunks_exact(channels).take(remaining) {
                let sample = |index| {
                    let value = f32::from_sample_(samples[index]);
                    if value.is_finite() {
                        value.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                };
                let frame = [sample(0), sample(usize::from(channels > 1))];
                peak = peak.max(frame[0].abs()).max(frame[1].abs());
                frames.push(frame);
                if listening {
                    let _ = monitor.try_send(frame);
                }
            }
            buffer.count.store(frames.len(), Relaxed);
            buffer.peak.store(peak.to_bits(), Relaxed);
        },
        move |_| error_buffer.failed.store(true, Relaxed),
        None,
    )
}

pub struct Monitor {
    receive: Receiver<Frame>,
    rate: f64,
    phase: f64,
    frame: Frame,
}

impl Monitor {
    pub fn next(&mut self, output_rate: f64) -> Frame {
        while self.phase >= 1.0 {
            self.frame = self.receive.try_recv().unwrap_or([0.0; 2]);
            self.phase -= 1.0;
        }
        self.phase += self.rate / output_rate;
        self.frame.map(|sample| sample * 0.5)
    }
}
