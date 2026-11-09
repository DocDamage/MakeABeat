use super::{BarContent, ChopInput, DRUMS, PlayedChop, STEPS, Studio, TICKS_PER_STEP};
use std::ops::RangeInclusive;

pub const LANES: usize = DRUMS.len() + 2;

pub fn lane_name(row: usize) -> &'static str {
    match row {
        0 => "Chops",
        1 => "Keys",
        _ => DRUMS[row - 2],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub lane: usize,
    pub step: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridRect {
    pub first: Cell,
    pub last: Cell,
}

impl GridRect {
    pub fn between(a: Cell, b: Cell) -> Self {
        Self {
            first: Cell {
                lane: a.lane.min(b.lane),
                step: a.step.min(b.step),
            },
            last: Cell {
                lane: a.lane.max(b.lane),
                step: a.step.max(b.step),
            },
        }
    }

    pub fn lanes(self) -> RangeInclusive<usize> {
        self.first.lane..=self.last.lane
    }

    pub fn steps(self) -> RangeInclusive<usize> {
        self.first.step..=self.last.step
    }

    pub fn contains(self, cell: Cell) -> bool {
        self.lanes().contains(&cell.lane) && self.steps().contains(&cell.step)
    }

    fn includes_note(self, note: &PlayedChop) -> bool {
        let lane = usize::from(matches!(note.input, ChopInput::Key(_)));
        self.contains(Cell {
            lane,
            step: (note.tick / TICKS_PER_STEP) as usize,
        })
    }
}

#[derive(Clone)]
pub struct GridClipboard {
    region: GridRect,
    content: BarContent,
}

impl GridClipboard {
    pub fn copy(studio: &Studio, bar: usize, region: GridRect) -> Self {
        Self {
            region,
            content: studio.copy_bar(bar),
        }
    }

    pub fn destination(&self, at: Cell) -> Option<GridRect> {
        let last = Cell {
            lane: at.lane + self.region.lanes().count() - 1,
            step: at.step + self.region.steps().count() - 1,
        };
        // The pad and key rows have their own note types. Drum rows share one format.
        let same_type =
            at.lane == self.region.first.lane || (at.lane >= 2 && self.region.first.lane >= 2);
        (same_type && last.lane < LANES && last.step < STEPS).then(|| GridRect::between(at, last))
    }

    pub fn paste(&self, studio: &mut Studio, bar: usize, at: Cell) -> Option<GridRect> {
        let target = self.destination(at)?;
        let mut destination = studio.copy_bar(bar);
        clear(&mut destination, target);
        for (from_lane, to_lane) in self.region.lanes().zip(target.lanes()) {
            for (from_step, to_step) in self.region.steps().zip(target.steps()) {
                match from_lane {
                    0 => destination.steps.pads[to_step] = self.content.steps.pads[from_step],
                    1 => destination.steps.keys[to_step] = self.content.steps.keys[from_step],
                    _ => {
                        destination.steps.drums[to_lane - 2][to_step] =
                            self.content.steps.drums[from_lane - 2][from_step]
                    }
                }
                if let Some(&rule) = self
                    .content
                    .steps
                    .rules
                    .get(&rule_key(from_lane, from_step))
                {
                    destination
                        .steps
                        .rules
                        .insert(rule_key(to_lane, to_step), rule);
                }
                if let Some(&feel) = self.content.steps.feel.get(&rule_key(from_lane, from_step)) {
                    destination.steps.feel.insert(rule_key(to_lane, to_step), feel);
                }
                if let Some(&sound) = self.content.steps.sounds.get(&rule_key(from_lane, from_step)) {
                    destination.steps.sounds.insert(rule_key(to_lane, to_step), sound);
                }
            }
        }
        let source_tick = self.region.first.step as u32 * TICKS_PER_STEP;
        let destination_tick = target.first.step as u32 * TICKS_PER_STEP;
        destination.notes.extend(
            self.content
                .notes
                .iter()
                .filter(|note| self.region.includes_note(note))
                .map(|note| PlayedChop {
                    id: Default::default(),
                    tick: note.tick - source_tick + destination_tick,
                    ..note.clone()
                }),
        );
        studio.replace_bar(bar, &destination);
        Some(target)
    }
}

pub fn clear_cells(studio: &mut Studio, bar: usize, region: GridRect) {
    let mut content = studio.copy_bar(bar);
    clear(&mut content, region);
    studio.replace_bar(bar, &content);
}

fn clear(content: &mut BarContent, region: GridRect) {
    for lane in region.lanes() {
        for step in region.steps() {
            match lane {
                0 => content.steps.pads[step] = None,
                1 => content.steps.keys[step] = None,
                _ => content.steps.drums[lane - 2][step] = 0,
            }
            content.steps.rules.remove(&rule_key(lane, step));
            content.steps.sounds.remove(&rule_key(lane, step));
            content.steps.ids.remove(&rule_key(lane, step));
            content.steps.feel.remove(&rule_key(lane, step));
        }
    }
    content.notes.retain(|note| !region.includes_note(note));
}

fn rule_key(lane: usize, step: usize) -> u8 {
    (lane * STEPS + step) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{StepRule, TICKS_PER_BAR};

    #[test]
    fn copying_hit_sound_assigns_new_ids_and_keeps_other_cells() {
        let mut studio = Studio { bars: 2, ..Studio::default() };
        studio.chops[0][1] = Some(0);
        studio.chops[1][9] = Some(1);
        let sound = crate::model::instrument::HitSound {
            pitch_cents: -700, cutoff_hz: Some(320), start_percent: 28,
        };
        studio.set_step_sound(0, 0, 1, sound);
        studio.identify_working_events();
        let original_id = studio.step_ids[0][&1];
        let untouched = studio.step_ids[1][&9];
        let cell = Cell { lane: 0, step: 1 };
        GridClipboard::copy(&studio, 0, GridRect::between(cell, cell))
            .paste(&mut studio, 1, Cell { lane: 0, step: 3 }).unwrap();
        studio.identify_working_events();
        assert_eq!(studio.step_ids[0][&1], original_id);
        assert_eq!(studio.step_ids[1][&9], untouched);
        assert_ne!(studio.step_ids[1][&3], original_id);
        assert_eq!(studio.step_sounds[1][&3], sound);
        studio.set_step_sound(1, 0, 3, Default::default());
        assert_eq!(studio.step_sounds[0][&1], sound);
        studio.save_pattern("Original", false);
        studio.save_pattern("Duplicate", false);
        studio.identify_working_events();
        assert_eq!(studio.patterns[0].notes.bars[0].ids[&1], original_id);
        assert_ne!(studio.step_ids[0][&1], original_id);
        assert_eq!(studio.step_sounds[0][&1], sound);
    }

    #[test]
    fn rectangle_paste_moves_conditions_and_notes_without_touching_other_cells() {
        let mut studio = Studio {
            bars: 2,
            ..Studio::default()
        };
        studio.chops[0][1] = Some(4);
        studio.hits[0][0][2] = 70;
        studio.set_step_rule(
            0,
            2,
            2,
            StepRule {
                repeats: 3,
                ..StepRule::default()
            },
        );
        studio.performance.push(PlayedChop {
            id: Default::default(),
            tick: TICKS_PER_STEP + 13,
            input: ChopInput::Pad(4),
            velocity: 100,
            mode: crate::audio::PadMode::Gate,
            length: Some(84),
            sound: Default::default(),
            expression: None,
        });
        studio.chops[1][2] = Some(2);
        studio.chops[1][8] = Some(8);
        studio.performance.push(PlayedChop {
            id: Default::default(),
            tick: TICKS_PER_BAR + 2 * TICKS_PER_STEP + 10,
            input: ChopInput::Pad(2),
            velocity: 91,
            mode: crate::audio::PadMode::Slice,
            length: None,
            sound: Default::default(),
            expression: None,
        });

        let rect = GridRect::between(Cell { lane: 2, step: 2 }, Cell { lane: 0, step: 1 });
        let clipboard = GridClipboard::copy(&studio, 0, rect);
        assert!(clipboard.destination(Cell { lane: 1, step: 2 }).is_none());
        assert!(clipboard.destination(Cell { lane: 0, step: 15 }).is_none());
        clipboard
            .paste(&mut studio, 1, Cell { lane: 0, step: 2 })
            .unwrap();
        assert_eq!(studio.chops[1][2], Some(4));
        assert_eq!(studio.chops[1][8], Some(8));
        assert_eq!(studio.hits[1][0][3], 70);
        assert_eq!(studio.step_rules[1][&rule_key(2, 3)].repeats, 3);
        assert_eq!(studio.performance.len(), 2);
        assert_eq!(
            studio.performance[1].tick,
            TICKS_PER_BAR + 2 * TICKS_PER_STEP + 13
        );
        assert_eq!(studio.performance[1].length, Some(84));
    }
}
