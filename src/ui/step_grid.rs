use super::{
    BLUE, EDGE, FAINT, GOLD, INSET, MINT, TEXT, action, label, layers, named_number,
    sequencer::Tool, small,
};
use crate::{
    app::{App, sequence_workflow::Edit},
    audio::Command,
    model::{
        ChopInput, STEPS, TICKS_PER_BAR, TICKS_PER_STEP, drum_velocity,
        grid_edit::{Cell, GridClipboard, GridRect, LANES, lane_name},
        piano_note,
    },
};
use eframe::egui::{
    self, Align2, Button, Color32, ComboBox, DragValue, FontId, Label, Pos2, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, vec2,
};

#[derive(Clone)]
pub(super) struct State {
    pub selection: Selection,
    pub velocity_cell: Option<Cell>,
    rule_cell: Cell,
    gesture: Option<u8>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selection: Selection::default(),
            velocity_cell: None,
            rule_cell: Cell { lane: 2, step: 0 },
            gesture: None,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct Selection {
    enabled: bool,
    anchor: Option<Cell>,
    corner: Option<Cell>,
    dragging: bool,
    bar: usize,
    clipboard: Option<GridClipboard>,
}

impl Selection {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn region(&self) -> Option<GridRect> {
        Some(GridRect::between(self.anchor?, self.corner?))
    }

    pub fn pointer(&mut self, start: Option<Cell>, end: Option<Cell>, down: bool) {
        if let Some(cell) = start {
            self.anchor = Some(cell);
            self.corner = Some(cell);
            self.dragging = true;
        }
        if self.dragging
            && let Some(cell) = end
        {
            self.corner = Some(cell);
        }
        if !down {
            self.dragging = false;
        }
    }

    pub fn set_bar(&mut self, bar: usize) {
        if self.bar != bar {
            self.bar = bar;
            self.anchor = None;
            self.corner = None;
            self.dragging = false;
        }
    }

    pub fn toggle(&mut self, ui: &mut Ui, editable: bool) {
        if ui
            .add_enabled(
                editable,
                egui::Checkbox::new(&mut self.enabled, "Select cells"),
            )
            .on_hover_text(
                "Drag a rectangle, then use Sequence tools to copy, cut, paste or clear it",
            )
            .changed()
        {
            self.dragging = false;
        }
    }

    pub fn controls(&mut self, app: &mut App, ui: &mut Ui) {
        let editable = !app.recording && app.song_playing.is_none();
        ui.add_enabled_ui(editable, |ui| {
            if ui
                .checkbox(&mut self.enabled, "Select cells on the grid")
                .changed()
            {
                self.dragging = false;
            }
            ui.horizontal_wrapped(|ui| {
                let region = self.region();
                if action(ui, "Copy cells", region.is_some()).clicked()
                    && let Some(region) = region
                {
                    self.clipboard = Some(GridClipboard::copy(&app.studio, app.bar, region));
                }
                if action(ui, "Cut cells", region.is_some()).clicked()
                    && let Some(region) = region
                {
                    self.clipboard = Some(GridClipboard::copy(&app.studio, app.bar, region));
                    app.edit_sequence([Edit::ClearCells(region)], false);
                }
                let target = region.map(|region| region.first);
                let can_paste = self
                    .clipboard
                    .as_ref()
                    .zip(target)
                    .is_some_and(|(clipboard, target)| clipboard.destination(target).is_some());
                if action(ui, "Paste cells", can_paste)
                    .on_hover_text("Replace cells starting at the upper left of the selection")
                    .on_disabled_hover_text(
                        "Copy cells, then select a compatible destination with enough room",
                    )
                    .clicked()
                    && let Some((clipboard, target)) = self.clipboard.as_ref().zip(target)
                    && let Some(result) = clipboard.destination(target)
                {
                    app.edit_sequence([Edit::PasteCells(clipboard, target)], false);
                    self.anchor = Some(result.first);
                    self.corner = Some(result.last);
                }
                if action(ui, "Clear cells", region.is_some()).clicked()
                    && let Some(region) = region
                {
                    app.edit_sequence([Edit::ClearCells(region)], false);
                }
                if let Some(region) = self.region() {
                    small(
                        ui,
                        format!(
                            "{} lanes × {} steps",
                            region.lanes().count(),
                            region.steps().count()
                        ),
                    );
                }
            });
            ui.horizontal_wrapped(|ui| {
                let mut anchor = self.anchor.unwrap_or(Cell { lane: 0, step: 0 });
                let mut corner = self.corner.unwrap_or(anchor);
                let changed = corner_controls(ui, "Start", &mut anchor)
                    | corner_controls(ui, "End", &mut corner);
                if changed || action(ui, "Set selection", self.region().is_none()).clicked() {
                    self.anchor = Some(anchor);
                    self.corner = Some(corner);
                }
            });
            small(
                ui,
                "Set both corners here or drag across cells on the grid.",
            );
        });
    }
}

fn corner_controls(ui: &mut Ui, label: &str, cell: &mut Cell) -> bool {
    let old = *cell;
    ComboBox::from_id_salt(label)
        .selected_text(format!("{label} lane · {}", lane_name(cell.lane)))
        .show_ui(ui, |ui| {
            for lane in 0..LANES {
                ui.selectable_value(&mut cell.lane, lane, lane_name(lane));
            }
        });
    let mut step = cell.step + 1;
    let response = ui.add(
        DragValue::new(&mut step)
            .range(1..=STEPS)
            .prefix(format!("{label} step ")),
    );
    named_number(response, &format!("{label} step"), step as f64);
    cell.step = step - 1;
    *cell != old
}

pub(super) fn show(app: &mut App, ui: &mut Ui, state: &mut State) {
    let selection = &mut state.selection;
    let can_select = !app.recording && app.song_playing.is_none();
    let selecting = selection.enabled() && can_select;
    let name_width = 77.0;
    let gap = 4.0;
    let step_width =
        ((ui.available_width() - name_width - gap * STEPS as f32) / STEPS as f32).max(20.0);
    let bar = app.bar;
    let cursor = app.step.filter(|_| app.song_playing.is_none());
    let (pressed, down, modifiers) = ui.input(|input| {
        (
            input.pointer.primary_pressed(),
            input.pointer.primary_down(),
            input.modifiers,
        )
    });
    if pressed || !down || selecting {
        state.gesture = None;
    }
    let (press_at, pointer_at) = ui.input(|input| {
        let press = input.events.iter().rev().find_map(|event| match event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } => Some(*pos),
            _ => None,
        });
        (press, input.pointer.interact_pos())
    });
    // Raw press positions can arrive before a widget gets a click response.
    // Keep them on the grid's input layer so modal backdrops still block edits.
    let layer = ui.layer_id();
    let allows_pointer = app.drum_templates.is_none()
        && app.variations.is_none()
        && app.snapshot_dialog.is_none()
        && app.sequence_tool.is_none()
        && ui.memory(|memory| memory.allows_interaction(layer));
    let on_grid_layer =
        |&position: &Pos2| allows_pointer && ui.ctx().layer_id_at(position) == Some(layer);
    let press_at = press_at.filter(on_grid_layer);
    let pointer_at = pointer_at.filter(on_grid_layer);
    let mut selection_start = None;
    let mut selection_end = None;
    let mut edits = Vec::new();
    let mut recorded = [[0; STEPS]; 2];
    for event in &app.studio.performance {
        if event.tick / TICKS_PER_BAR == bar as u32 && app.studio.performance_is_playable(event) {
            let row = usize::from(matches!(event.input, ChopInput::Key(_)));
            let step = (event.tick % TICKS_PER_BAR / TICKS_PER_STEP) as usize;
            recorded[row][step] += 1;
        }
    }
    ScrollArea::horizontal()
        .id_salt("step_grid_lanes")
        .drag_to_scroll(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(gap, gap);
            ui.horizontal(|ui| {
                ui.allocate_space(vec2(name_width, 14.0));
                for step in 0..STEPS {
                    let text = if step % 4 == 0 {
                        format!("{}", step / 4 + 1)
                    } else {
                        "·".into()
                    };
                    ui.add_sized(
                        [step_width, 14.0],
                        Label::new(label(text, 10.0, FAINT).monospace()),
                    );
                }
            });
            for row in 0..LANES {
                let name = lane_name(row);
                ui.horizontal(|ui| {
                    if row < 2 {
                        ui.add_sized(
                            [name_width, 24.0],
                            Label::new(
                                label(name, 12.0, if row == 0 { BLUE } else { MINT }).strong(),
                            ),
                        );
                    } else if ui
                        .add_sized(
                            [name_width, 24.0],
                            Button::new(label(name, 12.0, TEXT)).frame(false),
                        )
                        .clicked()
                    {
                        app.send(Command::Drum(row - 2));
                    }
                    for step in 0..STEPS {
                        let count = recorded.get(row).map_or(0, |counts| counts[step]);
                        let (fill, text) = if row == 0 {
                            let pad = app.studio.chops[bar][step];
                            (
                                pad.map(|_| Color32::from_rgb(34, 84, 115)),
                                pad.map_or(String::new(), |pad| (pad + 1).to_string()),
                            )
                        } else if row == 1 {
                            let note = app.studio.key_steps[bar][step];
                            (
                                note.map(|_| Color32::from_rgb(26, 87, 68)),
                                note.map_or_else(String::new, piano_note),
                            )
                        } else {
                            let velocity = drum_velocity(app.studio.hits[bar][row - 2][step]);
                            let fill = (velocity > 0).then(|| {
                                let intensity = f32::from(velocity) / 127.0;
                                Color32::from_rgb(
                                    (55.0 + intensity * 130.0) as u8,
                                    (36.0 + intensity * 117.0) as u8,
                                    (84.0 + intensity * 168.0) as u8,
                                )
                            });
                            (fill, String::new())
                        };
                        let active = cursor == Some(bar * STEPS + step);
                        let empty = if step / 4 % 2 == 0 {
                            INSET
                        } else {
                            Color32::from_rgb(20, 18, 17)
                        };
                        let sense = if selecting {
                            Sense::click_and_drag()
                        } else {
                            Sense::click()
                        };
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(step_width, 25.0), sense);
                        let cell = Cell { lane: row, step };
                        if selecting {
                            let visible = rect.intersect(ui.clip_rect());
                            if press_at.is_some_and(|pos| visible.contains(pos)) {
                                selection_start = Some(cell);
                            }
                            if pointer_at.is_some_and(|pos| visible.contains(pos)) {
                                selection_end = Some(cell);
                            }
                        }
                        let marked = selecting
                            && selection
                                .region()
                                .is_some_and(|region| region.contains(cell));
                        let p = ui.painter_at(rect);
                        let color = fill.unwrap_or(if response.hovered() { EDGE } else { empty });
                        let stroke = if marked {
                            Stroke::new(2.0_f32, BLUE)
                        } else if response.has_focus() || active {
                            Stroke::new(1.2_f32, MINT)
                        } else if state.velocity_cell == Some(cell) {
                            Stroke::new(1.2_f32, GOLD)
                        } else {
                            Stroke::NONE
                        };
                        p.rect(rect, 4.0, color, stroke, StrokeKind::Inside);
                        if count > 0 {
                            p.circle_filled(rect.right_top() + vec2(-4.5, 4.5), 2.6, GOLD);
                        }
                        if !text.is_empty() {
                            p.text(
                                rect.center(),
                                Align2::CENTER_CENTER,
                                text,
                                FontId::monospace(11.0),
                                TEXT,
                            );
                        }
                        if app.studio.step_rules[bar].contains_key(&((row * STEPS + step) as u8)) {
                            p.circle_stroke(
                                rect.left_top() + vec2(5.0, 5.0),
                                2.5,
                                Stroke::new(1.0_f32, BLUE),
                            );
                        }
                        let mut description =
                            format!("{name} · bar {} · step {}", bar + 1, step + 1);
                        if count > 0 {
                            description.push_str(&format!(" · {count} recorded"));
                        }
                        if row >= 2 {
                            let velocity = drum_velocity(app.studio.hits[bar][row - 2][step]);
                            description.push_str(&format!(" · velocity {velocity}"));
                        } else if let Some(assignment) = match row {
                            0 => app.studio.chops[bar][step].map(|pad| format!("Pad {}", pad + 1)),
                            _ => app.studio.key_steps[bar][step].map(piano_note),
                        } {
                            description.push_str(&format!(" · {assignment}"));
                        } else {
                            description.push_str(" · Empty");
                        }
                        response.widget_info(|| {
                            WidgetInfo::selected(WidgetType::Button, true, marked, &description)
                        });
                        // A tooltip from the previous cell can cover the next cell during a stroke.
                        if !down {
                            response.clone().on_hover_text(description);
                        }
                        if selecting {
                            continue;
                        }
                        if response.clicked() && modifiers.alt {
                            state.rule_cell = cell;
                            app.sequence_tool = Some(Tool::Conditions);
                            continue;
                        }
                        if response.secondary_clicked() {
                            edits.push(Edit::Clear(cell));
                            if row >= 2 {
                                state.velocity_cell = Some(cell);
                            }
                        } else if row < 2 {
                            if response.clicked() {
                                edits.push(Edit::Toggle(cell));
                            }
                        } else {
                            let current = drum_velocity(app.studio.hits[bar][row - 2][step]);
                            let visible = rect.intersect(ui.clip_rect());
                            let inside = response.enabled()
                                && pointer_at.is_some_and(|pos| visible.contains(pos));
                            let began_here = response.enabled()
                                && press_at.is_some_and(|pos| visible.contains(pos));
                            if began_here {
                                state.velocity_cell = Some(cell);
                                if !modifiers.command && !modifiers.alt {
                                    let velocity = if modifiers.shift { 127 } else { 89 };
                                    state.gesture =
                                        Some(if current == velocity { 0 } else { velocity });
                                }
                            }
                            if (began_here || (down && inside))
                                && let Some(velocity) = state.gesture
                                && velocity != current
                            {
                                edits.push(Edit::DrumVelocity(cell, velocity));
                            }
                            if response.clicked() {
                                state.velocity_cell = Some(cell);
                                if modifiers.command {
                                    app.sequence_tool = Some(Tool::Velocity);
                                } else if !response.clicked_by(egui::PointerButton::Primary) {
                                    edits.push(Edit::Toggle(cell));
                                }
                            }
                        }
                    }
                });
            }
        });
    if selecting {
        let previous = selection.region();
        selection.pointer(selection_start, selection_end, down);
        if selection.region() != previous {
            ui.ctx().request_repaint();
        }
    }
    app.edit_sequence(edits, down);
}

pub(super) fn lane_timing(app: &mut App, ui: &mut Ui) {
    let id = ui.id().with("timing_lane");
    let mut lane = ui.data(|data| data.get_temp::<usize>(id)).unwrap_or(2);
    ui.add_enabled_ui(!app.playing, |ui| {
        ui.horizontal_wrapped(|ui| {
            ComboBox::from_id_salt("timing_lane")
                .selected_text(lane_name(lane))
                .show_ui(ui, |ui| {
                    for row in 0..LANES {
                        let offset = app.studio.lane_offsets[row];
                        ui.selectable_value(
                            &mut lane,
                            row,
                            format!("{} · {offset:+} ticks", lane_name(row)),
                        );
                    }
                });
            let old = app.studio.lane_offsets[lane];
            let mut offset = old;
            let limit = crate::model::MAX_LANE_OFFSET;
            let timing = ui.add(
                DragValue::new(&mut offset)
                    .range(-limit..=limit)
                    .prefix("Timing ")
                    .suffix(" ticks"),
            );
            named_number(timing, "Lane timing offset in ticks", f64::from(offset));
            let milliseconds = f64::from(offset) * 60_000.0 / (f64::from(app.studio.bpm) * 480.0);
            small(ui, format!("{milliseconds:+.1} ms"));
            if action(ui, "Reset lane", offset != 0).clicked() {
                offset = 0;
            }
            if action(
                ui,
                "Reset all lanes",
                app.studio.lane_offsets.iter().any(|&offset| offset != 0),
            )
            .clicked()
            {
                app.edit_sequence([Edit::ResetLaneTiming], false);
            } else if offset != old {
                app.edit_sequence([Edit::LaneTiming(lane, offset)], pointer_down(ui));
            }
        });
    });
    ui.data_mut(|data| data.insert_temp(id, lane));
    small(
        ui,
        "Negative values play early. Timing applies to programmed steps within the bar, including repeats.",
    );
    if app.playing {
        small(ui, "Stop playback to edit lane timing.");
    }
}

impl State {
    pub fn conditions(&mut self, app: &mut App, ui: &mut Ui) {
        let bar = app.bar;
        let cell = &mut self.rule_cell;
        let mut edits = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ComboBox::from_id_salt("condition_lane")
                .selected_text(lane_name(cell.lane))
                .show_ui(ui, |ui| {
                    for row in 0..LANES {
                        ui.selectable_value(&mut cell.lane, row, lane_name(row));
                    }
                });
            let mut step = cell.step + 1;
            ui.add(DragValue::new(&mut step).range(1..=STEPS).prefix("Step "));
            cell.step = step - 1;
            let key = (cell.lane * STEPS + cell.step) as u8;
            let old = app.studio.step_rules[bar]
                .get(&key)
                .copied()
                .unwrap_or_default();
            let mut rule = old;
            ui.add(
                DragValue::new(&mut rule.chance)
                    .range(0..=100)
                    .prefix("Chance ")
                    .suffix("%"),
            );
            ui.add(
                DragValue::new(&mut rule.repeats)
                    .range(1..=4)
                    .prefix("Hits "),
            );
            ui.add(
                DragValue::new(&mut rule.every)
                    .range(1..=16)
                    .prefix("Every ")
                    .suffix(" passes"),
            );
            ComboBox::from_id_salt("condition_fill")
                .selected_text(rule.fill.label())
                .show_ui(ui, |ui| {
                    for condition in crate::model::FillCondition::ALL {
                        ui.selectable_value(&mut rule.fill, condition, condition.label());
                    }
                });
            if action(ui, "Reset condition", old != Default::default()).clicked() {
                rule = Default::default();
            }
            if rule != old {
                edits.push(Edit::Condition(*cell, rule));
            }
            if app.studio.step_has_hit(bar, cell.lane, cell.step) {
                let old = app.studio.step_feel[bar]
                    .get(&key)
                    .copied()
                    .unwrap_or_default();
                let mut feel = old;
                let max = crate::model::groove::MAX_OFFSET;
                ui.add(
                    DragValue::new(&mut feel.offset)
                        .range(-max..=max)
                        .prefix("Hit timing ")
                        .suffix(" ticks"),
                );
                if cell.lane < 2 {
                    let base = if cell.lane == 0 { 127 } else { 112 };
                    let mut velocity = feel.velocity.unwrap_or(base);
                    if ui
                        .add(
                            DragValue::new(&mut velocity)
                                .range(1..=127)
                                .prefix("Hit velocity "),
                        )
                        .changed()
                    {
                        feel.velocity = (velocity != base).then_some(velocity);
                    }
                }
                if action(ui, "Reset hit feel", old != Default::default()).clicked() {
                    feel = Default::default();
                }
                if feel != old {
                    edits.push(Edit::Feel(*cell, feel));
                }
                let mut sound = app.studio.step_sounds[bar]
                    .get(&key)
                    .copied()
                    .unwrap_or_default();
                if layers::hit_sound(ui, &mut sound) {
                    edits.push(Edit::Sound(*cell, sound));
                }
            }
        });
        app.edit_sequence(edits, pointer_down(ui));
        small(
            ui,
            "Alt-click a step on the grid to open its conditions. Exports render with Fill off.",
        );
    }
}

pub(super) fn velocity(app: &mut App, ui: &mut Ui, cell: Cell) {
    ui.horizontal_wrapped(|ui| {
        small(
            ui,
            format!(
                "{} · Bar {} · Step {}",
                lane_name(cell.lane),
                app.bar + 1,
                cell.step + 1
            ),
        );
        let mut velocity = drum_velocity(app.studio.hits[app.bar][cell.lane - 2][cell.step]);
        if ui
            .add(
                DragValue::new(&mut velocity)
                    .range(0..=127)
                    .prefix("Velocity "),
            )
            .changed()
        {
            app.edit_sequence([Edit::DrumVelocity(cell, velocity)], pointer_down(ui));
        }
    });
}

fn pointer_down(ui: &Ui) -> bool {
    ui.input(|input| input.pointer.primary_down())
}
