use crate::audio::{Clip, Frame};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed},
    mpsc::{Receiver, SyncSender, sync_channel},
};
use std::thread::{self, JoinHandle};

const BLOCK_FRAMES: usize = 256;
const CAPTURE_QUEUE_BLOCKS: usize = 256;

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
    count: AtomicUsize,
    peak: AtomicU32,
    failed: AtomicBool,
    overflowed: AtomicBool,
    monitor: AtomicBool,
}

struct Block {
    frames: [Frame; BLOCK_FRAMES],
    len: usize,
}

pub struct Recording {
    stream: Stream,
    writer: JoinHandle<Vec<Frame>>,
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
        let (capture, recorded) = sync_channel(CAPTURE_QUEUE_BLOCKS);
        let input = Capture {
            buffer: buffer.clone(),
            limit,
            capture,
            monitor: send,
        };
        let stream = match format.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, input),
            SampleFormat::F64 => build::<f64>(&device, &config, input),
            SampleFormat::I16 => build::<i16>(&device, &config, input),
            SampleFormat::I32 => build::<i32>(&device, &config, input),
            SampleFormat::U16 => build::<u16>(&device, &config, input),
            _ => return Err("This input device uses an unsupported sample format.".into()),
        }
        .map_err(|error| error.to_string())?;
        let writer = thread::Builder::new()
            .name("recording-writer".into())
            .spawn(move || collect_recording(recorded))
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
                writer,
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
        self.buffer.stop_reason(self.limit)
    }

    pub fn finish(self) -> (Option<Clip>, Option<&'static str>) {
        let Self {
            stream,
            writer,
            buffer,
            rate,
            limit,
            ..
        } = self;
        drop(stream);
        // Closing the producer lets the writer drain every accepted block before joining.
        match writer.join() {
            Ok(frames) => (
                (!frames.is_empty()).then(|| Clip::new(rate, frames)),
                buffer.stop_reason(limit),
            ),
            Err(_) => (None, Some("The recording worker stopped unexpectedly.")),
        }
    }
}

impl Buffer {
    fn stop_reason(&self, limit: usize) -> Option<&'static str> {
        if self.overflowed.load(Relaxed) {
            Some("The recording buffer filled. Audio captured before the overflow was kept.")
        } else if self.failed.load(Relaxed) {
            Some("Input disconnected. The available recording was kept.")
        } else if self.count.load(Relaxed) >= limit {
            Some("Recording reached the project or five-minute limit.")
        } else {
            None
        }
    }
}

fn collect_recording(recorded: Receiver<Block>) -> Vec<Frame> {
    let mut frames = Vec::new();
    while let Ok(block) = recorded.recv() {
        frames.extend_from_slice(&block.frames[..block.len]);
    }
    frames
}

struct Capture {
    buffer: Arc<Buffer>,
    limit: usize,
    capture: SyncSender<Block>,
    monitor: SyncSender<Frame>,
}

impl Capture {
    fn push<T: SizedSample>(&self, input: &[T], channels: usize)
    where
        f32: FromSample<T>,
    {
        if self.buffer.overflowed.load(Relaxed) {
            return;
        }
        let mut count = self.buffer.count.load(Relaxed);
        let available = (input.len() / channels).min(self.limit - count);
        let listening = self.buffer.monitor.load(Relaxed);
        let mut peak = 0.0_f32;
        for samples in input[..available * channels].chunks(BLOCK_FRAMES * channels) {
            let mut block = Block {
                frames: [[0.0; 2]; BLOCK_FRAMES],
                len: samples.len() / channels,
            };
            for (frame, samples) in block.frames.iter_mut().zip(samples.chunks_exact(channels)) {
                let sample = |index| {
                    let value = f32::from_sample_(samples[index]);
                    if value.is_finite() {
                        value.clamp(-1.0, 1.0)
                    } else {
                        0.0
                    }
                };
                *frame = [sample(0), sample(usize::from(channels > 1))];
                peak = peak.max(frame[0].abs()).max(frame[1].abs());
                if listening {
                    let _ = self.monitor.try_send(*frame);
                }
            }
            let len = block.len;
            if self.capture.try_send(block).is_err() {
                self.buffer.overflowed.store(true, Relaxed);
                break;
            }
            count += len;
        }
        self.buffer.count.store(count, Relaxed);
        self.buffer.peak.store(peak.to_bits(), Relaxed);
    }
}

fn build<T: SizedSample>(
    device: &cpal::Device,
    config: &StreamConfig,
    capture: Capture,
) -> Result<Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels).max(1);
    let error_buffer = capture.buffer.clone();
    #[cfg(windows)]
    let mut promote = true;
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            #[cfg(windows)]
            if std::mem::take(&mut promote) {
                crate::platform::promote_audio_thread();
            }
            capture.push(input, channels);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(limit: usize, queue: usize) -> (Capture, Receiver<Block>) {
        let (capture, recorded) = sync_channel(queue);
        let (monitor, _) = sync_channel(1);
        let buffer = Arc::new(Buffer::default());
        (
            Capture {
                buffer,
                limit,
                capture,
                monitor,
            },
            recorded,
        )
    }

    #[test]
    fn recording_keeps_the_final_partial_block_at_the_frame_limit() {
        let (capture, recorded) = capture(BLOCK_FRAMES + 1, 2);
        let mut samples = vec![0.25_f32; BLOCK_FRAMES + 2];
        samples[BLOCK_FRAMES] = f32::NAN;
        capture.push(&samples, 1);
        assert_eq!(capture.buffer.count.load(Relaxed), BLOCK_FRAMES + 1);
        assert!(!capture.buffer.overflowed.load(Relaxed));
        drop(capture);

        let frames = collect_recording(recorded);
        assert_eq!(frames.len(), BLOCK_FRAMES + 1);
        assert!(
            frames[..BLOCK_FRAMES]
                .iter()
                .all(|&frame| frame == [0.25; 2])
        );
        assert_eq!(frames[BLOCK_FRAMES], [0.0; 2]);
    }

    #[test]
    fn capture_overflow_keeps_a_contiguous_prefix_and_stops_accepting_frames() {
        let (capture, recorded) = capture(100, 1);
        capture.push(&[0.5_f32, -0.5], 2);
        capture.push(&[0.25_f32, -0.25], 2);
        assert!(capture.buffer.overflowed.load(Relaxed));
        assert_eq!(capture.buffer.count.load(Relaxed), 1);
        assert_eq!(recorded.try_recv().unwrap().frames[0], [0.5, -0.5]);

        capture.push(&[1.0_f32, -1.0], 2);
        assert!(recorded.try_recv().is_err());
        assert_eq!(capture.buffer.count.load(Relaxed), 1);
    }
}
