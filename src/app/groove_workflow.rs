use super::App;
use crate::model::{
    Sequence,
    groove::{MAX_TEMPLATES, Template, Transfer},
};
use std::{fs, io::Read};

#[derive(Default)]
pub(crate) struct GrooveEditor {
    pub draft: Option<Template>,
    pub saved: Option<usize>,
    pub transfer: Transfer,
    comparison: Option<u64>,
}

impl App {
    pub(super) fn accept_groove(&mut self, draft: Template) {
        self.groove_editor.draft = Some(draft);
        self.groove_editor.saved = None;
    }

    pub fn select_groove(&mut self, index: usize) {
        if let Some(template) = self.studio.grooves.get(index) {
            self.groove_editor.draft = Some(template.clone());
            self.groove_editor.saved = Some(index);
        }
    }

    pub fn save_groove(&mut self) {
        let Some(mut template) = self.groove_editor.draft.clone() else {
            return;
        };
        let index = self
            .groove_editor
            .saved
            .filter(|&index| index < self.studio.grooves.len());
        if !template.validate()
            || self.studio.grooves.iter().enumerate().any(|(i, saved)| {
                Some(i) != index && saved.name.eq_ignore_ascii_case(&template.name)
            })
        {
            self.status = Some(("Give the groove a unique name.".into(), true));
            return;
        }
        if index.is_none() && self.studio.grooves.len() >= MAX_TEMPLATES {
            self.status = Some(("The project groove library is full.".into(), true));
            return;
        }
        self.observe_edits(false);
        if let Some(index) = index {
            self.studio.grooves[index] = template.clone();
        } else {
            self.groove_editor.saved = Some(self.studio.grooves.len());
            self.studio.grooves.push(template.clone());
        }
        self.groove_editor.draft = Some(template);
        self.observe_edits(false);
    }

    pub fn import_groove(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Groove template", &["groove"])
            .pick_file()
        else {
            return;
        };
        let result = (|| -> crate::files::Result<Template> {
            let mut bytes = Vec::new();
            fs::File::open(path)?.take(32_769).read_to_end(&mut bytes)?;
            if bytes.len() > 32_768 {
                return Err("The groove template is too large.".into());
            }
            let mut template: Template = serde_json::from_slice(&bytes)?;
            if !template.validate() {
                return Err("The groove template is invalid.".into());
            }
            Ok(template)
        })();
        match result {
            Ok(template) => self.accept_groove(template),
            Err(error) => self.status = Some((error.to_string(), true)),
        }
    }

    pub fn export_groove(&mut self) {
        let Some(mut template) = self
            .groove_editor
            .draft
            .clone()
            .filter(|t| !t.name.trim().is_empty())
        else {
            return;
        };
        if !template.validate() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Groove template", &["groove"])
            .set_file_name("template.groove")
            .save_file()
        else {
            return;
        };
        let result = crate::files::write_atomically(&path, |file| {
            serde_json::to_writer_pretty(file, &template)?;
            Ok(())
        });
        self.status = Some(match result {
            Ok(()) => ("Groove template exported.".into(), false),
            Err(error) => (error.to_string(), true),
        });
    }

    fn groove_versions(&self) -> Option<(Sequence, Sequence)> {
        let template = self.groove_editor.draft.as_ref()?;
        let before = self.studio.active_sequence();
        let after = self.groove_editor.transfer.bake(before.clone(), template);
        Some((before, after))
    }

    pub fn preview_groove(&mut self, processed: bool) {
        if let Some((before, after)) = self.groove_versions() {
            self.groove_editor.comparison =
                self.audition_pattern(if processed { after } else { before });
        }
    }

    pub fn groove_preview_playing(&self) -> bool {
        self.groove_editor.comparison.is_some()
            && self.groove_editor.comparison == self.song_playing
    }

    pub fn stop_groove_preview(&mut self) {
        if self.groove_preview_playing() {
            self.stop_all();
        }
    }

    pub fn apply_groove(&mut self) {
        if self.recording {
            return;
        }
        self.observe_edits(false);
        let Some((before, after)) = self.groove_versions() else {
            return;
        };
        if before == after {
            self.status = Some((
                "The selected groove settings leave these hits as they are.".into(),
                false,
            ));
            return;
        }
        self.stop_groove_preview();
        for (index, bar) in after.bars.into_iter().enumerate() {
            self.studio.hits[index] = bar.drums;
            self.studio.step_feel[index] = bar.feel;
        }
        let end = self
            .studio
            .performance
            .partition_point(|note| note.tick < self.studio.pattern_ticks());
        self.studio.performance.splice(..end, after.performance);
        self.observe_edits(false);
        self.status = Some(("Applied groove to the working pattern.".into(), false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        STEPS,
        groove::{Provenance, Slot},
    };

    #[test]
    fn baking_is_one_undo_and_preserves_hidden_bars() {
        let mut app = App::new(eframe::egui::Context::default());
        app.studio.hits[0][0][1] = 1;
        app.studio.hits[1][0][1] = 2;
        app.studio.save_pattern("Working", false);
        app.observe_edits(false);
        app.session.reset(app.project_snapshot(), None, None);
        let before = app.session.current.clone();
        app.accept_groove(Template {
            name: "Timing test".into(),
            bars: 1,
            slots: vec![
                Slot {
                    offset: 20,
                    accent: 1.0
                };
                STEPS
            ],
            source: Provenance {
                title: "Break".into(),
                range: (0.0, 2.0),
                fingerprint: "test".into(),
            },
        });
        app.apply_groove();
        assert_eq!(app.studio.step_feel[0][&(2 * STEPS as u8 + 1)].offset, 20);
        assert_eq!(app.studio.bar_notes(1), before.studio.bar_notes(1));
        assert!(app.project_dirty());
        app.undo_project(false);
        assert!(app.session.current == before);
        assert!(!app.can_undo_project(false));
    }
}
