use super::App;
use crate::model::{
    ChopInput, DRUMS, MAX_BARS, STEPS, StepRule, TICKS_PER_BAR, TICKS_PER_STEP, drum_velocity,
    encode_drum_velocity,
    grid_edit::{Cell, GridClipboard, GridRect, clear_cells},
    groove::{HitFeel, set_feel},
    instrument::HitSound,
};

pub(crate) enum BarEdit {
    SetCount(usize),
    Paste,
    Duplicate,
    Insert,
    Remove,
}

pub(crate) enum Edit<'a> {
    Toggle(Cell),
    Clear(Cell),
    DrumVelocity(Cell, u8),
    ClearProgrammed(ClearLane),
    ClearCells(GridRect),
    PasteCells(&'a GridClipboard, Cell),
    Bar(BarEdit),
    LaneTiming(usize, i8),
    ResetLaneTiming,
    Condition(Cell, StepRule),
    Feel(Cell, HitFeel),
    Sound(Cell, HitSound),
}

#[derive(Clone, Copy)]
pub(crate) enum ClearLane {
    Pads,
    Keys,
    Drums,
}

impl App {
    pub(crate) fn edit_sequence<'a>(
        &mut self,
        edits: impl IntoIterator<Item = Edit<'a>>,
        group: bool,
    ) {
        let mut edits = edits.into_iter().peekable();
        if edits.peek().is_none() {
            return;
        }
        for edit in edits {
            self.apply_sequence_edit(edit);
        }
        self.observe_edits(group || self.recording);
    }

    fn apply_sequence_edit(&mut self, edit: Edit<'_>) {
        let bar = self.bar;
        match edit {
            Edit::Toggle(cell) => self.toggle_sequence_step(cell),
            Edit::Clear(Cell { lane, step }) => {
                match lane {
                    0 => self.project.studio.chops[bar][step] = None,
                    1 => self.project.studio.key_steps[bar][step] = None,
                    _ => self.project.studio.hits[bar][lane - 2][step] = 0,
                }
                if lane < 2 {
                    let start = bar as u32 * TICKS_PER_BAR + step as u32 * TICKS_PER_STEP;
                    self.project.studio.performance.retain(|event| {
                        !(start..start + TICKS_PER_STEP).contains(&event.tick)
                            || usize::from(matches!(event.input, ChopInput::Key(_))) != lane
                    });
                }
            }
            Edit::DrumVelocity(cell, velocity) => {
                self.project.studio.hits[bar][cell.lane - 2][cell.step] = encode_drum_velocity(velocity);
            }
            Edit::ClearProgrammed(lane) => match lane {
                ClearLane::Pads => self.project.studio.chops[bar] = [None; STEPS],
                ClearLane::Keys => self.project.studio.key_steps[bar] = [None; STEPS],
                ClearLane::Drums => self.project.studio.hits[bar] = [[0; STEPS]; DRUMS.len()],
            },
            Edit::ClearCells(region) => clear_cells(&mut self.project.studio, bar, region),
            Edit::PasteCells(clipboard, target) => {
                clipboard.paste(&mut self.project.studio, bar, target);
            }
            Edit::Bar(edit) => self.edit_bar(edit),
            Edit::LaneTiming(lane, offset) => self.project.studio.lane_offsets[lane] = offset,
            Edit::ResetLaneTiming => self.project.studio.lane_offsets.fill(0),
            Edit::Condition(cell, rule) => {
                self.project.studio.set_step_rule(bar, cell.lane, cell.step, rule)
            }
            Edit::Feel(cell, feel) => {
                set_feel(
                    &mut self.project.studio.step_feel[bar],
                    (cell.lane * STEPS + cell.step) as u8,
                    feel,
                );
            }
            Edit::Sound(cell, sound) => {
                self.project.studio.set_step_sound(bar, cell.lane, cell.step, sound)
            }
        }
    }

    fn toggle_sequence_step(&mut self, Cell { lane, step }: Cell) {
        match lane {
            0 => {
                let pad = self.selected_pad;
                let slot = &mut self.project.studio.chops[self.bar][step];
                if *slot == Some(pad) {
                    *slot = None;
                } else if self.project.studio.pads[pad].is_some() {
                    *slot = Some(pad);
                }
            }
            1 => {
                let note = self.piano_key;
                let assigned = self.key_range(note).is_some();
                let slot = &mut self.project.studio.key_steps[self.bar][step];
                if *slot == Some(note) {
                    *slot = None;
                } else if assigned {
                    *slot = Some(note);
                }
            }
            _ => {
                let slot = &mut self.project.studio.hits[self.bar][lane - 2][step];
                *slot = encode_drum_velocity(if drum_velocity(*slot) == 0 { 89 } else { 0 });
            }
        }
    }

    fn edit_bar(&mut self, edit: BarEdit) {
        if self.recording {
            return;
        }
        if let BarEdit::SetCount(count) = edit {
            self.project.studio.bars = count.clamp(1, MAX_BARS);
            self.bar = self.bar.min(self.project.studio.bars - 1);
            return;
        }
        if self.song_playing.is_some() {
            return;
        }
        let bar = self.bar;
        match edit {
            BarEdit::Paste => {
                if let Some(content) = &self.bar_clipboard {
                    self.project.studio.paste_bar(bar, content);
                }
            }
            BarEdit::Insert | BarEdit::Duplicate if self.project.studio.bars < MAX_BARS => {
                let content = matches!(edit, BarEdit::Duplicate).then(|| self.project.studio.copy_bar(bar));
                self.project.studio.insert_bar(bar + 1);
                self.bar = bar + 1;
                if let Some(content) = content {
                    self.project.studio.paste_bar(self.bar, &content);
                }
            }
            BarEdit::Remove => {
                self.project.studio.remove_bar(bar);
                self.bar = bar.min(self.project.studio.bars - 1);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{Command, Mixer};
    use eframe::egui;

    #[test]
    fn working_pattern_edits_reach_running_playback_and_named_pattern_undo() {
        let mut app = App::new(egui::Context::default());
        app.audio.remove_output();
        app.project.studio.bpm = 120;
        app.project.studio.save_pattern("Live", false);
        app.observe_edits(false);
        app.session.reset(app.project.clone(), None, None);
        app.playing = true;

        let mut mixer = Mixer::new(8_000);
        mixer.command(Command::Settings(Box::new(app.audio.settings().clone())));
        mixer.command(Command::Pattern(true));
        assert!((0..1_000).all(|_| mixer.next_frame() == [0.0; 2]));

        let step = |step| Cell { lane: 2, step };
        app.edit_sequence([Edit::DrumVelocity(step(2), 89)], true);
        app.edit_sequence([Edit::DrumVelocity(step(3), 89)], true);
        app.observe_edits(false);
        assert_eq!(
            drum_velocity(app.project.studio.patterns[0].notes.bars[0].drums[0][3]),
            89
        );
        mixer.command(Command::Settings(Box::new(app.audio.settings().clone())));
        assert!((0..2_000).any(|_| mixer.next_frame() != [0.0; 2]));

        app.undo_project(false);
        assert!(app.playing);
        assert!(!app.can_undo_project(false));
        assert_eq!(&app.audio.settings().hits[0][0][2..4], &[0, 0]);
        assert_eq!(
            &app.project.studio.patterns[0].notes.bars[0].drums[0][2..4],
            &[0, 0]
        );
        app.undo_project(true);
        assert_eq!(&app.audio.settings().hits[0][0][2..4], &[encode_drum_velocity(89); 2]);
    }

    #[test]
    fn duplicating_a_bar_is_one_undoable_edit() {
        let mut app = App::new(egui::Context::default());
        app.audio.remove_output();
        app.project.studio.hits[0][0][0] = encode_drum_velocity(110);
        app.observe_edits(false);
        app.session.reset(app.project.clone(), None, None);

        app.edit_sequence([Edit::Bar(BarEdit::Duplicate)], false);
        assert_eq!((app.bar, app.audio.settings().bars), (1, 2));
        assert_eq!(app.audio.settings().hits[1][0], app.audio.settings().hits[0][0]);
        app.undo_project(false);
        assert_eq!((app.bar, app.audio.settings().bars), (0, 1));
        assert!(!app.can_undo_project(false));
        app.undo_project(true);
        assert_eq!(app.audio.settings().bars, 2);
        assert_eq!(app.audio.settings().hits[1][0], app.audio.settings().hits[0][0]);
    }
}
