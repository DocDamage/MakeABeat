use super::{App, Job};
use crate::{
    analysis_jobs::{Priority, Queue},
    analysis_worker::{self, JobKey, Model, Proposal},
    audio::{Clip, Command, Frame},
    files,
    model::{
        Record, Source,
        beat_map::{BeatMap, WarpPlan, WarpSource},
    },
};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
struct Scope {
    revision: u64,
    source: usize,
    selection: Option<(f64, f64)>,
    frames: Arc<[Frame]>,
}

pub struct PreparedWarp {
    pub plan: WarpPlan,
    pub bpm: u16,
    fingerprint: String,
    clip: Arc<Clip>,
}

enum Completion {
    Beats(Scope, std::result::Result<Proposal, String>),
    Warp(Scope, std::result::Result<PreparedWarp, String>),
    Groove(Scope, std::result::Result<crate::model::groove::Template, String>),
    Weights,
    Environment(PathBuf),
}

#[derive(Default)]
pub(super) struct Analysis {
    jobs: Queue<Completion>,
    revision: u64,
    next_id: u64,
    proposal: Option<(Scope, Proposal)>,
    warp: Option<(Scope, PreparedWarp)>,
    auditioning: bool,
    checked_environment: Option<PathBuf>,
}

impl App {
    fn analysis_scope(&self, clip: &Clip) -> Scope {
        Scope {
            revision: self.analysis.revision,
            source: self.media.active_source(),
            selection: self.selection(),
            frames: clip.frames.clone(),
        }
    }

    fn scope_is_current(&self, scope: &Scope) -> bool {
        self.analysis.revision == scope.revision
            && self.media.active_source() == scope.source
            && self.selection() == scope.selection
            && self
                .active_clip()
                .is_some_and(|clip| Arc::ptr_eq(&scope.frames, &clip.frames))
    }

    pub(super) fn analysis_invalidate(&mut self) {
        self.analysis.revision = self.analysis.revision.wrapping_add(1);
        self.analysis.proposal = None;
        self.discard_warp_preview();
    }

    pub(super) fn reset_analysis(&mut self) {
        self.analysis.jobs.clear();
        self.analysis_invalidate();
    }

    pub fn analysis_job(&self) -> Option<(&'static str, crate::background::State, usize)> {
        self.analysis.jobs.current()
    }

    pub fn cancel_analysis(&mut self) {
        self.analysis.jobs.cancel();
    }

    fn queue_analysis(
        &mut self,
        label: &'static str,
        priority: Priority,
        work: impl Fn() -> files::Result<Completion> + Send + Sync + 'static,
    ) {
        match self.analysis.jobs.submit(&self.ctx, label, priority, work) {
            Ok(()) => self.status = None,
            Err(error) => self.status = Some((error.to_string(), true)),
        }
    }

    pub(super) fn poll_analysis(&mut self) {
        if self
            .analysis
            .warp
            .as_ref()
            .is_some_and(|(scope, _)| !self.scope_is_current(scope))
        {
            self.discard_warp_preview();
        }
        if self
            .analysis
            .proposal
            .as_ref()
            .is_some_and(|(scope, _)| !self.scope_is_current(scope))
        {
            self.analysis.proposal = None;
        }
        let Some(result) = self.analysis.jobs.poll(&self.ctx) else {
            return;
        };
        match result {
            Ok(Completion::Beats(scope, result)) if self.scope_is_current(&scope) => match result {
                Ok(proposal) => {
                    self.status = Some((
                        format!("Found {} beat suggestions.", proposal.beats.len()),
                        false,
                    ));
                    self.analysis.proposal = Some((scope, proposal));
                }
                Err(error) => self.status = Some((error, true)),
            },
            Ok(Completion::Warp(scope, result)) if self.scope_is_current(&scope) => match result {
                Ok(preview) => {
                    self.analysis.warp = Some((scope, preview));
                    self.status = Some(("Warp preview ready.".into(), false));
                }
                Err(error) => self.status = Some((error, true)),
            },
            Ok(Completion::Groove(scope, result)) if self.scope_is_current(&scope) => match result {
                Ok(template) => self.accept_groove(template),
                Err(error) => self.status = Some((error, true)),
            },
            Ok(Completion::Weights) => {
                self.status = Some(("Beat This! weights verified.".into(), false))
            }
            Ok(Completion::Environment(python)) => {
                self.analysis.checked_environment = Some(python);
                self.status = Some(("Beat This! CPU environment is available.".into(), false));
            }
            Ok(_) => {
                self.status = Some((
                    "Source or project changed while analysis was running.".into(),
                    false,
                ))
            }
            Err(error) => self.status = Some((error, true)),
        }
    }

    pub fn set_beat_python(&mut self, path: PathBuf) {
        self.library.beat_python = Some(path);
        self.analysis.checked_environment = None;
        self.persist_library();
    }

    pub fn beat_environment_checked(&self) -> bool {
        self.analysis.checked_environment.is_some()
            && self.analysis.checked_environment == self.library.beat_python
    }

    pub fn check_beat_environment(&mut self) {
        let Some(path) = self.library.beat_python.clone() else {
            return;
        };
        self.queue_analysis(
            "Checking Beat This! setup…",
            Priority::Foreground,
            move || {
                analysis_worker::probe(&path)?;
                Ok(Completion::Environment(path.clone()))
            },
        );
    }

    pub fn download_beat_weights(&mut self) {
        self.queue_analysis(
            "Preparing Beat This! weights…",
            Priority::Foreground,
            || {
                analysis_worker::download_checkpoint()?;
                Ok(Completion::Weights)
            },
        );
    }

    pub fn analyze_beat_map(&mut self, neural: bool) {
        let model = if neural {
            let Some(path) = self.library.beat_python.clone() else {
                return;
            };
            Model::BeatThis(path)
        } else {
            Model::Pulse
        };
        self.observe_edits(false);
        let Some(clip) = self.source_arc(self.media.active_source()) else {
            return;
        };
        let scope = self.analysis_scope(&clip);
        let range = scope.selection.unwrap_or((0.0, clip.duration()));
        self.analysis.next_id = self.analysis.next_id.wrapping_add(1);
        let key = JobKey {
            job_id: self.analysis.next_id,
            project_revision: scope.revision,
            source_id: self.project.source_ids.get(scope.source).copied(),
        };
        let bpm = self.project.studio.bpm;
        self.analysis.proposal = None;
        self.queue_analysis(
            "Analyzing beats locally…",
            Priority::Foreground,
            move || {
                let result = analysis_worker::analyze(&model, key, &clip, range, bpm)
                    .map_err(|error| error.to_string());
                Ok(Completion::Beats(scope.clone(), result))
            },
        );
    }

    pub fn beat_proposal(&self) -> Option<&Proposal> {
        let (scope, proposal) = self.analysis.proposal.as_ref()?;
        self.scope_is_current(scope).then_some(proposal)
    }

    pub fn discard_beat_proposal(&mut self) {
        self.analysis.proposal = None;
    }

    pub fn adopt_beat_proposal(&mut self) {
        self.observe_edits(false);
        let Some((scope, proposal)) = self.analysis.proposal.take() else {
            return;
        };
        if !self.scope_is_current(&scope) {
            return;
        }
        let mut map = self
            .project.studio
            .beat_maps
            .get(&scope.source)
            .map_or_else(BeatMap::default, |map| map.as_ref().clone());
        map.adopt(&proposal.beats, proposal.provenance, self.duration());
        self.project.studio.beat_maps.insert(scope.source, Arc::new(map));
        self.observe_edits(false);
    }

    pub fn extract_groove(&mut self) {
        let Some(clip) = self.source_arc(self.media.active_source()) else { return };
        let Some(range) = self.selection() else { return };
        let Some(beats) = self.project.studio.beat_maps.get(&self.media.active_source()).and_then(|map| {
            crate::groove::loop_beats(map, range, crate::model::groove::MAX_BARS)
        }) else {
            self.status = Some(("Select a complete bar from its downbeat through the next downbeat in the beat map.".into(), true));
            return;
        };
        let scope = self.analysis_scope(&clip);
        let beats = beats.to_vec();
        let title = self.source_record(self.media.active_source()).map_or_else(|| "Audio selection".into(), |r| r.title.clone());
        self.queue_analysis("Extracting groove", Priority::Interactive, move || {
            Ok(Completion::Groove(scope.clone(), crate::groove::extract(&clip, &beats, title.clone())
                .map_err(|error| error.to_string())))
        });
    }

    pub fn prepare_mapped_warp(&mut self) {
        let range = self.selection().unwrap_or((0.0, self.duration()));
        let Some(map) = self.project.studio.beat_maps.get(&self.media.active_source()) else {
            return;
        };
        match map.warp_plan(range, self.project.studio.bpm) {
            Ok(plan) => self.prepare_warp(plan),
            Err(error) => self.status = Some((error.to_string(), true)),
        }
    }

    pub fn stretch_selection(&mut self, bars: usize) {
        self.prepare_manual_warp(bars as f64 * 4.0, false);
    }

    pub fn warp_selection(&mut self, bars: usize) {
        self.warp_selection_beats(bars * 4);
    }

    pub fn warp_selection_beats(&mut self, beats: usize) {
        self.prepare_manual_warp(beats as f64, true);
    }

    fn prepare_manual_warp(&mut self, beats: f64, use_markers: bool) {
        let Some(range) = self.selection() else {
            return;
        };
        let markers = if use_markers {
            self.project.studio
                .warp_markers
                .get(&self.media.active_source())
                .into_iter()
                .flatten()
                .copied()
                .filter(|marker| range.0 < marker.source_seconds && marker.source_seconds < range.1)
                .collect()
        } else {
            Vec::new()
        };
        self.prepare_warp(WarpPlan {
            range,
            beats,
            markers,
        });
    }

    fn prepare_warp(&mut self, plan: WarpPlan) {
        if self.recording || self.input_recording.is_some() {
            return;
        }
        self.observe_edits(false);
        let Some(clip) = self.source_arc(self.media.active_source()) else {
            return;
        };
        let scope = self.analysis_scope(&clip);
        let bpm = self.project.studio.bpm;
        let remaining = files::MAX_PROJECT_FRAMES
            .saturating_sub(self.project_frame_count())
            .min(files::MAX_PROJECT_FRAMES / 2);
        self.discard_warp_preview();
        self.queue_analysis(
            "Preparing pitch-preserved warp…",
            Priority::Interactive,
            move || {
                let render = || -> files::Result<PreparedWarp> {
                    crate::background::stage("Rendering warp preview")?;
                    let seconds = plan.beats * 60.0 / f64::from(bpm);
                    let rendered = if plan.markers.is_empty() {
                        crate::stretch::fit(&clip, plan.range, seconds, remaining)
                    } else {
                        crate::stretch::fit_warp(
                            &clip,
                            plan.range,
                            seconds,
                            f64::from(bpm),
                            &plan.markers,
                            remaining,
                        )
                    }?;
                    let fingerprint = analysis_worker::fingerprint(&clip)?;
                    Ok(PreparedWarp {
                        plan: plan.clone(),
                        bpm,
                        fingerprint,
                        clip: Arc::new(rendered),
                    })
                };
                Ok(Completion::Warp(
                    scope.clone(),
                    render().map_err(|error| error.to_string()),
                ))
            },
        );
    }

    pub fn warp_preview(&self) -> Option<&PreparedWarp> {
        let (scope, preview) = self.analysis.warp.as_ref()?;
        self.scope_is_current(scope).then_some(preview)
    }

    pub fn audition_warp(&mut self, processed: bool) {
        if self.audio.output().is_none() || self.recording {
            return;
        }
        let Some(preview) = self.warp_preview() else {
            return;
        };
        let audio = if processed {
            Some((preview.clip.clone(), (0.0, preview.clip.duration())))
        } else {
            self.source_arc(self.media.active_source())
                .map(|clip| (clip, preview.plan.range))
        };
        if let Some((clip, range)) = audio {
            self.stop_all();
            self.send(Command::Listen(clip, range, 1.0, None));
            self.analysis.auditioning = true;
        }
    }

    pub fn discard_warp_preview(&mut self) {
        self.analysis.warp = None;
        if std::mem::take(&mut self.analysis.auditioning) {
            self.stop_listening();
        }
    }

    pub fn edit_warp_proposal(&mut self) {
        let Some(preview) = self.warp_preview() else {
            return;
        };
        let plan = preview.plan.clone();
        self.project.studio
            .warp_markers
            .insert(self.media.active_source(), plan.markers);
        self.set_loop_range(plan.range);
        self.observe_edits(false);
    }

    pub fn keep_warp_preview(&mut self) {
        if !self.can_add_source() || self.recording {
            return;
        }
        self.observe_edits(false);
        let Some((scope, preview)) = self.analysis.warp.take() else {
            return;
        };
        if !self.scope_is_current(&scope) {
            return;
        }
        let size = self
            .project_frame_count()
            .saturating_add(preview.clip.frames.len());
        if size > files::MAX_PROJECT_FRAMES {
            self.status = Some((
                "The rendered warp would exceed the project audio limit.".into(),
                true,
            ));
            return;
        }
        let title = self.source_record(scope.source).map_or_else(
            || "Warped source".to_owned(),
            |record| format!("{} · Warp", record.title),
        );
        let record = Record {
            title,
            artist: "Project audio".into(),
            genre: String::new(),
            source: Source::Generated(self.random()),
            release: None,
        };
        self.discard_warp_preview();
        let target = self.project.studio.sources.len() + 1;
        self.apply_job(Job::SourceAdded(
            record,
            preview.clip.as_ref().clone(),
            None,
        ));
        self.project.studio.warp_sources.insert(
            target,
            WarpSource {
                parent: scope.source,
                algorithm: "wsola-v1".into(),
                fingerprint: preview.fingerprint,
                range: preview.plan.range,
                target_bpm: preview.bpm,
                target_beats: preview.plan.beats,
                markers: preview.plan.markers,
            },
        );
        self.observe_edits(false);
        self.status = Some(("Saved the preview as a new source.".into(), false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use std::time::{Duration, Instant};

    fn finish(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.analysis_job().is_some() && Instant::now() < deadline {
            app.poll_analysis();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.analysis_job().is_none());
    }

    #[test]
    fn a_cancelled_or_stale_warp_attaches_nothing_and_keep_uses_the_preview_buffer()
    -> files::Result<()> {
        let mut app = App::new(egui::Context::default());
        app.audio.remove_output();
        let rate = 8_000;
        app.media.replace(Some(Arc::new(Clip::new(
            rate,
            (0..3 * rate)
                .map(|frame| {
                    let t = f64::from(frame) / f64::from(rate);
                    let sample = (t * std::f64::consts::TAU * 120.0).sin() as f32 * 0.4;
                    [sample; 2]
                })
                .collect(),
        ))));
        app.project.studio.bpm = 120;
        let mut map = BeatMap::default();
        for at in [0.1, 0.59, 1.1, 1.64, 2.22] {
            map.add(at, 3.0).unwrap();
        }
        map.set_downbeat(0);
        app.project.studio.beat_maps.insert(0, Arc::new(map));
        app.set_loop_range((0.0, 2.5));
        app.observe_edits(false);
        let original = app.project.clone();
        app.prepare_mapped_warp();
        app.cancel_analysis();
        finish(&mut app);
        assert!(app.warp_preview().is_none());
        assert!(app.project.clone() == original);

        app.prepare_mapped_warp();
        app.set_loop_range((0.05, 2.3));
        app.observe_edits(false);
        finish(&mut app);
        assert!(app.warp_preview().is_none());
        assert!(app.project.studio.sources.is_empty());

        let before_keep = app.project.clone();
        app.prepare_mapped_warp();
        finish(&mut app);
        let preview = app.warp_preview().expect("a prepared preview");
        let frames = preview.clip.frames.clone();
        assert_eq!(frames.len(), 16_000);
        assert!(app.project.clone() == before_keep);
        app.keep_warp_preview();
        let committed = app.source_arc(1).expect("new project source");
        assert!(Arc::ptr_eq(&committed.frames, &frames));
        assert_eq!(app.project.studio.warp_sources[&1].parent, 0);

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("warped.flip");
        let project = app.project.clone();
        files::save_project(&path, &project, app.media.root_clip())?;
        assert_eq!(&std::fs::read(&path)?[..8], b"LSFLIP06");
        let (reopened, clip) = files::open_project(&path)?;
        assert!(reopened == project);
        assert_eq!(clip.unwrap().sources[0].frames, frames);
        app.undo_project(false);
        assert!(app.project.clone() == before_keep);
        Ok(())
    }
}
