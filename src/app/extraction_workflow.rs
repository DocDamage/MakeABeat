use super::{App, Job};
use crate::{
    analysis_jobs::{Priority, Queue},
    audio::{Clip, Command},
    files,
    model::project::Project,
    model::{ExtractionOrigin, Record, Source},
    separation::sam::{self, Part, Span},
};
use std::{path::PathBuf, sync::Arc};

type Outputs = [Arc<Clip>; 3];

struct Request {
    project: Arc<Project>,
    source: usize,
    clip: Arc<Clip>,
    span: Span,
    prompt: String,
}

#[derive(Default)]
pub(crate) struct State {
    pub prompt: String,
    pub status: Option<(String, bool)>,
    pub playing: bool,
    request: Option<Request>,
    outputs: Option<Outputs>,
    jobs: Queue<Outputs>,
}

impl App {
    fn extraction_span(&self) -> files::Result<Span> {
        let clip = self.active_clip().ok_or("Load a sample first.")?;
        Span::new(
            clip,
            self.selection().ok_or("Mark a waveform selection first.")?,
        )
    }

    pub fn can_extract(&self) -> bool {
        !self.recording
            && self.input_recording.is_none()
            && self.extraction_job().is_none()
            && !self.extraction.prompt.trim().is_empty()
            && self.extraction_span().is_ok()
    }

    pub fn extraction_setup(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        let (python, ffmpeg) = sam::local_setup();
        (
            self.library.sam_python.clone().or(python),
            self.library.sam_ffmpeg.clone().or(ffmpeg),
        )
    }

    pub fn choose_extraction_setup(&mut self, ffmpeg: bool) {
        let dialog = rfd::FileDialog::new();
        let selected = if ffmpeg {
            dialog
                .set_title("Choose the shared FFmpeg bin folder")
                .pick_folder()
        } else {
            dialog
                .set_title("Choose the SAM Audio Python executable")
                .pick_file()
        };
        if let Some(path) = selected {
            if ffmpeg {
                self.library.sam_ffmpeg = Some(path);
            } else {
                self.library.sam_python = Some(path);
            }
            self.persist_library();
        }
    }

    pub fn start_extraction(&mut self) {
        if !self.can_extract() {
            return;
        }
        let (python, ffmpeg) = self.extraction_setup();
        let Some(python) = python else {
            self.extraction.status = Some((
                "Choose a prepared SAM Audio Python environment in Setup.".into(),
                true,
            ));
            return;
        };
        self.observe_edits(false);
        let Ok(span) = self.extraction_span() else {
            return;
        };
        let source = self.media.active_source();
        let Some(clip) = self.source_arc(source) else {
            return;
        };
        self.clear_extraction();
        let prompt: String = self
            .extraction
            .prompt
            .trim()
            .chars()
            .take(sam::MAX_PROMPT)
            .collect();
        let work_clip = clip.clone();
        let work_prompt = prompt.clone();
        match self.extraction.jobs.submit(
            &self.ctx,
            "Extracting sound",
            Priority::Interactive,
            move || sam::extract(&python, ffmpeg.as_deref(), &work_clip, span, &work_prompt),
        ) {
            Ok(()) => {
                self.extraction.request = Some(Request {
                    project: self.session.current.clone(),
                    source,
                    clip,
                    span,
                    prompt,
                })
            }
            Err(error) => self.extraction.status = Some((error.to_string(), true)),
        }
    }

    fn extraction_matches(&self) -> bool {
        self.extraction.request.as_ref().is_some_and(|request| {
            Arc::ptr_eq(&request.project, &self.session.current)
                && request.source == self.media.active_source()
                && self
                    .active_clip()
                    .is_some_and(|clip| Arc::ptr_eq(&request.clip.frames, &clip.frames))
                && self
                    .extraction_span()
                    .is_ok_and(|span| span == request.span)
        })
    }

    pub fn extraction_job(&self) -> Option<(&'static str, crate::background::State, usize)> {
        self.extraction.jobs.current()
    }

    pub(super) fn poll_extraction(&mut self) {
        if self.extraction.request.is_some() && !self.extraction_matches() {
            self.clear_extraction();
            self.extraction.status = Some((
                "The project or selection changed. Extract the current selection again.".into(),
                false,
            ));
        }
        if let Some(result) = self.extraction.jobs.poll(&self.ctx)
            && self.extraction.request.is_some()
        {
            match result {
                Ok(outputs) => self.extraction.outputs = Some(outputs),
                Err(error) => {
                    self.extraction.request = None;
                    self.extraction.status = Some((error, true));
                }
            }
        }
    }

    pub fn stop_extraction_preview(&mut self) {
        if self.extraction.playing {
            self.stop_listening();
        }
    }

    pub fn clear_extraction(&mut self) {
        self.stop_extraction_preview();
        self.extraction.jobs.clear();
        self.extraction.request = None;
        self.extraction.outputs = None;
        self.extraction.status = None;
    }

    pub fn extraction_ready(&self) -> bool {
        self.extraction.outputs.is_some() && self.extraction_matches()
    }

    pub fn preview_extraction(&mut self, part: Part) {
        if !self.extraction_ready()
            || self.audio.output().is_none()
            || self.recording
            || self.input_recording.is_some()
        {
            return;
        }
        let clip = self.extraction.outputs.as_ref().unwrap()[part as usize].clone();
        self.stop_all();
        self.send(Command::Listen(
            clip.clone(),
            (0.0, clip.duration()),
            1.0,
            None,
        ));
        self.extraction.playing = true;
    }

    pub fn can_save_extraction(&self, count: usize) -> bool {
        self.extraction_ready()
            && self.can_add_source()
            && !self.recording
            && self.project.studio.sources.len() + 1 + count <= crate::model::MAX_SOURCES
            && self.project_frame_count()
                + self.extraction.outputs.as_ref().unwrap()[1].frames.len() * count
                <= files::MAX_PROJECT_FRAMES
    }

    pub fn save_extraction(&mut self, parts: &[Part]) {
        if parts.is_empty()
            || parts.contains(&Part::Original)
            || !self.can_save_extraction(parts.len())
        {
            return;
        }
        self.sync_source_ids();
        let request = self.extraction.request.take().unwrap();
        let outputs = self.extraction.outputs.take().unwrap();
        let parent_id = self.project.source_ids[request.source].0;
        let parent_title = self
            .source_record(request.source)
            .map_or("Source audio", |record| &record.title)
            .to_owned();
        for &part in parts {
            let residual = part == Part::Residual;
            let title = if residual { "Residual" } else { "Extracted" };
            let record = Record {
                title: format!("{title} · {}", request.prompt),
                artist: "SAM Audio Small".into(),
                genre: String::new(),
                source: Source::Extracted(Box::new(ExtractionOrigin {
                    id: self.random(),
                    parent_id,
                    parent_title: parent_title.clone(),
                    start_frame: request.span.start,
                    end_frame: request.span.end,
                    sample_rate: request.span.rate,
                    prompt: request.prompt.clone(),
                    model: sam::MODEL.into(),
                    revision: sam::REVISION.into(),
                    residual,
                })),
                release: None,
            };
            self.apply_job(Job::SourceAdded(
                record,
                outputs[part as usize].as_ref().clone(),
                None,
            ));
        }
        self.observe_edits(false);
        self.status = Some((
            if parts.len() == 2 {
                "Target and residual added to the project."
            } else {
                "Extraction added to the project."
            }
            .into(),
            false,
        ));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn select(app: &mut App, clip: Clip) {
        app.replace_project(
            Project {
                record: Some(Record {
                    title: "Parent".into(),
                    artist: String::new(),
                    genre: String::new(),
                    source: Source::Generated(42),
                    release: None,
                }),
                ..Default::default()
            },
            Some(clip),
            false,
        );
        app.project.studio.loop_in = Some(0.15);
        app.project.studio.loop_out = Some(1.65);
        app.extraction.prompt = "bass guitar".into();
        app.observe_edits(false);
        app.session.reset(app.project.clone(), None, None);
    }

    #[test]
    fn changed_selection_blocks_preview_and_save_and_drops_completion() {
        let mut app = App::new(eframe::egui::Context::default());
        select(&mut app, Clip::new(8_000, vec![[0.1; 2]; 16_000]));
        let span = app.extraction_span().unwrap();
        let clip = app.source_arc(0).unwrap();
        app.extraction.request = Some(Request {
            project: app.session.current.clone(),
            source: 0,
            clip: clip.clone(),
            span,
            prompt: app.extraction.prompt.clone(),
        });
        app.extraction.outputs = Some([clip.clone(), clip.clone(), clip]);
        assert!(app.extraction_ready());
        app.project.studio.loop_out = Some(1.7);
        assert!(!app.extraction_ready());
        app.save_extraction(&[Part::Target]);
        app.poll_extraction();
        assert!(app.extraction.outputs.is_none() && app.project.studio.sources.is_empty());
    }

    fn wait_for_worker(app: &mut App, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while app.extraction_job().is_some() && Instant::now() < deadline {
            app.poll_extraction();
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            app.extraction_job().is_none(),
            "SAM Audio worker did not stop"
        );
    }

    #[test]
    #[ignore = "requires the cached SAM Audio Small model, CUDA, and a Windows audio device"]
    fn windows_sam_audio_previews_and_saves_selected_passage() -> files::Result<()> {
        let fixture =
            PathBuf::from(std::env::var_os("LOST_SAMPLES_SAM_TEST_WAV").ok_or(
                "Set LOST_SAMPLES_SAM_TEST_WAV to an audio file at least four seconds long.",
            )?);
        let mut app = App::new(eframe::egui::Context::default());
        let clip = files::decode_path(&fixture)?;
        assert!(clip.duration() >= 4.0);
        select(&mut app, clip);
        // At 44.1 kHz this selection exercises TorchAudio's off-by-one resampling case.
        app.project.studio.loop_out = Some(156_736.0 / 44_100.0);
        app.observe_edits(false);
        app.session.reset(app.project.clone(), None, None);
        let span = app.extraction_span()?;
        let before = app.project.clone();
        app.start_extraction();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app
            .extraction_job()
            .is_some_and(|(_, state, _)| state.stage == Some("Loading SAM Audio and extracting"))
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(Instant::now() < deadline, "SAM Audio worker did not start");
        thread::sleep(Duration::from_millis(250));
        app.clear_extraction();
        wait_for_worker(&mut app, Duration::from_secs(5));
        assert!(!app.extraction_ready() && app.project.clone() == before);

        app.start_extraction();
        wait_for_worker(&mut app, Duration::from_secs(125));
        assert!(app.extraction_ready(), "{:?}", app.extraction.status);
        let meters = app
            .audio.output()
            .ok_or("No Windows audio output")?
            .meters
            .clone();
        for part in [Part::Original, Part::Target, Part::Residual] {
            app.preview_extraction(part);
            let deadline = Instant::now() + Duration::from_secs(1);
            while !meters.listening() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(meters.listening());
            app.stop_extraction_preview();
            let deadline = Instant::now() + Duration::from_secs(1);
            while meters.listening() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            assert!(!meters.listening());
        }
        assert!(app.project.clone() == before);
        assert!(!app.can_undo_project(false));
        app.save_extraction(&[Part::Target, Part::Residual]);
        let after = app.project.clone();
        assert_eq!(after.studio.sources.len(), 2);
        for (index, record) in after.studio.sources.iter().enumerate() {
            let Source::Extracted(origin) = &record.source else {
                panic!("Missing provenance");
            };
            assert_eq!(
                (origin.start_frame, origin.end_frame),
                (span.start, span.end)
            );
            assert_eq!(origin.parent_id, after.source_ids[0].0);
            assert_eq!(origin.residual, index == 1);
        }
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("extraction.flip");
        files::save_project(&path, &after, app.media.root_clip())?;
        assert_eq!(&std::fs::read(&path)?[..8], b"LSFLIP09");
        let (reopened, audio) = files::open_project(&path)?;
        assert!(reopened == after);
        assert_eq!(audio.unwrap().sources.len(), 2);
        app.undo_project(false);
        assert!(app.project.clone() == before);
        app.undo_project(true);
        assert!(app.project.clone() == after);
        Ok(())
    }
}
