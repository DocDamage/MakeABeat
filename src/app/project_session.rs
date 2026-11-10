use super::{App, StudioView, recovery_workflow::Recovery};
use crate::{
    audio::{Clip, Command},
    files,
    model::{NamedSample, project::Project, snapshot::SourceId},
    waveform::WaveformView,
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const UNDO_LIMIT: usize = 64;

pub(super) struct Session {
    pub path: Option<PathBuf>,
    pub current: Arc<Project>,
    saved: Arc<Project>,
    restored: Option<PathBuf>,
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    grouped: bool,
    pub samples_in_project: bool,
    next_source_id: u64,
    recovery: Option<Recovery>,
    pub available_recovery: Option<PathBuf>,
    pub show_recovery: bool,
}

impl Session {
    pub fn new(project: Project, nonce: u64, ctx: egui::Context) -> Self {
        let root = files::app_data_dir().map(|path| path.join("recovery"));
        let available_recovery = root.as_deref().and_then(files::recovery::find);
        let current = Arc::new(project);
        Self {
            path: None,
            saved: current.clone(),
            current,
            restored: None,
            undo: Vec::new(),
            redo: Vec::new(),
            grouped: false,
            samples_in_project: false,
            next_source_id: 1,
            recovery: root.map(|path| {
                Recovery::new(
                    path.join(format!("{:x}-{nonce:x}", std::process::id())),
                    ctx,
                )
            }),
            show_recovery: available_recovery.is_some(),
            available_recovery,
        }
    }

    pub fn dirty(&self) -> bool {
        self.restored.is_some()
            || (!Arc::ptr_eq(&self.current, &self.saved) && self.current != self.saved)
    }

    pub fn reset(&mut self, project: Project, path: Option<PathBuf>, restored: Option<PathBuf>) {
        self.clear_recovery();
        self.path = path;
        self.current = Arc::new(project);
        self.saved = self.current.clone();
        self.restored = restored;
        self.undo.clear();
        self.redo.clear();
        self.grouped = false;
    }

    pub fn draft_changed(&self, draft: &Project) -> bool {
        self.current.studio != draft.studio
            || self.current.record != draft.record
            || self.current.samples != draft.samples
            || self.current.source_ids != draft.source_ids
            || !Arc::ptr_eq(&self.current.snapshots, &draft.snapshots)
    }

    pub fn observe(&mut self, draft: &Project, group: bool) {
        if self.current.as_ref() != draft {
            if !self.grouped {
                if self.undo.len() == UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.undo.push(self.current.clone());
            }
            self.redo.clear();
            self.current = Arc::new(draft.clone());
            self.grouped = group;
        } else if !group {
            self.grouped = false;
        }
    }

    pub fn seed_source_ids(&mut self, ids: &[SourceId]) {
        self.next_source_id = ids
            .iter()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
    }

    pub fn sync_source_ids(&mut self, ids: &mut Vec<SourceId>, count: usize) {
        ids.truncate(count);
        while ids.len() < count {
            let id = SourceId(self.next_source_id);
            self.next_source_id = self.next_source_id.wrapping_add(1).max(1);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }

    pub fn can_travel(&self, redo: bool) -> bool {
        if redo {
            !self.redo.is_empty()
        } else {
            !self.undo.is_empty()
        }
    }

    pub fn end_group(&mut self) {
        self.grouped = false;
    }

    pub fn travel(&mut self, redo: bool) -> Option<Arc<Project>> {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let project = from.pop()?;
        to.push(std::mem::replace(&mut self.current, project.clone()));
        self.grouped = false;
        Some(project)
    }

    pub fn mark_saved(&mut self, path: PathBuf, project: Arc<Project>) {
        self.path = Some(path);
        self.saved = project;
        self.remove_restored_recovery();
        if !self.dirty() {
            self.clear_recovery();
        }
    }

    pub fn autosave(&mut self, clip: Option<&Arc<Clip>>, enabled: bool) -> Option<String> {
        let dirty = self.dirty();
        let recovery = self.recovery.as_mut()?;
        let error = recovery.poll();
        if !dirty {
            recovery.clear_if_needed();
        } else if enabled && let Err(message) = recovery.save(&self.current, clip) {
            return Some(message);
        }
        error
    }

    pub fn cleaning_recovery(&self) -> bool {
        self.recovery.as_ref().is_some_and(Recovery::cleaning)
    }

    pub fn clear_recovery(&mut self) {
        if let Some(recovery) = &mut self.recovery {
            recovery.clear();
        }
    }

    pub fn discard(&mut self) {
        self.remove_restored_recovery();
        self.clear_recovery();
    }

    fn remove_restored_recovery(&mut self) {
        if let Some(dir) = self.restored.take().as_deref().and_then(Path::parent)
            && let Some(recovery) = &mut self.recovery
        {
            recovery.remove(dir.to_owned());
        }
    }
}

impl App {
    pub(super) fn replace_project(
        &mut self,
        mut project: Project,
        clip: Option<Clip>,
        samples_in_project: bool,
    ) {
        self.reset_analysis();
        self.clear_transcription();
        self.clear_extraction();
        self.stop_beat_audition();
        self.groove_editor = Default::default();
        self.scene_active = None;
        self.scene_pending = None;
        self.scene_capture = false;
        self.scene_take.clear();
        self.stop_listening();
        self.suggestions.clear();
        self.tempo_estimate = None;
        self.playing = false;
        self.song_playing = None;
        self.view.studio_view = StudioView::Sample;
        self.variations = None;
        self.snapshot_dialog = None;
        self.clear_snapshot_preview();
        self.view.arrangement_timeline = Default::default();
        self.stop_recording();
        self.recording_project += 1;
        self.view.piano_roll = None;
        self.piano_roll_clipboard.clear();
        self.bar_clipboard = None;
        self.recorded_gates.clear();
        self.preview = false;
        self.step = None;
        self.send(Command::Pattern(false));
        self.send(Command::Play(false));
        self.view_count = None;
        self.media.replace(clip.map(Arc::new));
        project.studio.validate_media(self.media.root_clip());
        project
            .studio
            .validate_source_analysis(self.media.root_clip());
        project.studio.validate_song();
        project.studio.identify_saved_events();
        if !samples_in_project {
            project.samples = project
                .record
                .as_ref()
                .and_then(|record| {
                    self.library
                        .sample_banks
                        .iter()
                        .find(|bank| bank.source == record.source)
                })
                .map_or_else(Vec::new, |bank| bank.samples.clone());
            NamedSample::validate_list(&mut project.samples, self.duration());
        }
        self.session.samples_in_project = samples_in_project;
        self.session.seed_source_ids(&project.source_ids);
        self.project = project;
        self.view.waveform = WaveformView::default();
        self.sample_name.clear();
        self.sample_rename = None;
        self.selected_sample = None;
        self.taps.clear();
        self.bar = 0;
        self.selected_pad = 0;
        self.send(Command::Clip(self.media.root().cloned()));
        self.sync();
        let source = self.project.studio.loop_source;
        if source == 0 {
            self.seek(self.project.studio.loop_in.unwrap_or(0.0));
        } else {
            self.select_source(source);
        }
    }

    pub(super) fn sync_source_ids(&mut self) {
        let count = self
            .media
            .root()
            .map_or(0, |_| self.project.studio.sources.len() + 1);
        self.session
            .sync_source_ids(&mut self.project.source_ids, count);
    }

    pub(crate) fn observe_edits(&mut self, group: bool) {
        self.sync();
        if self.session.draft_changed(&self.project) {
            self.clear_snapshot_preview();
            self.analysis_invalidate();
            self.session.observe(&self.project, group);
        } else if !group {
            self.session.end_group();
        }
    }

    pub fn can_undo_project(&self, redo: bool) -> bool {
        !self.recording && self.pending_change.is_none() && self.session.can_travel(redo)
    }

    pub fn undo_project(&mut self, redo: bool) {
        if !self.can_undo_project(redo) {
            return;
        }
        let Some(snapshot) = self.session.travel(redo) else {
            return;
        };
        self.analysis_invalidate();
        self.recording_project += 1;
        self.recorded_gates.clear();
        self.stop_song();
        self.view
            .arrangement_timeline
            .restore_sections(&self.project.studio.sections, &snapshot.studio.sections);
        self.clear_snapshot_preview();
        if self.project.record.is_some() && snapshot.record.is_none() {
            self.stop_all();
            self.media.detach_for_undo();
            self.send(Command::Clip(None));
            self.position = 0.0;
        } else if self.project.record.is_none() && snapshot.record.is_some() {
            self.media.restore_for_redo();
            self.send(Command::Clip(self.media.root().cloned()));
        }
        let samples_changed = self.project.samples != snapshot.samples;
        self.project.clone_from(&snapshot);
        if self.media.active_source() > self.project.studio.sources.len() {
            self.select_source(0);
        }
        if let Some(roll) = self.view.piano_roll.as_mut() {
            roll.reset_selection();
        }
        self.selected_sample = None;
        self.sample_rename = None;
        self.bar = self.bar.min(self.project.studio.bars - 1);
        if samples_changed {
            self.persist_samples();
        }
        self.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{Job, OpenKind},
        model::{SampleBank, Studio},
    };

    #[test]
    fn opened_project_owns_its_cuts_and_keeps_source_ids_across_undo() -> files::Result<()> {
        let mut app = App::new(egui::Context::default());
        app.audio.remove_output();
        let record = files::local_record("fixture.wav".into());
        let cut = |name: &str| NamedSample {
            source: 0,
            name: name.into(),
            start: 0.1,
            end: 0.2,
        };
        app.library.sample_banks = vec![SampleBank {
            source: record.source.clone(),
            samples: vec![cut("Crate cut")],
        }];
        let media = || Clip::new(8_000, vec![[0.25; 2]; 8_000]);
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("fixture.flip");
        files::save_project(
            &path,
            &Project {
                record: Some(record),
                samples: vec![cut("Project cut")],
                source_ids: vec![SourceId(9)],
                ..Default::default()
            },
            Some(&media()),
        )?;
        let (opened, audio) = files::open_project(&path)?;
        app.apply_job(Job::Opened(
            Box::new(opened),
            audio,
            path,
            OpenKind::Template,
        ));
        assert_eq!(app.project.samples[0].name, "Project cut");
        assert_eq!(app.project.source_ids, [SourceId(9)]);
        assert!(app.session.samples_in_project);
        assert!(!app.project_dirty());

        let baseline = app.session.current.clone();
        app.apply_job(Job::SourceAdded(
            files::local_record("first.wav".into()),
            media(),
            None,
        ));
        app.observe_edits(false);
        assert_eq!(app.project.source_ids, [SourceId(9), SourceId(10)]);
        assert_eq!(app.media.active_source(), 1);
        let first_clip = app.source_arc(1).unwrap();
        assert!(baseline.studio.sources.is_empty());
        app.undo_project(false);
        assert!(app.project == *baseline);
        assert_eq!(app.media.active_source(), 0);
        assert!(app.source_clip(1).is_none());
        assert!(app.source_arc(1).is_none());
        assert_eq!(app.library.sample_banks[0].samples[0].name, "Crate cut");
        app.undo_project(true);
        assert_eq!(app.project.source_ids, [SourceId(9), SourceId(10)]);
        assert!(Arc::ptr_eq(&app.source_arc(1).unwrap(), &first_clip));
        app.undo_project(false);
        app.apply_job(Job::SourceAdded(
            files::local_record("second.wav".into()),
            media(),
            None,
        ));
        app.observe_edits(false);
        assert_eq!(app.project.source_ids, [SourceId(9), SourceId(11)]);
        assert!(!Arc::ptr_eq(&app.source_arc(1).unwrap(), &first_clip));
        assert!(!app.can_undo_project(true));
        Ok(())
    }

    #[test]
    fn undo_and_redo_of_the_first_import_detach_and_restore_its_audio() {
        let mut app = App::new(egui::Context::default());
        app.audio.remove_output();
        app.apply_job(Job::SourceAdded(
            files::local_record("first.wav".into()),
            Clip::new(8_000, vec![[0.25; 2]; 8_000]),
            None,
        ));
        app.observe_edits(false);
        let original = app.source_arc(0).unwrap();

        app.undo_project(false);
        assert!(app.project.record.is_none());
        assert!(app.media.root().is_none());
        assert!(app.active_clip().is_none());

        app.undo_project(true);
        assert!(app.project.record.is_some());
        assert!(Arc::ptr_eq(&original, &app.source_arc(0).unwrap()));
        assert_eq!(app.duration(), 1.0);
    }

    fn snapshot(bpm: u16) -> Project {
        Project {
            record: None,
            studio: Studio {
                bpm,
                ..Studio::default()
            },
            samples: Vec::new(),
            ..Project::default()
        }
    }

    #[test]
    fn finishing_an_older_save_preserves_newer_edits_and_undo_reaches_saved_state() {
        let mut session = Session::new(snapshot(90), 1, egui::Context::default());
        session.observe(&snapshot(100), true);
        let saving = session.current.clone();
        session.observe(&snapshot(110), true);
        session.end_group();
        session.mark_saved(PathBuf::from("beat.flip"), saving);
        assert!(session.dirty());
        assert_eq!(session.travel(false).unwrap().studio.bpm, 90);
        assert_eq!(session.travel(true).unwrap().studio.bpm, 110);
        session.observe(&snapshot(100), false);
        assert!(!session.dirty());
    }
}
