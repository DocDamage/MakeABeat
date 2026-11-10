use super::App;
use crate::model::{ChopInput, STEPS};

pub(crate) enum Edit {
    Assign {
        pad: usize,
        source: usize,
        range: (f64, f64),
        name: Option<String>,
    },
    Copy {
        from: usize,
        to: usize,
    },
    Swap {
        first: usize,
        second: usize,
    },
    LinkEnd {
        pad: usize,
        linked: bool,
    },
    Clear(usize),
    ClearAll,
}

impl App {
    pub(crate) fn edit_pad(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::Assign {
                pad,
                source,
                range: (start, end),
                name,
            } if pad < STEPS
                && start.is_finite()
                && end.is_finite()
                && start >= 0.0
                && start < end
                && end <= self.source_duration(source) =>
            {
                self.assign_pad_range(pad, source, (start, end));
                if let Some(name) = name {
                    self.project.studio.pad_names[pad] = name;
                }
            }
            Edit::Copy { from, to }
                if from < STEPS && to < STEPS && self.project.studio.pads[to].is_none() =>
            {
                let Some(range) = self.pad_range(from) else {
                    return false;
                };
                self.assign_pad_range(to, self.project.studio.pad_sources[from], range);
                self.project.studio.pad_names[to] = self.project.studio.pad_names[from].clone();
                self.project.studio.pad_colors[to] = self.project.studio.pad_colors[from];
                self.project.studio.pad_controls[to] = self.project.studio.pad_controls[from];
                self.project.studio.choke_groups.pads[to] = self.project.studio.choke_groups.pads[from];
                if let Some(layers) = self.project.studio.instruments.pads.get(&from).copied() {
                    self.project.studio.instruments.pads.insert(to, layers);
                }
            }
            Edit::Swap { first, second } if first < STEPS && second < STEPS && first != second => {
                self.swap_pads(first, second);
            }
            Edit::LinkEnd { pad, linked } if pad < STEPS => {
                let Some((_, end)) = self.pad_range(pad) else {
                    return false;
                };
                self.project.studio.pad_ends[pad] = (!linked).then_some(end);
            }
            Edit::Clear(pad) if pad < STEPS => self.clear_pad(pad),
            Edit::ClearAll => {
                for pad in 0..STEPS {
                    self.clear_pad(pad);
                }
            }
            _ => return false,
        }
        self.observe_edits(self.recording);
        true
    }

    fn swap_pads(&mut self, first: usize, second: usize) {
        // Resolve linked ends before moving either start marker.
        for pad in [first, second] {
            if let Some((_, end)) = self.pad_range(pad) {
                self.project.studio.pad_ends[pad] = Some(end);
            }
        }
        self.project.studio.pads.swap(first, second);
        self.project.studio.pad_ends.swap(first, second);
        self.project.studio.pad_sources.swap(first, second);
        self.project.studio.pad_names.swap(first, second);
        self.project.studio.pad_colors.swap(first, second);
        self.project.studio.pad_controls.swap(first, second);
        self.project.studio.choke_groups.pads.swap(first, second);
        let a = self.project.studio.instruments.pads.remove(&first);
        let b = self.project.studio.instruments.pads.remove(&second);
        self.project.studio
            .instruments
            .pads
            .extend(a.map(|layers| (second, layers)));
        self.project.studio
            .instruments
            .pads
            .extend(b.map(|layers| (first, layers)));
        self.selected_pad = second;
    }

    fn clear_pad(&mut self, index: usize) {
        self.project.studio.pads[index] = None;
        self.project.studio.pad_ends[index] = None;
        self.project.studio.pad_sources[index] = 0;
        self.project.studio.pad_names[index].clear();
        self.project.studio.pad_colors[index] = 0;
        self.project.studio.choke_groups.pads[index] = 0;
        self.project.studio.instruments.pads.remove(&index);
        self.project.studio
            .performance
            .retain(|event| event.input != ChopInput::Pad(index as u8));
        for cell in self.project.studio.chops.iter_mut().flatten() {
            if *cell == Some(index) {
                *cell = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Clip;
    use std::sync::Arc;

    #[test]
    fn named_assignment_copy_and_bulk_clear_reach_playback_and_undo() {
        let mut app = App::new(eframe::egui::Context::default());
        app.audio.remove_output();
        app.media.replace(Some(Arc::new(Clip::new(8_000, vec![[0.25; 2]; 8_000]))));
        app.session.reset(app.project.clone(), None, None);

        assert!(app.edit_pad(Edit::Assign {
            pad: 0,
            source: 0,
            range: (0.25, 0.5),
            name: Some("Slice".into()),
        }));
        assert!(app.edit_pad(Edit::Copy { from: 0, to: 1 }));
        assert_eq!(app.audio.settings().pads[..2], [Some(0.25); 2]);
        assert_eq!(app.audio.settings().pad_ends[..2], [Some(0.5); 2]);
        assert_eq!(app.project.studio.pad_names[..2], ["Slice", "Slice"]);

        app.project.studio.chops[0][0] = Some(1);
        app.observe_edits(false);
        assert!(app.edit_pad(Edit::ClearAll));
        assert_eq!(app.audio.settings().pads[..2], [None; 2]);
        assert_eq!(app.audio.settings().chops[0][0], None);

        app.undo_project(false);
        assert_eq!(app.audio.settings().pads[..2], [Some(0.25); 2]);
        assert_eq!(app.audio.settings().chops[0][0], Some(1));
        assert_eq!(app.project.studio.pad_names[..2], ["Slice", "Slice"]);
        app.undo_project(true);
        assert_eq!(app.audio.settings().pads[..2], [None; 2]);
    }
}
