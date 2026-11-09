use super::{App, StudioView};
use crate::{
    analysis_jobs::{Priority, Queue},
    audio::{Clip, Command},
    files,
    model::{
        MAX_SAMPLE_NAME, MAX_SECTIONS, MAX_SONG_BARS, PlayedChop, Section, SectionKind,
        beat_map::BeatMap, melody::Instrument,
    },
    transcription::{self, Alignment, Candidate, Detection},
};
use std::{collections::HashSet, path::PathBuf, sync::Arc};

type Completion = (u64, Result<Vec<Candidate>, String>);

#[derive(Default)]
pub(crate) struct State {
    pub editor: Option<Editor>,
    jobs: Queue<Completion>,
    revision: u64,
}

pub(crate) struct Editor {
    source: usize,
    clip: Arc<Clip>,
    pub title: String,
    pub range: (f64, f64),
    map: Option<Arc<BeatMap>>,
    pub bpm: u16,
    pub alignment: Alignment,
    pub detection: Detection,
    raw: Option<Vec<Candidate>>,
    pub notes: Vec<PlayedChop>,
    pub unsupported: Vec<u8>,
    pub sound: Instrument,
    pub name: String,
    pub selected: Option<usize>,
    pub bar: u32,
    pub status: Option<String>,
    playing: bool,
}

impl Editor {
    pub fn mapped(&self) -> bool {
        self.map.as_ref().is_some_and(|map| map.beats.len() >= 2)
    }

    pub fn has_downbeat(&self) -> bool {
        self.mapped()
            && self
                .map
                .as_ref()
                .is_some_and(|map| map.beats.iter().any(|beat| beat.downbeat))
    }

    pub fn has_proposal(&self) -> bool {
        self.raw.is_some()
    }

    pub fn align(&mut self) {
        let Some(raw) = &self.raw else { return };
        match transcription::convert(
            raw,
            self.range,
            self.map.as_deref(),
            self.bpm,
            self.alignment,
            self.detection,
        ) {
            Ok(result) => {
                self.notes = result.notes;
                self.unsupported = result.unsupported;
                self.selected = None;
                self.bar = 0;
                self.status = self
                    .notes
                    .is_empty()
                    .then(|| "No notes match these settings.".into());
            }
            Err(error) => self.status = Some(error.to_string()),
        }
    }
}

impl App {
    pub fn open_transcription(&mut self) {
        let source = self.active_source;
        let Some(clip) = self.source_arc(source) else {
            return;
        };
        let range = self.selection().unwrap_or((0.0, clip.duration()));
        if range.1 - range.0 > transcription::MAX_SECONDS {
            self.status = Some((
                "Select at most three minutes of the source or stem.".into(),
                true,
            ));
            return;
        }
        let title = self
            .source_record(source)
            .map_or("Source audio", |record| record.title.as_str())
            .to_owned();
        let map = self.studio.beat_maps.get(&source).cloned();
        let sound = self
            .transcription_sounds()
            .into_iter()
            .nth(1)
            .filter(|(name, _)| name == "Studio chromatic sample")
            .map_or(Instrument::Sine, |(_, sound)| sound);
        self.transcription.editor = Some(Editor {
            source,
            clip,
            title,
            range,
            map,
            bpm: self.studio.bpm,
            alignment: Alignment::Selection,
            detection: Detection::default(),
            raw: None,
            notes: Vec::new(),
            unsupported: Vec::new(),
            sound,
            name: (1..)
                .map(|i| format!("Transcription {i}"))
                .find(|name| {
                    !self
                        .studio
                        .patterns
                        .iter()
                        .any(|pattern| pattern.name.eq_ignore_ascii_case(name))
                })
                .unwrap(),
            selected: None,
            bar: 0,
            status: None,
            playing: false,
        });
    }

    pub fn pitch_python(&self) -> PathBuf {
        self.library
            .pitch_python
            .clone()
            .unwrap_or_else(|| PathBuf::from("python"))
    }

    pub fn choose_pitch_python(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Choose the Basic Pitch Python executable")
            .pick_file()
        {
            self.library.pitch_python = Some(path);
            self.persist_library();
        }
    }

    pub fn detect_transcription(&mut self, editor: &mut Editor) {
        if editor.detection.low > editor.detection.high {
            editor.status = Some("The low note must be at or below the high note.".into());
            return;
        }
        self.stop_transcription_preview(editor);
        self.transcription.jobs.clear();
        self.transcription.revision += 1;
        let revision = self.transcription.revision;
        let (clip, range, detection) = (editor.clip.clone(), editor.range, editor.detection);
        let python = self.pitch_python();
        editor.status = None;
        if let Err(error) = self.transcription.jobs.submit(
            &self.ctx,
            "Transcribing notes",
            Priority::Interactive,
            move || {
                Ok((
                    revision,
                    transcription::worker::analyze(&python, &clip, range, detection)
                        .map_err(|error| error.to_string()),
                ))
            },
        ) {
            editor.status = Some(error.to_string());
        }
    }

    pub fn transcription_job(&self) -> Option<(&'static str, crate::background::State, usize)> {
        self.transcription.jobs.current()
    }

    pub fn cancel_transcription_job(&mut self) {
        self.transcription.jobs.clear();
    }

    pub(super) fn poll_transcription(&mut self) {
        let Some(result) = self.transcription.jobs.poll(&self.ctx) else {
            return;
        };
        let Some(editor) = &mut self.transcription.editor else {
            return;
        };
        match result {
            Ok((revision, result)) if revision == self.transcription.revision => match result {
                Ok(notes) => {
                    editor.raw = Some(notes);
                    editor.align();
                }
                Err(error) => editor.status = Some(error),
            },
            Err(error) => editor.status = Some(error),
            _ => (),
        }
    }

    pub fn stop_transcription_preview(&mut self, editor: &mut Editor) {
        if std::mem::take(&mut editor.playing) {
            self.stop_all();
        }
    }

    pub fn close_transcription_editor(&mut self, editor: &mut Editor) {
        self.stop_transcription_preview(editor);
        self.cancel_transcription_job();
    }

    pub(super) fn clear_transcription(&mut self) {
        self.cancel_transcription_job();
        if let Some(mut editor) = self.transcription.editor.take() {
            self.stop_transcription_preview(&mut editor);
        }
    }

    pub fn listen_transcription_source(&mut self, editor: &mut Editor) {
        if self.output.is_none() || self.recording {
            return;
        }
        self.stop_all();
        self.send(Command::Listen(
            editor.clip.clone(),
            editor.range,
            0.8,
            None,
        ));
        editor.playing = true;
    }

    pub fn listen_transcription_notes(&mut self, editor: &mut Editor) {
        if !editor.notes.is_empty() && self.can_play_section() {
            editor.playing = self
                .audition_pattern(transcription::phrase(&editor.notes, editor.sound))
                .is_some();
        }
    }

    pub fn transcription_sounds(&self) -> Vec<(String, Instrument)> {
        let mut choices = vec![("Sine".into(), Instrument::Sine)];
        let mut add = |name: String, source, range, root| {
            if let Some(clip) = self.source_arc(source)
                && let Some(instrument) =
                    transcription::sample_instrument(source, &clip, range, root)
            {
                choices.push((name, instrument));
            }
        };
        if let Some(sample) = self.studio.chromatic {
            add(
                "Studio chromatic sample".into(),
                sample.source,
                (sample.start, sample.end),
                sample.root,
            );
        }
        if let Some(range) = self.selection() {
            add("Current selection".into(), self.active_source, range, 60);
        }
        for cut in &self.samples {
            add(cut.name.clone(), cut.source, cut.range(), 60);
        }
        choices
    }

    pub fn set_transcription_sound(&mut self, sound: Instrument) {
        let groups: HashSet<_> = self
            .studio
            .performance
            .iter()
            .filter_map(|note| {
                note.expression
                    .as_ref()
                    .and_then(|expression| expression.phrase)
            })
            .collect();
        self.observe_edits(false);
        for note in &mut self.studio.performance {
            if let Some(expression) = &mut note.expression
                && expression.instrument != sound
            {
                Arc::make_mut(expression).instrument = sound;
            }
        }
        for note in self
            .studio
            .patterns
            .iter_mut()
            .flat_map(|pattern| &mut pattern.notes.performance)
        {
            if let Some(expression) = &mut note.expression
                && expression.phrase.is_some_and(|id| groups.contains(&id))
                && expression.instrument != sound
            {
                Arc::make_mut(expression).instrument = sound;
            }
        }
        self.observe_edits(false);
    }

    pub fn commit_transcription(&mut self, editor: &mut Editor) -> files::Result<()> {
        if self.recording
            || editor.notes.is_empty()
            || self
                .source_arc(editor.source)
                .is_none_or(|clip| !Arc::ptr_eq(&clip.frames, &editor.clip.frames))
        {
            return Err("The transcription source or working state changed.".into());
        }
        let name = editor.name.trim();
        if name.is_empty() {
            return Err("Enter a name for the new pattern.".into());
        }
        let mut phrase = transcription::phrase(&editor.notes, editor.sound);
        if phrase
            .performance
            .windows(2)
            .any(|pair| pair[0].tick == pair[1].tick && pair[0].input == pair[1].input)
        {
            return Err("Move or remove one of the notes sharing the same pitch and start.".into());
        }
        let id = self.random().max(1);
        for note in &mut phrase.performance {
            if let Some(expression) = &mut note.expression {
                Arc::make_mut(expression).phrase = Some(id);
            }
        }
        let bars = phrase.bars.len();
        if !(1..=MAX_SONG_BARS).contains(&bars) {
            return Err("The phrase exceeds the song length limit.".into());
        }
        let parts = transcription::split(phrase, || self.random().max(1));
        let linked = parts.len() > 1;
        self.observe_edits(false);
        let mut project = self.project_snapshot();
        let studio = &mut project.studio;
        if linked
            && (studio.sections.len() + parts.len() > MAX_SECTIONS
                || studio.song_bars() + bars > MAX_SONG_BARS)
        {
            return Err("There is not enough arrangement space for the linked patterns.".into());
        }
        let mut first = None;
        for (index, part) in parts.into_iter().enumerate() {
            let name = if linked {
                let suffix = format!(" {}", index + 1);
                name.chars()
                    .take(MAX_SAMPLE_NAME - suffix.len())
                    .collect::<String>()
                    + &suffix
            } else {
                name.to_owned()
            };
            if studio.save_generated_pattern(&name, part).is_none() {
                return Err(
                    "Choose a unique pattern name and make room for the new patterns.".into(),
                );
            }
            let pattern = studio.editing_pattern.unwrap();
            first.get_or_insert(pattern);
            if linked {
                studio
                    .sections
                    .push(Section::new(SectionKind::Verse, pattern, 1));
            }
        }
        studio.load_pattern(first.unwrap());
        if serde_json::to_vec(&project)?.len() > files::MAX_PROJECT_METADATA {
            return Err("The phrase would exceed the project metadata limit.".into());
        }
        self.stop_all();
        self.studio = project.studio;
        self.reset_pattern_editor();
        self.studio_view = StudioView::Sequence;
        self.observe_edits(false);
        self.piano_roll = Some(crate::ui::piano_roll::PianoRoll::keys(self));
        self.status = Some((
            if linked {
                "Imported linked transcription patterns.".into()
            } else {
                "Imported transcription as a new pattern.".into()
            },
            false,
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TICKS_PER_BAR;

    #[test]
    fn commits_linked_patterns_and_changes_their_instrument_as_undoable_edits() -> files::Result<()>
    {
        let mut app = App::new(eframe::egui::Context::default());
        let clip = Arc::new(Clip::new(1_000, vec![[0.2; 2]; 34_000]));
        app.clip = Some(clip.clone());
        app.studio.bpm = 120;
        app.observe_edits(false);
        app.session.reset(app.project_snapshot(), None, None);
        let before = app.session.current.clone();
        app.open_transcription();
        let mut editor = app.transcription.editor.take().unwrap();
        editor.raw = Some(vec![
            Candidate {
                start: 31.9,
                end: 32.2,
                pitch: 40,
                strength: 0.7,
                bends: vec![0, 33, 67],
            },
            Candidate {
                start: 32.25,
                end: 32.75,
                pitch: 45,
                strength: 0.6,
                bends: Vec::new(),
            },
        ]);
        editor.align();
        app.commit_transcription(&mut editor)?;
        assert_eq!(app.studio.sections.len(), 2);
        let song = app.studio.song_sequence().unwrap();
        assert_eq!(song.ticks(), 17 * TICKS_PER_BAR);
        assert_eq!(song.performance.len(), 2);
        let timing = |notes: &[PlayedChop]| {
            notes
                .iter()
                .map(|note| (note.tick, note.length))
                .collect::<Vec<_>>()
        };
        let sound = transcription::sample_instrument(0, &clip, (0.0, 0.8), 48).unwrap();
        app.set_transcription_sound(sound);
        let updated = app.studio.song_sequence().unwrap();
        assert_eq!(timing(&updated.performance), timing(&song.performance));
        assert!(
            updated
                .performance
                .iter()
                .all(|note| note.expression.as_ref().unwrap().instrument == sound)
        );
        app.undo_project(false);
        assert_eq!(app.studio.song_sequence(), Some(song));
        app.undo_project(false);
        assert!(app.session.current == before);
        assert!(!app.can_undo_project(false));
        Ok(())
    }
}
