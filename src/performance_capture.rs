use crate::{
    audio::PadMode,
    model::{ChopInput, PlayedChop, Quantize, TICKS_PER_BAR},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};

const QUEUE_SIZE: usize = 2048;
const MAX_NOTES: usize = 8192;
const BAR: f64 = TICKS_PER_BAR as f64;

#[derive(Clone, Copy)]
pub struct Note {
    pub source: u64,
    pub input: ChopInput,
    pub velocity: u8,
    pub mode: PadMode,
    pub bent: bool,
}

#[derive(Clone, Copy)]
pub enum InputEvent {
    On(Note),
    Off(u64, ChopInput, bool),
    PedalUp(u64),
    SilenceKeys(Option<u64>),
    SilencePads,
    Bend(u64),
    Preview {
        request: u64,
        bars: usize,
        phase: Option<f64>,
    },
}

#[derive(Clone, Copy)]
struct Stamp {
    frame: u64,
    tick: f64,
}

struct Packet {
    epoch: u64,
    at: Stamp,
    event: InputEvent,
}

#[derive(Default)]
struct Status {
    epoch: AtomicU64,
    ticks: AtomicU64,
    overflowed: AtomicU64,
    request: AtomicU64,
}

pub struct Input {
    tx: SyncSender<Packet>,
    status: Arc<Status>,
    at: Stamp,
    epoch: u64,
    pulse_deadline: Option<u64>,
    interrupted: bool,
}

pub struct History {
    rx: Receiver<Packet>,
    status: Arc<Status>,
    epoch: u64,
    notes: VecDeque<TimedNote>,
    preview: Option<Preview>,
    truncated: bool,
    pruned_at: f64,
}

struct TimedNote {
    note: Note,
    start: Stamp,
    end: Option<Stamp>,
    sustained: bool,
}

pub struct Preview {
    request: u64,
    pub bars: usize,
    pub synced: bool,
    pub overflowed: bool,
    pub lost_bend: bool,
    notes: Vec<PlayedChop>,
}

pub fn channel() -> (Input, History) {
    let (tx, rx) = mpsc::sync_channel(QUEUE_SIZE);
    let status = Arc::new(Status::default());
    (
        Input {
            tx,
            status: status.clone(),
            at: Stamp {
                frame: 0,
                tick: 0.0,
            },
            epoch: 0,
            pulse_deadline: None,
            interrupted: false,
        },
        History {
            rx,
            status,
            epoch: 0,
            notes: VecDeque::new(),
            preview: None,
            truncated: false,
            pruned_at: 0.0,
        },
    )
}

impl Input {
    pub fn push(&mut self, event: InputEvent) {
        let packet = Packet {
            epoch: self.epoch,
            at: self.at,
            event,
        };
        if let Err(TrySendError::Full(_)) = self.tx.try_send(packet)
            && self.status.overflowed.load(Ordering::Relaxed) != self.epoch + 1
        {
            self.reset();
            self.status
                .overflowed
                .store(self.epoch + 1, Ordering::Relaxed);
        }
    }

    pub fn reset(&mut self) {
        self.epoch += 1;
        self.at.tick = 0.0;
        self.publish();
        self.status.epoch.store(self.epoch, Ordering::Release);
    }

    pub fn reset_clock(&mut self) {
        self.interrupted = false;
        self.pulse_deadline = None;
        self.reset();
    }

    pub fn pulse(&mut self, seconds: f64, rate: f64, jump: f64) {
        if self.interrupted {
            self.reset_clock();
        } else {
            self.at.tick += jump.max(0.0);
        }
        self.pulse_deadline = Some(self.at.frame + (rate * (seconds * 4.0).max(0.25)) as u64);
    }

    pub fn frame(&mut self, transport_delta: Option<f64>, free_delta: f64) {
        self.at.frame += 1;
        if self
            .pulse_deadline
            .is_some_and(|deadline| self.at.frame >= deadline)
        {
            self.reset_clock();
            self.interrupted = true;
        }
        self.at.tick += if self.interrupted {
            free_delta
        } else {
            transport_delta.unwrap_or(free_delta).max(0.0)
        };
    }

    pub fn external_clock_running(&self) -> bool {
        self.pulse_deadline.is_some()
    }

    pub fn stop_clock(&mut self) {
        if self.pulse_deadline.is_some() || self.interrupted {
            self.reset_clock();
        }
    }

    pub fn publish(&self) {
        self.status
            .ticks
            .store(self.at.tick.to_bits(), Ordering::Relaxed);
    }
}

impl History {
    pub fn poll(&mut self) {
        self.move_to_epoch(self.status.epoch.load(Ordering::Acquire));
        let tick = f64::from_bits(self.status.ticks.load(Ordering::Relaxed));
        if tick - self.pruned_at >= BAR {
            self.notes.retain(|note| note.overlaps(tick - 16.0 * BAR));
            self.pruned_at = tick;
        }
        for _ in 0..QUEUE_SIZE {
            let Ok(packet) = self.rx.try_recv() else {
                break;
            };
            if packet.epoch >= self.epoch {
                self.move_to_epoch(packet.epoch);
                self.apply(packet);
            }
        }
        self.move_to_epoch(self.status.epoch.load(Ordering::Acquire));
    }

    fn move_to_epoch(&mut self, epoch: u64) {
        if epoch > self.epoch {
            self.epoch = epoch;
            self.notes.clear();
            self.preview = None;
            self.truncated = false;
            self.pruned_at = 0.0;
        }
    }

    fn apply(&mut self, packet: Packet) {
        let at = packet.at;
        match packet.event {
            InputEvent::On(note) => {
                self.finish(at, |held| match note.input {
                    ChopInput::Pad(_) => matches!(held.note.input, ChopInput::Pad(_)),
                    ChopInput::Key(_) => {
                        held.note.source == note.source && held.note.input == note.input
                    }
                });
                if self.notes.len() == MAX_NOTES {
                    self.notes.pop_front();
                    self.truncated = true;
                }
                self.notes.push_back(TimedNote {
                    note,
                    start: at,
                    end: None,
                    sustained: false,
                });
            }
            InputEvent::Off(source, input, sustain) => {
                if let Some(held) = self.notes.iter_mut().rev().find(|held| {
                    held.is_held() && held.note.source == source && held.note.input == input
                }) {
                    held.sustained = sustain;
                    if !sustain {
                        held.end = Some(at);
                    }
                }
            }
            InputEvent::PedalUp(source) => {
                self.finish(at, |held| held.note.source == source && held.sustained);
            }
            InputEvent::SilenceKeys(source) => {
                self.finish(at, |held| {
                    matches!(held.note.input, ChopInput::Key(_))
                        && source.is_none_or(|id| id == held.note.source)
                });
            }
            InputEvent::SilencePads => {
                self.finish(at, |held| matches!(held.note.input, ChopInput::Pad(_)));
            }
            InputEvent::Bend(source) => {
                for note in &mut self.notes {
                    if note.is_held()
                        && matches!(note.note.input, ChopInput::Key(_))
                        && note.note.source == source
                    {
                        note.note.bent = true;
                    }
                }
            }
            InputEvent::Preview {
                request,
                bars,
                phase,
            } => {
                if request == self.status.request.load(Ordering::Relaxed) {
                    self.preview = Some(self.build_preview(request, at, bars, phase));
                }
            }
        }
    }

    fn finish(&mut self, at: Stamp, mut matches: impl FnMut(&TimedNote) -> bool) {
        for held in &mut self.notes {
            if held.is_held() && matches(held) {
                held.end = Some(at);
            }
        }
    }

    pub fn preview(&self) -> Option<&Preview> {
        self.preview
            .as_ref()
            .filter(|preview| self.can_accept(preview))
    }

    pub fn invalidate_preview(&self) -> u64 {
        self.status.request.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn can_accept(&self, preview: &Preview) -> bool {
        self.current() && preview.request == self.status.request.load(Ordering::Relaxed)
    }

    pub fn bars(&self) -> f64 {
        if !self.current() || self.notes.is_empty() {
            return 0.0;
        }
        let tick = f64::from_bits(self.status.ticks.load(Ordering::Relaxed));
        let first = self.notes.front().map_or(tick, |note| note.start.tick);
        ((tick - first) / BAR).clamp(0.0, 16.0)
    }

    pub fn has_notes(&self) -> bool {
        let tick = f64::from_bits(self.status.ticks.load(Ordering::Relaxed));
        self.current()
            && self
                .notes
                .iter()
                .any(|note| note.overlaps(tick - 16.0 * BAR))
    }

    fn current(&self) -> bool {
        self.epoch == self.status.epoch.load(Ordering::Acquire)
    }

    pub fn overflowed(&self) -> bool {
        self.truncated || self.status.overflowed.load(Ordering::Relaxed) == self.epoch + 1
    }

    fn build_preview(&self, request: u64, now: Stamp, bars: usize, phase: Option<f64>) -> Preview {
        let length = bars as f64 * BAR;
        // A running capture includes the current partial bar. A stopped capture
        // aligns the first retained hit to beat one if it fits inside the window.
        let start = match phase {
            Some(phase) => now.tick - length + if phase < 1e-6 { 0.0 } else { BAR - phase },
            None => {
                (now.tick - length).max(self.notes.front().map_or(now.tick, |note| note.start.tick))
            }
        };
        let mut lost_bend = false;
        let mut notes = Vec::new();
        for note in &self.notes {
            let gate_end = (note.note.mode == PadMode::Gate).then(|| note.end.unwrap_or(now));
            if note.start.frame > now.frame
                || gate_end.map_or(note.start.tick < start, |end| {
                    end.tick <= start && note.start.tick < start
                })
            {
                continue;
            }
            let tick = (note.start.tick - start).max(0.0);
            if tick >= length {
                continue;
            }
            let tick = (tick.round() as u32).min(length as u32 - 1);
            let length = gate_end.map(|end| {
                ((end.tick.min(now.tick) - start).round() as u32)
                    .saturating_sub(tick)
                    .max(1)
            });
            lost_bend |= note.note.bent;
            notes.push(PlayedChop {
                id: Default::default(),
                tick,
                input: note.note.input,
                velocity: note.note.velocity,
                mode: note.note.mode,
                length,
                sound: Default::default(),
                expression: None,
            });
        }
        Preview {
            request,
            bars,
            synced: phase.is_some(),
            overflowed: self.overflowed(),
            lost_bend,
            notes,
        }
    }
}

impl TimedNote {
    fn is_held(&self) -> bool {
        self.note.mode == PadMode::Gate && self.end.is_none()
    }

    fn overlaps(&self, start: f64) -> bool {
        self.is_held() || self.end.map_or(self.start.tick, |end| end.tick) >= start
    }
}

impl Preview {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn notes(&self, quantize: Quantize, swing: f32) -> Vec<PlayedChop> {
        let end = self.bars as u32 * TICKS_PER_BAR;
        // The piano roll addresses notes by lane and tick. Keep the last trigger
        // at a shared tick and leave a hit off-grid if snapping would collide.
        let original: BTreeMap<_, _> = self
            .notes
            .iter()
            .cloned()
            .map(|note| ((note.tick, note.input), note))
            .collect();
        let mut snapped = BTreeSet::new();
        let mut notes: Vec<_> = original.values().cloned().collect();
        for note in &mut notes {
            let tick = (quantize.snap(f64::from(note.tick), swing) as u32).min(end - 1);
            let slot = (tick, note.input);
            if !original.contains_key(&slot) && snapped.insert(slot) {
                note.tick = tick;
            }
            if let Some(length) = &mut note.length {
                *length = (*length).min(end - note.tick);
            }
        }
        notes.sort_by_key(|note| (note.tick, note.input));
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> InputEvent {
        InputEvent::On(Note {
            source: 1,
            input: ChopInput::Key(60),
            velocity: 89,
            mode: PadMode::Gate,
            bent: false,
        })
    }

    #[test]
    fn a_sustained_note_spanning_the_window_is_trimmed_at_both_edges() {
        let (mut input, mut history) = channel();
        input.push(key());
        input.frame(None, BAR * 3.0);
        input.push(InputEvent::Off(1, ChopInput::Key(60), true));
        input.frame(None, BAR * 4.0 + 250.0);
        input.push(InputEvent::Preview {
            request: 0,
            bars: 4,
            phase: Some(250.0),
        });
        input.push(InputEvent::PedalUp(1));
        history.poll();
        let preview = history.preview().unwrap();
        assert!(preview.synced);
        let notes = preview.notes(Quantize::Off, 0.0);
        assert_eq!(
            notes,
            [PlayedChop {
                id: Default::default(),
                tick: 0,
                input: ChopInput::Key(60),
                velocity: 89,
                mode: PadMode::Gate,
                length: Some(3 * TICKS_PER_BAR + 250),
                sound: Default::default(),
                expression: None,
            }]
        );
    }

    #[test]
    fn overflow_and_clock_timeout_invalidate_previews_and_allow_new_history() {
        let (mut input, mut history) = channel();
        input.push(key());
        input.push(InputEvent::Preview {
            request: 0,
            bars: 4,
            phase: None,
        });
        history.poll();
        assert!(history.preview().is_some());
        for _ in 0..=QUEUE_SIZE {
            input.push(key());
        }
        assert!(history.preview().is_none());
        history.poll();
        assert!(history.overflowed());
        assert!(!history.has_notes());
        input.push(key());
        history.poll();
        assert!(history.has_notes());
        input.pulse(0.025, 100.0, 0.0);
        for _ in 0..25 {
            input.frame(Some(1.0), 1.0);
        }
        history.poll();
        assert!(!history.has_notes());
        assert!(!input.external_clock_running());
        assert!(!history.overflowed());
        input.pulse(0.025, 100.0, 20.0);
        assert!(input.external_clock_running());
    }
}
