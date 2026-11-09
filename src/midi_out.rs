use midir::MidiOutput;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Relaxed},
    mpsc::{self, SyncSender},
};

pub const DRUM_NOTES: [u8; crate::model::DRUMS.len()] =
    [36, 38, 45, 47, 50, 37, 39, 42, 46, 49, 51];
pub const CHANNELS: [u8; 3] = [0, 1, 9];
const CLOCK_TICKS: u64 = 20;

pub fn devices() -> Vec<(String, String)> {
    let Ok(output) = MidiOutput::new("Lost Samples Desktop") else {
        return Vec::new();
    };
    output
        .ports()
        .iter()
        .filter_map(|port| Some((port.id(), output.port_name(port).ok()?)))
        .collect()
}

#[derive(Clone, Copy)]
struct Packet([u8; 3]);

impl Packet {
    fn bytes(&self) -> &[u8] {
        &self.0[..if self.0[0] >= 0xf8 { 1 } else { 3 }]
    }
}

pub struct Sink {
    send: SyncSender<Packet>,
    failed: Arc<AtomicBool>,
    overrun: Arc<AtomicBool>,
    clock: bool,
    clock_started: bool,
    next_clock: Option<u64>,
    last_tick: Option<u64>,
    releases: [[Option<u64>; 128]; 3],
}

impl Sink {
    pub fn connect(id: String, clock: bool) -> Result<(Self, Arc<AtomicBool>), String> {
        let (send, receive) = mpsc::sync_channel::<Packet>(512);
        let (connected, ready) = mpsc::sync_channel(1);
        let failed = Arc::new(AtomicBool::new(false));
        let overrun = Arc::new(AtomicBool::new(false));
        let failed_worker = failed.clone();
        let overrun_worker = overrun.clone();
        std::thread::spawn(move || {
            let connection = (|| {
                let output =
                    MidiOutput::new("Lost Samples Desktop").map_err(|error| error.to_string())?;
                let port = output
                    .ports()
                    .into_iter()
                    .find(|port| port.id() == id)
                    .ok_or("MIDI output port is unavailable.")?;
                output
                    .connect(&port, "Lost Samples sequencer")
                    .map_err(|error| error.to_string())
            })();
            let mut connection = match connection {
                Ok(connection) => {
                    if connected.send(Ok(())).is_err() {
                        return;
                    }
                    connection
                }
                Err(error) => {
                    let _ = connected.send(Err(error));
                    return;
                }
            };
            while let Ok(packet) = receive.recv() {
                if overrun_worker.swap(false, Relaxed) {
                    for channel in CHANNELS {
                        let _ = connection.send(&[0xb0 | channel, 123, 0]);
                    }
                }
                if connection.send(packet.bytes()).is_err() {
                    failed_worker.store(true, Relaxed);
                    break;
                }
            }
            if clock {
                let _ = connection.send(&[0xfc]);
            }
            for channel in CHANNELS {
                let _ = connection.send(&[0xb0 | channel, 123, 0]);
            }
        });
        ready
            .recv()
            .map_err(|_| "Could not start MIDI output.".to_owned())??;
        Ok((
            Self {
                send,
                failed: failed.clone(),
                overrun,
                clock,
                clock_started: false,
                next_clock: None,
                last_tick: None,
                releases: [[None; 128]; 3],
            },
            failed,
        ))
    }

    fn emit(&self, bytes: [u8; 3]) {
        if !self.failed.load(Relaxed) && self.send.try_send(Packet(bytes)).is_err() {
            self.overrun.store(true, Relaxed);
        }
    }

    pub fn note(&mut self, part: usize, note: u8, velocity: u8, now: u64, length: u64) {
        let slot = usize::from(note.min(127));
        let channel = CHANNELS[part];
        if self.releases[part][slot].is_some() {
            self.emit([0x80 | channel, slot as u8, 0]);
        }
        self.emit([0x90 | channel, slot as u8, velocity]);
        self.releases[part][slot] = Some(now + length.max(1));
    }

    pub fn advance(&mut self, tick: u64, external_clock: bool) {
        if self.last_tick.replace(tick) == Some(tick) {
            return;
        }
        if self.clock && !external_clock {
            if self.next_clock.is_none() {
                if tick == 0 {
                    self.emit([0xfa, 0, 0]);
                } else {
                    let step = ((tick / 120) & 0x3fff) as u16;
                    self.emit([0xf2, (step & 127) as u8, (step >> 7) as u8]);
                    self.emit([0xfb, 0, 0]);
                }
                self.clock_started = true;
                self.next_clock = Some(tick.div_ceil(CLOCK_TICKS) * CLOCK_TICKS);
            }
            if let Some(next) = self.next_clock
                && tick >= next
            {
                self.emit([0xf8, 0, 0]);
                self.next_clock = Some(next + CLOCK_TICKS);
            }
        }
        for (part, channel) in CHANNELS.into_iter().enumerate() {
            for note in 0..128 {
                if self.releases[part][note].is_some_and(|end| end <= tick) {
                    self.releases[part][note] = None;
                    self.emit([0x80 | channel, note as u8, 0]);
                }
            }
        }
    }

    pub fn stop(&mut self) {
        if self.clock_started {
            self.emit([0xfc, 0, 0]);
        }
        self.clock_started = false;
        self.next_clock = None;
        self.last_tick = None;
        for (part, channel) in CHANNELS.into_iter().enumerate() {
            for note in 0..128 {
                if self.releases[part][note].take().is_some() {
                    self.emit([0x80 | channel, note as u8, 0]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_note_release_follow_sequencer_ticks() {
        let (send, received) = mpsc::sync_channel(32);
        let mut sink = Sink {
            send,
            failed: Arc::new(AtomicBool::new(false)),
            overrun: Arc::new(AtomicBool::new(false)),
            clock: true,
            clock_started: false,
            next_clock: None,
            last_tick: None,
            releases: [[None; 128]; 3],
        };
        sink.advance(0, false);
        sink.note(2, 36, 99, 0, 20);
        sink.advance(19, false);
        sink.advance(20, false);
        sink.advance(20, false);
        sink.stop();
        let messages: Vec<_> = received
            .try_iter()
            .map(|packet| packet.bytes().to_vec())
            .collect();
        assert_eq!(
            messages,
            [
                vec![0xfa],
                vec![0xf8],
                vec![0x99, 36, 99],
                vec![0xf8],
                vec![0x89, 36, 0],
                vec![0xfc],
            ]
        );
        sink.advance(0, true);
        assert!(received.try_recv().is_err());
    }
}
