use super::App;
use crate::{
    analysis_jobs::{Priority, Queue},
    audio::{Clip, Command},
    audition::{Cache, Prepared},
    model::{Record, Source, beat_map::BeatMap},
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
enum Media {
    Loaded(Arc<Clip>),
    Queue(Record),
}

#[derive(Clone)]
struct Target {
    media: Media,
    source: Option<usize>,
    range: Option<(f64, f64)>,
    preview_point: f64,
    preview_seconds: f64,
}

pub(crate) struct Preview {
    pub(super) queue_origin: Option<Source>,
    pub title: String,
    target: Target,
    pub bars: Option<usize>,
    pub fit: bool,
    pub prepared: Option<Prepared>,
    pub status: Option<String>,
    pub preparing: bool,
    pub external: bool,
    paused: bool,
    bpm: u16,
    map: Option<Arc<BeatMap>>,
}

type Completion = (u64, Result<Prepared, String>);

pub(crate) struct Audition {
    pub preview: Option<Preview>,
    pub gain: f32,
    jobs: Queue<Completion>,
    cache: Arc<Mutex<Cache>>,
    serial: u64,
}

impl Default for Audition {
    fn default() -> Self {
        Self {
            preview: None,
            gain: 0.45,
            jobs: Queue::default(),
            cache: Default::default(),
            serial: 0,
        }
    }
}

impl App {
    pub fn audition_with_beat(
        &mut self,
        title: String,
        clip: Arc<Clip>,
        range: (f64, f64),
        source: Option<usize>,
    ) {
        self.open_beat_audition(
            title,
            Target {
                media: Media::Loaded(clip),
                source,
                range: Some(range),
                preview_point: 0.0,
                preview_seconds: 0.0,
            },
        );
    }

    pub fn audition_saved_cut_with_beat(&mut self, index: usize) {
        let Some(cut) = self.project.samples.get(index) else {
            return;
        };
        if let Some(clip) = self.source_arc(cut.source) {
            self.audition_with_beat(cut.name.clone(), clip, cut.range(), Some(cut.source));
        }
    }

    pub fn audition_queue_with_beat(&mut self, index: usize) {
        let Some(entry) = self.library.queue.get(index).cloned() else {
            return;
        };
        let record = entry.record;
        let queue_origin = record.source.clone();
        let source = (0..=self.project.studio.sources.len()).find(|&i| {
            self.source_record(i)
                .is_some_and(|current| current.source == record.source)
        });
        let loaded = source.and_then(|i| self.source_arc(i)).or_else(|| {
            self.queue_audio
                .as_ref()
                .filter(|audio| audio.source == record.source)
                .map(|audio| audio.clip.clone())
        });
        let range = entry.match_range.or_else(|| {
            loaded.as_ref().and_then(|clip| {
                self.queue_playing
                    .as_ref()
                    .filter(|source| **source == record.source)?;
                self.queue_range.filter(|range| range.1 <= clip.duration())
            })
        });
        self.open_beat_audition(
            record.title.clone(),
            Target {
                media: loaded.map_or_else(|| Media::Queue(record), Media::Loaded),
                source,
                range,
                preview_point: self.queue_preview_point,
                preview_seconds: self.queue_preview_seconds,
            },
        );
        if let Some(preview) = &mut self.audition.preview {
            preview.queue_origin = Some(queue_origin);
        }
    }

    fn open_beat_audition(&mut self, title: String, target: Target) {
        if self.audio.output().is_none() {
            return;
        }
        self.stop_beat_audition();
        self.stop_listening();
        self.audition.preview = Some(Preview {
            queue_origin: None,
            title,
            target,
            bars: None,
            fit: true,
            prepared: None,
            status: None,
            preparing: false,
            bpm: self.project.studio.bpm,
            map: None,
            external: self.midi_clock_connection.is_some(),
            paused: false,
        });
        self.prepare_beat_audition();
    }

    pub fn stop_beat_audition(&mut self) {
        if self.audition.preview.take().is_some() {
            self.audition.jobs.clear();
            self.audition.serial += 1;
            self.send(Command::StopBeatAudition);
        }
    }

    pub fn set_beat_audition_bars(&mut self, bars: Option<usize>) {
        if let Some(preview) = &mut self.audition.preview {
            preview.bars = bars;
            if bars.is_none() && preview.fit {
                self.send(Command::StopBeatAudition);
            }
            self.prepare_beat_audition();
        }
    }

    pub fn set_beat_audition_fit(&mut self, fit: bool) {
        if let Some(preview) = &mut self.audition.preview {
            preview.fit = fit;
            if std::mem::take(&mut preview.paused) {
                preview.status = preview.prepared.as_ref().and_then(|p| p.status.clone());
            }
            self.queue_beat_audition();
        }
    }

    pub fn set_beat_audition_gain(&mut self, gain: f32) {
        self.audition.gain = gain;
        self.send(Command::BeatAuditionGain(gain));
    }

    pub fn start_beat_audition_transport(&mut self) {
        if self.audition.preview.is_some() && self.can_play_section() && !self.playing {
            self.set_playing(true);
        }
    }

    fn prepare_beat_audition(&mut self) {
        let Some(preview) = &mut self.audition.preview else {
            return;
        };
        self.audition.jobs.clear();
        self.audition.serial += 1;
        preview.bpm = self.project.studio.bpm;
        preview.map = preview
            .target
            .source
            .and_then(|index| self.project.studio.beat_maps.get(&index))
            .cloned();
        if !preview.paused {
            preview.status = None;
        }
        preview.prepared = None;
        preview.preparing = false;
        preview.external = self.midi_clock_connection.is_some();
        let serial = self.audition.serial;
        let target = preview.target.clone();
        let bpm = (!preview.external).then_some(preview.bpm);
        let (map, bars) = (preview.map.clone(), preview.bars);
        let cache = self.audition.cache.clone();
        let result = self.audition.jobs.submit(
            &self.ctx,
            "Preparing beat audition",
            Priority::Interactive,
            move || {
                let prepare = || -> crate::files::Result<Prepared> {
                    let clip = match &target.media {
                        Media::Loaded(clip) => clip.clone(),
                        Media::Queue(record) => Arc::new(crate::files::load(record)?.0),
                    };
                    let range = target.range.unwrap_or_else(|| {
                        let duration = clip.duration();
                        let start = (duration * target.preview_point)
                            .min((duration - target.preview_seconds).max(0.0));
                        (start, (start + target.preview_seconds).min(duration))
                    });
                    crate::background::check()?;
                    cache
                        .lock()
                        .map_err(|_| "Audition cache is unavailable.")?
                        .prepare(clip, range, map.clone(), bpm, bars)
                };
                Ok((serial, prepare().map_err(|error| error.to_string())))
            },
        );
        match result {
            Ok(()) => preview.preparing = true,
            Err(error) => preview.status = Some(error.to_string()),
        }
    }

    pub(super) fn queue_beat_audition(&self) {
        let Some(output) = self.audio.output() else { return };
        let Some(preview) = &self.audition.preview else {
            return;
        };
        let Some(prepared) = &preview.prepared else {
            return;
        };
        if preview.paused || (!self.playing && !preview.external) {
            return;
        }
        self.send(Command::BeatAuditionGain(self.audition.gain));
        if let Some(clip) = prepared.fitted.as_ref().filter(|_| preview.fit) {
            output.queue_beat_audition(clip.clone(), (0.0, clip.duration()), prepared.bars);
        } else if !preview.fit || preview.bars.is_some() || prepared.mapped || preview.external {
            output.queue_beat_audition(prepared.clip.clone(), prepared.range, None);
        }
    }

    pub(super) fn poll_beat_audition(&mut self) {
        if let Some(output) = self.audio.output() {
            output.retire_beat_auditions();
        }
        if let Some(preview) = &self.audition.preview {
            let source_changed = preview.target.source.is_some_and(|index| {
                let Media::Loaded(original) = &preview.target.media else {
                    return false;
                };
                self.source_arc(index)
                    .is_none_or(|current| !Arc::ptr_eq(&current.frames, &original.frames))
            });
            if source_changed || self.audio.output().is_none() {
                self.stop_beat_audition();
            } else if (!preview.external && preview.bpm != self.project.studio.bpm)
                || preview
                    .target
                    .source
                    .is_some_and(|index| self.project.studio.beat_maps.get(&index) != preview.map.as_ref())
            {
                self.send(Command::StopBeatAudition);
                self.prepare_beat_audition();
            }
        }
        let Some(result) = self.audition.jobs.poll(&self.ctx) else {
            return;
        };
        let Some(preview) = &mut self.audition.preview else {
            return;
        };
        let result = match result {
            Ok((serial, result)) if serial == self.audition.serial => result,
            Ok(_) => return,
            Err(error) => Err(error),
        };
        preview.preparing = false;
        match result {
            Ok(prepared) => {
                if !preview.paused {
                    preview.status.clone_from(&prepared.status);
                }
                preview.target.media = Media::Loaded(prepared.clip.clone());
                preview.prepared = Some(prepared);
                self.queue_beat_audition();
            }
            Err(error) => preview.status = Some(error),
        }
    }

    pub(super) fn beat_audition_clock_changed(&mut self) {
        if let Some(preview) = &mut self.audition.preview {
            preview.paused = false;
            self.send(Command::StopBeatAudition);
            self.prepare_beat_audition();
        }
    }

    pub(super) fn beat_audition_clock_jumped(&mut self) {
        if let Some(preview) = &mut self.audition.preview {
            preview.paused = true;
            preview.status =
                Some("External clock repositioned. Choose Original to join the next bar.".into());
            self.send(Command::StopBeatAudition);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::model::{NamedSample, beat_map::Beat};
    use std::time::{Duration, Instant};

    fn wait_for(app: &mut App, predicate: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(app) {
            assert!(Instant::now() < deadline, "Audition timed out");
            app.poll_beat_audition();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "requires a working Windows audio output device"]
    fn repeated_saved_cut_audition_preserves_project_and_refits_after_tempo_change() {
        let mut app = App::new(eframe::egui::Context::default());
        assert!(app.audio.output().is_some(), "{:?}", app.audio.error());
        app.media.replace(Some(Arc::new(Clip::new(8_000, vec![[0.12; 2]; 12_000]))));
        app.project.studio.bpm = 240;
        app.project.studio.hits[0][0][0] = 1;
        let mut map = BeatMap::default();
        map.beats
            .extend([0.1, 0.35, 0.6, 0.85, 1.1].map(Beat::manual));
        map.set_downbeat(0);
        app.project.studio.beat_maps.insert(0, Arc::new(map));
        app.project.samples.push(NamedSample {
            name: "Break".into(),
            start: 0.1,
            end: 1.1,
            source: 0,
        });
        app.send(Command::Clip(app.media.root().cloned()));
        app.observe_edits(false);
        app.session.reset(app.project.clone(), None, None);
        let before = app.session.current.clone();
        app.set_playing(true);
        for _ in 0..2 {
            app.audition_saved_cut_with_beat(0);
            wait_for(&mut app, |app| {
                app.audio.output().unwrap().meters.beat_audition() == 2
            });
            assert!(app.audio.output().unwrap().meters.step().is_some());
            app.stop_beat_audition();
            wait_for(&mut app, |app| {
                app.audio.output().unwrap().meters.beat_audition() == 0
            });
            assert!(app.playing);
            assert!(app.project.clone() == *before && !app.can_undo_project(false));
        }
        app.audition_saved_cut_with_beat(0);
        wait_for(&mut app, |app| {
            app.audio.output().unwrap().meters.beat_audition() == 2
        });
        app.project.studio.bpm = 200;
        app.observe_edits(false);
        wait_for(&mut app, |app| {
            app.audition
                .preview
                .as_ref()
                .and_then(|p| p.prepared.as_ref())
                .and_then(|p| p.fitted.as_ref())
                .is_some_and(|clip| clip.duration() == 1.2)
                && app.audio.output().unwrap().meters.beat_audition() == 2
        });
        assert!(app.playing);
        app.stop_beat_audition();
        app.undo_project(false);
        assert!(app.project.clone() == *before && !app.can_undo_project(false));
        app.stop_all();
    }
}
