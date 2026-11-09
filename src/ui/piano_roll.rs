use super::{BLUE, DIM, EDGE, GOLD, INSET, MINT, TEXT, action, label, pill, primary, small};
use crate::{
    app::App,
    audio::PadMode,
    model::{
        ChopInput, MAX_BARS, PIANO_HIGH, PIANO_LOW, PlayedChop, Quantize, STEPS, Studio,
        TICKS_PER_BAR, piano_note, swung_step,
    },
};
use eframe::egui::{
    self, Align2, Color32, ComboBox, Context, DragValue, FontId, Key, Pos2, Rect, ScrollArea,
    Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use std::collections::{BTreeSet, HashMap, HashSet};

const LANE_HEIGHT: f32 = 19.0;
const LABEL_WIDTH: f32 = 64.0;
const CLIPBOARD_HEADER: &str = "Lost Samples piano roll v1\n";
type NoteId = (u32, ChopInput);

fn note_id(note: &PlayedChop) -> NoteId {
    (note.tick, note.input)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Track {
    Pads,
    Keys,
}

impl Track {
    fn of(input: ChopInput) -> Self {
        match input {
            ChopInput::Pad(_) => Self::Pads,
            ChopInput::Key(_) => Self::Keys,
        }
    }

    fn rows(self) -> usize {
        match self {
            Self::Pads => STEPS,
            Self::Keys => usize::from(PIANO_HIGH - PIANO_LOW + 1),
        }
    }

    fn input(self, row: usize) -> ChopInput {
        match self {
            Self::Pads => ChopInput::Pad(row as u8),
            Self::Keys => ChopInput::Key(PIANO_HIGH - row as u8),
        }
    }

    fn row(input: ChopInput) -> usize {
        match input {
            ChopInput::Pad(pad) => usize::from(pad),
            ChopInput::Key(note) => usize::from(PIANO_HIGH - note),
        }
    }
}

fn input_name(input: ChopInput) -> String {
    match input {
        ChopInput::Pad(pad) => format!("Pad {:02}", pad + 1),
        ChopInput::Key(note) => piano_note(note),
    }
}

fn assigned(studio: &Studio, input: ChopInput, duration: f64) -> bool {
    match input {
        ChopInput::Pad(pad) => studio.slice(usize::from(pad), duration).is_some(),
        ChopInput::Key(note) => studio.key_range(note, duration).is_some(),
    }
}

#[derive(Clone, Copy)]
enum Gesture {
    Select {
        start: Pos2,
    },
    Move {
        start: Pos2,
        note: NoteId,
        additive: bool,
    },
    Resize {
        start: Pos2,
        note: NoteId,
    },
    Draw,
}

struct View {
    rect: Rect,
    start: u32,
    ticks_visible: u32,
    lanes: Vec<ChopInput>,
    lane_height: f32,
}

impl View {
    fn grid(&self) -> Rect {
        self.rect.with_min_x(self.rect.left() + LABEL_WIDTH)
    }

    fn x(&self, tick: u32) -> f32 {
        self.grid().left() + (tick - self.start) as f32 * self.grid().width() / self.ticks_visible as f32
    }

    fn ticks(&self, pixels: f32) -> i64 {
        (pixels * self.ticks_visible as f32 / self.grid().width()).round() as i64
    }

    fn hit(&self, pos: Pos2, grid: Quantize, swing: f32) -> Option<NoteId> {
        if !self.grid().contains(pos) {
            return None;
        }
        let row = ((pos.y - self.rect.top()) / self.lane_height) as usize;
        let input = *self.lanes.get(row)?;
        let tick = self
            .ticks(pos.x - self.grid().left())
            .clamp(0, i64::from(self.ticks_visible - 1)) as u64;
        let step = (0..u64::from(self.ticks_visible / (TICKS_PER_BAR / STEPS as u32)))
            .rev()
            .find(|&step| swung_step(step, swing) <= tick)
            .unwrap_or(0);
        let snapped = match grid {
            Quantize::Off => tick,
            Quantize::Eighth => swung_step(step & !1, swing),
            Quantize::Sixteenth => swung_step(step, swing),
            Quantize::ThirtySecond => {
                let start = swung_step(step, swing);
                let middle = (start + swung_step(step + 1, swing)) / 2;
                if tick < middle { start } else { middle }
            }
            Quantize::EighthTriplet | Quantize::SixteenthTriplet => {
                let unit = u64::from(grid.triplet_unit()?);
                tick / unit * unit
            }
        };
        Some((self.start + snapped as u32, input))
    }

    fn note_rect(&self, note: &PlayedChop) -> Option<Rect> {
        if !(self.start..self.start + self.ticks_visible).contains(&note.tick) {
            return None;
        }
        let x = self.x(note.tick);
        let end = self.x((note.tick + note.length.unwrap_or(34)).min(self.start + self.ticks_visible));
        let row = self.lanes.iter().position(|input| *input == note.input)?;
        let y = self.rect.top() + row as f32 * self.lane_height;
        let left = x.min(self.rect.right() - 5.0);
        let right = end.max(left + 5.0).min(self.rect.right());
        Some(Rect::from_min_max(
            pos2(left, y + 2.0),
            pos2(right, y + self.lane_height - 2.0),
        ))
    }
}

pub(super) fn transcription_canvas(
    ui: &mut Ui, notes: &[PlayedChop], bar: u32, bars: u32, selected: &mut Option<usize>,
) {
    let start = bar * TICKS_PER_BAR;
    let span = (bars - bar).min(4) * TICKS_PER_BAR;
    let mut pitches: Vec<_> = notes.iter().filter(|note| (start..start + span).contains(&note.tick))
        .map(|note| note.input).collect();
    pitches.sort_unstable_by(|a, b| b.cmp(a));
    pitches.dedup();
    if pitches.is_empty() {
        small(ui, "This part of the piano roll is empty.");
        return;
    }
    ScrollArea::vertical().id_salt("transcription_roll").max_height(215.0).show(ui, |ui| {
        let lane_height = 18.0;
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), pitches.len() as f32 * lane_height), Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Detected note piano roll"));
        let view = View { rect, start, lanes: pitches, lane_height, ticks_visible: span };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, INSET);
        for (index, &input) in view.lanes.iter().enumerate() {
            let y = rect.top() + index as f32 * lane_height;
            painter.text(pos2(rect.left() + 6.0, y + lane_height / 2.0), Align2::LEFT_CENTER,
                input_name(input), FontId::monospace(11.0), DIM);
            painter.hline(rect.x_range(), y + lane_height, Stroke::new(0.6_f32, EDGE));
        }
        for tick in (start..start + span).step_by(TICKS_PER_BAR as usize / 4) {
            painter.vline(view.x(tick), rect.y_range(), Stroke::new(if tick.is_multiple_of(TICKS_PER_BAR) { 1.5_f32 } else { 0.6_f32 }, EDGE));
        }
        let pointer = response.clicked().then(|| response.interact_pointer_pos()).flatten();
        for (index, note) in notes.iter().enumerate() {
            let Some(hit) = view.note_rect(note) else { continue };
            painter.rect_filled(hit, 2.0, if *selected == Some(index) { GOLD } else { MINT });
            if let Some(expression) = &note.expression {
                let points: Vec<_> = expression.bends.iter()
                    .take_while(|point| point.tick <= note.length.unwrap_or(1) && note.tick + point.tick < start + span)
                    .map(|point| pos2(view.x(note.tick + point.tick), hit.center().y - f32::from(point.cents).clamp(-200.0, 200.0) / 48.0))
                    .collect();
                if points.len() > 1 { painter.add(egui::Shape::line(points, Stroke::new(1.0_f32, INSET))); }
            }
            if pointer.is_some_and(|point| hit.contains(point)) { *selected = Some(index); }
        }
    });
}

pub(crate) struct PianoRoll {
    track: Track,
    bar: usize,
    grid: Quantize,
    strength: u8,
    bars_visible: usize,
    assigned_only: bool,
    lane_height: f32,
    velocity_lane: bool,
    human_ticks: u8,
    human_velocity: u8,
    draw: bool,
    snap_draw: bool,
    draw_gate: bool,
    selection: BTreeSet<NoteId>,
    gesture: Option<Gesture>,
    focus_row: Option<usize>,
    velocity: u8,
    hold_ticks: u32,
    message: Option<String>,
    transcription_sound: crate::model::melody::Instrument,
}

impl PianoRoll {
    pub fn new(app: &App) -> Self {
        let mut roll = Self {
            track: Track::Pads,
            bar: app.bar,
            grid: Quantize::Sixteenth,
            strength: 100,
            bars_visible: 1,
            assigned_only: false,
            lane_height: LANE_HEIGHT,
            velocity_lane: false,
            human_ticks: 8,
            human_velocity: 6,
            draw: !app
                .studio
                .roll_notes()
                .iter()
                .any(|note| note.tick < app.studio.pattern_ticks()),
            snap_draw: true,
            draw_gate: false,
            selection: BTreeSet::new(),
            gesture: None,
            focus_row: None,
            velocity: 100,
            hold_ticks: 34,
            message: None,
            transcription_sound: Default::default(),
        };
        let preferred = if app.library.midi.piano_view {
            Track::Keys
        } else {
            Track::Pads
        };
        let available = |track| {
            app.studio.roll_notes().iter().any(|note| {
                note.tick < app.studio.pattern_ticks() && Track::of(note.input) == track
            })
        };
        let track = [preferred, Track::Pads, Track::Keys]
            .into_iter()
            .find(|&track| available(track))
            .unwrap_or(preferred);
        roll.show_track(&app.studio, track);
        roll
    }

    pub(crate) fn reset_selection(&mut self) {
        self.selection.clear();
        self.gesture = None;
        self.message = None;
    }

    fn show_track(&mut self, studio: &Studio, track: Track) {
        self.track = track;
        self.selection.clear();
        self.gesture = None;
        self.message = None;
        self.focus_lane(studio, true);
    }

    fn focus_lane(&mut self, studio: &Studio, find_bar: bool) {
        let notes = studio.roll_notes();
        let hits = || {
            notes.iter().filter(|note| {
                note.tick < studio.pattern_ticks() && Track::of(note.input) == self.track
            })
        };
        let first = hits()
            .find(|note| note.tick / TICKS_PER_BAR == self.bar as u32)
            .or_else(|| find_bar.then(|| hits().next()).flatten());
        if let Some(note) = first {
            self.bar = (note.tick / TICKS_PER_BAR) as usize;
            self.focus_row = Some(Track::row(note.input));
        } else {
            self.focus_row = (0..self.track.rows())
                .find(|&row| assigned(studio, self.track.input(row), f64::INFINITY))
                .or(Some(if self.track == Track::Pads {
                    0
                } else {
                    Track::row(ChopInput::Key(60))
                }));
        }
    }

    fn apply(
        &mut self,
        notes: &mut Vec<PlayedChop>,
        mut edit: impl FnMut(PlayedChop) -> Option<PlayedChop>,
    ) {
        if self.selection.is_empty() {
            return;
        }
        let mut after = Vec::with_capacity(notes.len());
        let mut selected = BTreeSet::new();
        let mut ids = HashSet::with_capacity(notes.len());
        self.message = None;
        for note in notes.iter() {
            let is_selected = self.selection.contains(&note_id(note));
            let edited = if is_selected { edit(note.clone()) } else { Some(note.clone()) };
            if let Some(edited) = edited {
                let id = note_id(&edited);
                if !ids.insert(id) {
                    self.message = Some(
                        "Two hits would share the same lane and start. Move one before applying the edit.".into(),
                    );
                    return;
                }
                if is_selected {
                    selected.insert(id);
                }
                after.push(edited);
            }
        }
        self.commit(notes, after, selected);
    }

    fn commit(
        &mut self,
        notes: &mut Vec<PlayedChop>,
        mut after: Vec<PlayedChop>,
        selected: BTreeSet<NoteId>,
    ) {
        after.sort_unstable_by_key(note_id);
        if *notes != after {
            *notes = after;
            self.selection = selected;
        }
    }

    fn insert(
        &mut self,
        notes: &mut Vec<PlayedChop>,
        additions: impl IntoIterator<Item = PlayedChop>,
    ) -> (usize, usize) {
        let mut occupied: HashSet<_> = notes.iter().map(note_id).collect();
        let mut after = notes.clone();
        let mut selection = BTreeSet::new();
        let mut skipped = 0;
        for note in additions {
            let id = note_id(&note);
            if occupied.insert(id) {
                after.push(note);
                selection.insert(id);
            } else {
                skipped += 1;
            }
        }
        let added = selection.len();
        self.commit(notes, after, selection);
        (added, skipped)
    }

    fn copy_selected(&mut self, app: &mut App, ctx: &Context) {
        let mut copy: Vec<_> = app
            .studio
            .roll_notes()
            .into_iter()
            .filter(|note| self.selection.contains(&note_id(note)))
            .collect();
        let Some(first) = copy.first() else { return };
        let origin = first.tick / TICKS_PER_BAR * TICKS_PER_BAR;
        for note in &mut copy {
            note.tick -= origin;
        }
        if let Ok(encoded) = serde_json::to_string(&copy) {
            ctx.copy_text(format!("{CLIPBOARD_HEADER}{encoded}"));
        }
        self.message = Some(format!(
            "Copied {} {}. Choose a destination bar and paste.",
            copy.len(),
            if copy.len() == 1 { "hit" } else { "hits" }
        ));
        app.piano_roll_clipboard = copy;
    }

    fn paste(&mut self, app: &mut App) {
        let Some(first) = app.piano_roll_clipboard.first() else {
            return;
        };
        let start = self.bar as u32 * TICKS_PER_BAR;
        if app
            .piano_roll_clipboard
            .iter()
            .any(|note| start + note.tick >= app.studio.pattern_ticks())
        {
            self.message = Some("Add more bars before pasting this selection.".into());
            return;
        }
        let track = Track::of(first.input);
        let additions = app.piano_roll_clipboard.iter().cloned().map(|note| PlayedChop {
            tick: start + note.tick,
            id: Default::default(),
            ..note
        });
        let (added, skipped) = app.studio.edit_roll_notes(|notes| self.insert(notes, additions));
        if added > 0 {
            self.track = track;
            self.focus_row = self.selection.first().map(|&(_, input)| Track::row(input));
            self.refresh_fields(&app.studio);
        }
        self.message = Some(if skipped == 0 {
            format!(
                "Pasted {added} {} into bar {}.",
                if added == 1 { "hit" } else { "hits" },
                self.bar + 1
            )
        } else {
            format!("Pasted {added}. Skipped {skipped} occupied positions.")
        });
    }

    fn draw_note(&self, app: &App, view: &View, pos: Pos2) -> Option<PlayedChop> {
        let grid = if self.snap_draw {
            self.grid
        } else {
            Quantize::Off
        };
        let (tick, input) = view.hit(pos, grid, app.studio.swing)?;
        Some(PlayedChop {
            id: Default::default(),
            tick,
            input,
            velocity: self.velocity,
            mode: if self.draw_gate {
                PadMode::Gate
            } else {
                PadMode::Slice
            },
            length: self
                .draw_gate
                .then_some(self.hold_ticks.min(app.studio.pattern_ticks())),
            sound: Default::default(),
            expression: None,
        })
    }

    fn quantize(&mut self, studio: &mut Studio) {
        let loop_ticks = u64::from(studio.pattern_ticks());
        let swing = studio.swing;
        let grid = self.grid;
        let strength = f64::from(self.strength) / 100.0;
        let occupied: HashSet<_> = studio.roll_notes().iter().map(note_id).collect();
        let proposals: HashMap<_, _> = self
            .selection
            .iter()
            .filter_map(|&(start, input)| {
                let snapped = grid.snap(f64::from(start), swing) as f64;
                let tick = (f64::from(start) + (snapped - f64::from(start)) * strength).round() as u64;
                let tick = (tick % loop_ticks) as u32;
                (tick != start).then_some(((start, input), (tick, input)))
            })
            .collect();
        let mut count = HashMap::new();
        for &destination in proposals.values() {
            *count.entry(destination).or_insert(0) += 1;
        }
        let mut collision = false;
        studio.edit_roll_notes(|notes| self.apply(notes, |mut note| {
            if let Some(&target) = proposals.get(&note_id(&note)) {
                if occupied.contains(&target) || count[&target] > 1 {
                    collision = true;
                } else {
                    note.tick = target.0;
                }
            }
            Some(note)
         }));
        if collision {
            self.message = Some(
                "Hits that would land together on the same lane kept their original timing.".into(),
            );
        }
    }


    fn select_all(&mut self, studio: &Studio) {
        self.selection = studio
            .roll_notes()
            .iter()
            .filter(|note| {
                note.tick < studio.pattern_ticks() && Track::of(note.input) == self.track
            })
            .map(note_id)
            .collect();
        self.refresh_fields(studio);
    }

    fn refresh_fields(&mut self, studio: &Studio) {
        if let Some(note) = studio
            .roll_notes()
            .iter()
            .find(|note| self.selection.contains(&note_id(note)))
        {
            self.velocity = note.velocity;
            self.hold_ticks = note.length.unwrap_or(34);
        }
    }

    pub fn keys(app: &App) -> Self {
        let mut roll = Self::new(app);
        roll.show_track(&app.studio, Track::Keys);
        roll.transcription_sound = app.studio.performance.iter().find_map(|note| note.expression.as_ref().map(|expression| expression.instrument)).unwrap_or_default();
        roll
    }

    pub fn show(&mut self, app: &mut App, ctx: &Context) -> bool {
        self.bar = self.bar.min(app.studio.bars - 1);
        let mut close = false;
        let screen = ctx.screen_rect();
        let width = (screen.width() - 48.0).min(1090.0);
        let height = (screen.height() - 270.0).clamp(105.0, 495.0);
        let modal = super::responsive_modal(ctx, "piano_roll_modal", width, |ui| {
            ui.horizontal(|ui| {
                ui.label(label("Piano roll", 22.0, TEXT).strong());
                if action(ui, if app.playing { "Stop" } else { "Play" }, app.output.is_some()).clicked() {
                    app.toggle_pattern();
                }
                if action(ui, "Close", true).clicked() {
                    close = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (track, name) in [(Track::Pads, "Pads"), (Track::Keys, "Keys")] {
                    if pill(ui, name, self.track == track).clicked() && self.track != track {
                        self.show_track(&app.studio, track);
                    }
                }
                let prior = self.bar;
                ComboBox::from_id_salt("roll_bar").selected_text(format!("Bar {}", self.bar + 1)).show_ui(ui, |ui| {
                    for bar in 0..app.studio.bars {
                        ui.selectable_value(&mut self.bar, bar, format!("Bar {}", bar + 1));
                    }
                });
                ComboBox::from_id_salt("roll_view_span")
                    .selected_text(format!("Show {} {}", self.bars_visible, if self.bars_visible == 1 { "bar" } else { "bars" }))
                    .show_ui(ui, |ui| {
                        for count in [1, 2, 4, 8] {
                            ui.selectable_value(&mut self.bars_visible, count, format!("{count} bars"));
                        }
                    });
                if self.bar != prior {
                    self.focus_lane(&app.studio, false);
                }
                if action(ui, "Add bar", app.studio.bars < MAX_BARS).clicked() {
                    app.studio.insert_bar(app.studio.bars);
                    self.bar = app.studio.bars - 1;
                    self.focus_lane(&app.studio, false);
                }
                if pill(ui, "Draw", self.draw).on_hover_text("Click an empty spot to add a hit. Turn Draw off to box-select.").clicked() {
                    self.draw = !self.draw;
                    self.message = None;
                }
                if action(ui, "Copy", !self.selection.is_empty()).on_hover_text("Copy the selected hits (Ctrl+C)").clicked() {
                    self.copy_selected(app, ctx);
                }
                if action(ui, "Paste", !app.piano_roll_clipboard.is_empty()).on_hover_text("Paste at the same offsets in the current bar (Ctrl+V)").clicked() {
                    self.paste(app);
                }
                if action(ui, "Select all", true).on_hover_text("Select this lane type across all bars").clicked() {
                    self.select_all(&app.studio);
                }
                if action(ui, "Delete", !self.selection.is_empty()).clicked() {
                    app.studio.edit_roll_notes(|notes| self.apply(notes, |_| None));
                }
                super::project_history(app, ui);
            });
            ui.horizontal_wrapped(|ui| {
                small(ui, "Quantize grid");
                ComboBox::from_id_salt("roll_quantize").selected_text(self.grid.label()).show_ui(ui, |ui| {
                    for grid in Quantize::ALL.into_iter().skip(1) {
                        ui.selectable_value(&mut self.grid, grid, grid.label());
                    }
                });
                ui.add(DragValue::new(&mut self.strength).range(0..=100).prefix("Strength ").suffix("%"));
                if primary(ui, "Quantize selected", !self.selection.is_empty()).clicked() {
                    self.quantize(&mut app.studio);
                }
                ui.add_enabled_ui(self.draw || !self.selection.is_empty(), |ui| {
                    ui.add(DragValue::new(&mut self.velocity).range(1..=127).prefix("Velocity "));
                    if !self.draw && action(ui, "Apply velocity", true).clicked() {
                        let velocity = self.velocity;
                        app.studio.edit_roll_notes(|notes| self.apply(notes, |mut note| { note.velocity = velocity; Some(note) }));
                    }
                });
                if self.draw {
                    ui.checkbox(&mut self.snap_draw, "Snap draw");
                    ui.checkbox(&mut self.draw_gate, "Gate");
                    if self.draw_gate {
                        ui.add(DragValue::new(&mut self.hold_ticks).range(1..=app.studio.pattern_ticks()).prefix("Hold ").suffix(" ticks"));
                    }
                } else if self.selection.len() == 1
                    && app.studio.roll_notes().iter().any(|note| self.selection.contains(&note_id(note)) && note.mode == PadMode::Gate)
                {
                    ui.add(DragValue::new(&mut self.hold_ticks).range(1..=app.studio.pattern_ticks()).prefix("Hold ").suffix(" ticks"));
                    if action(ui, "Apply hold", true).clicked() {
                        let length = self.hold_ticks;
                        app.studio.edit_roll_notes(|notes| self.apply(notes, |mut note| { note.length = Some(length); Some(note) }));
                    }
                }
                small(ui, format!("{} selected", self.selection.len()));
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.assigned_only, "Assigned lanes");
                ui.add(DragValue::new(&mut self.lane_height).range(14.0..=32.0).prefix("Lane height "));
                ui.checkbox(&mut self.velocity_lane, "Velocity lane");
                ui.add_enabled_ui(!self.selection.is_empty(), |ui| {
                    ui.menu_button("Humanize…", |ui| {
                        ui.add(DragValue::new(&mut self.human_ticks).range(0..=32).prefix("Timing ± ").suffix(" ticks"));
                        ui.add(DragValue::new(&mut self.human_velocity).range(0..=30).prefix("Velocity ± "));
                        if ui.button("Apply to selected notes").clicked() {
                            self.humanize(&mut app.studio);
                            ui.close_menu();
                        }
                    });
                });
                if let Some(&(tick, input)) = self.selection.first().filter(|_| self.selection.len() == 1) {
                    let mut at = tick;
                    if ui.add(DragValue::new(&mut at).range(0..=app.studio.pattern_ticks() - 1).prefix("Start tick ")).changed() {
                        app.studio.edit_roll_notes(|notes| self.apply(notes, |mut note| {
                            if note_id(&note) == (tick, input) { note.tick = at; }
                            Some(note)
                        }));
                    }
                    if let Some(note) = app.studio.roll_notes().into_iter()
                        .find(|note| note_id(note) == (tick, input))
                    {
                        let mut sound = note.sound;
                        if super::layers::hit_sound(ui, &mut sound) {
                            app.studio.set_roll_sound(tick, input, sound);
                        }
                    }
                }
            });
            ui.collapsing("Select notes by name and time", |ui| {
                let start = self.bar as u32 * TICKS_PER_BAR;
                let end = start + self.bars_visible as u32 * TICKS_PER_BAR;
                let notes: Vec<_> = app.studio.roll_notes().into_iter()
                    .filter(|note| (start..end).contains(&note.tick) && Track::of(note.input) == self.track)
                    .collect();
                if notes.is_empty() {
                    small(ui, "There are no notes in the visible bars.");
                }
                ScrollArea::vertical().id_salt("accessible_notes").max_height(120.0).show(ui, |ui| {
                    for note in notes {
                        let id = note_id(&note);
                        let mut selected = self.selection.contains(&id);
                        if ui.checkbox(&mut selected, format!("{} · bar {} · tick {} · velocity {}",
                            input_name(note.input), note.tick / TICKS_PER_BAR + 1,
                            note.tick % TICKS_PER_BAR, note.velocity)).changed()
                        {
                            if selected { self.selection.insert(id); }
                            else { self.selection.remove(&id); }
                        }
                    }
                });
            });
            if self.track == Track::Keys && app.studio.performance.iter().any(|note| note.expression.is_some()) {
                ui.collapsing("Transcribed phrase sound", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        super::transcription::sound_picker(app, ui, &mut self.transcription_sound);
                        if action(ui, "Apply to linked patterns", !app.playing && !app.recording).clicked() {
                            app.set_transcription_sound(self.transcription_sound);
                        }
                    });
                });
            }
            let (ruler, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            let mut scroll = ScrollArea::vertical().id_salt("roll_lanes").max_height(height)
                .auto_shrink([false, true]).drag_to_scroll(false);
            if let Some(row) = self.focus_row.take() {
                let input = self.track.input(row.min(self.track.rows() - 1));
                let row = self.displayed_lanes(&app.studio).iter().position(|lane| *lane == input).unwrap_or(0);
                scroll = scroll.vertical_scroll_offset((row as f32 * self.lane_height - height * 0.35).max(0.0));
            }
            let view = scroll.show(ui, |ui| self.canvas(app, ui)).inner;
            let beats = view.ticks_visible / (TICKS_PER_BAR / 4);
            for beat in 0..beats {
                if beat % 4 != 0 && ruler.width() / (beats as f32) < 34.0 { continue; }
                let tick = view.start + beat * (TICKS_PER_BAR / 4);
                ui.painter().text(pos2(view.x(tick) + 3.0, ruler.center().y), Align2::LEFT_CENTER,
                    format!("{}.{}", self.bar + 1 + (beat / 4) as usize, beat % 4 + 1), FontId::monospace(11.0), DIM);
            }
            if self.velocity_lane { self.draw_velocity_lane(app, ui, &view); }
            if let Some(message) = self.message.as_deref() {
                ui.label(label(message, 12.0, GOLD));
            } else if self.draw {
                small(ui, "Click an empty spot on an assigned lane. Snap draw uses the selected grid.");
            } else {
                small(ui, "Shift-click or box-select. Drag to move, arrow keys to nudge, or drag a Gate note’s edge to resize.");
            }
            self.shortcuts(app, ctx);
        });
        let close = close || modal.should_close();
        if close {
            app.bar = self.bar;
        }
        close
    }

    fn shortcuts(&mut self, app: &mut App, ctx: &Context) {
        let (focused, popup) = ctx.memory(|memory| (memory.focused(), memory.any_popup_open()));
        if popup || focused.is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some()) {
            return;
        }
        let (copy, paste) = ctx.input(|input| {
            let copy = input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Copy));
            let paste = input.events.iter().find_map(|event| match event {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            });
            (copy, paste)
        });
        if copy {
            self.copy_selected(app, ctx);
        }
        if let Some(text) = paste {
            if let Some(notes) = clipboard_notes(&text) {
                app.piano_roll_clipboard = notes;
                self.paste(app);
            } else {
                self.message = Some("Copy notes from the piano roll before pasting.".into());
            }
        }
        let duration = f64::INFINITY;
        let studio = &mut app.studio;
        ctx.input_mut(|input| {
            if input.consume_key(egui::Modifiers::COMMAND, Key::A) {
                self.select_all(studio);
            }
            if input.consume_key(egui::Modifiers::NONE, Key::Delete) {
                studio.edit_roll_notes(|notes| self.apply(notes, |_| None));
            }
            let modifiers = if input.modifiers.shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            let ticks = if modifiers.shift { 10 } else { 1 };
            if input.consume_key(modifiers, Key::ArrowLeft) {
                self.move_notes(studio, duration, -ticks, 0, None);
            } else if input.consume_key(modifiers, Key::ArrowRight) {
                self.move_notes(studio, duration, ticks, 0, None);
            }
        });
    }

    fn displayed_lanes(&self, studio: &Studio) -> Vec<ChopInput> {
        (0..self.track.rows()).map(|row| self.track.input(row))
            .filter(|&input| !self.assigned_only || studio.input_is_assigned(input)
                || studio.performance.iter().any(|note| note.input == input && note.expression.is_some())).collect()
    }

    fn humanize(&mut self, studio: &mut Studio) {
        let original = studio.roll_notes();
        let mut occupied: HashSet<_> = original.iter().map(note_id).collect();
        let timing = i32::from(self.human_ticks);
        let velocity = i32::from(self.human_velocity);
        let limit = studio.pattern_ticks() - 1;
        studio.edit_roll_notes(|notes| self.apply(notes, |mut note| {
            let lane = Track::row(note.input) as u32;
            let seed = note.tick.wrapping_mul(0x9e37_79b9).wrapping_add(lane.wrapping_mul(0x85eb_ca6b));
            let jitter = |seed: u32, range: i32| (seed % (2 * range + 1) as u32) as i32 - range;
            let tick = note.tick.saturating_add_signed(jitter(seed, timing)).min(limit);
            if !occupied.contains(&(tick, note.input)) {
                occupied.insert((tick, note.input));
                note.tick = tick;
            }
            note.velocity = (i32::from(note.velocity) + jitter(seed.rotate_left(13), velocity)).clamp(1, 127) as u8;
            Some(note)
        }));
    }

    fn draw_velocity_lane(&mut self, app: &mut App, ui: &mut Ui, view: &View) {
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, INSET);
        painter.text(rect.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, "Velocity", FontId::monospace(10.0), DIM);
        let notes: Vec<_> = app.studio.roll_notes().into_iter().filter(|note| {
            view.lanes.contains(&note.input) && (view.start..view.start + view.ticks_visible).contains(&note.tick)
        }).collect();
        for note in &notes {
            let x = view.x(note.tick);
            let top = rect.bottom() - f32::from(note.velocity) / 127.0 * (rect.height() - 4.0);
            let tint = if self.selection.contains(&note_id(note)) { GOLD } else { MINT };
            painter.vline(x, top..=rect.bottom(), Stroke::new(2.0_f32, tint));
        }
        if (response.clicked() || response.dragged()) && let Some(pos) = response.interact_pointer_pos() {
            let target = notes.iter().filter(|note| (view.x(note.tick) - pos.x).abs() < 11.0)
                .min_by_key(|note| (!self.selection.contains(&note_id(note)), ((view.x(note.tick) - pos.x).abs() * 10.0) as u32));
            if let Some(note) = target {
                self.selection = BTreeSet::from([note_id(note)]);
                let velocity = ((rect.bottom() - pos.y) / rect.height() * 127.0).round().clamp(1.0, 127.0) as u8;
                app.studio.edit_roll_notes(|notes| self.apply(notes, |mut note| { note.velocity = velocity; Some(note) }));
                self.velocity = velocity;
            }
        }
    }

    fn canvas(&mut self, app: &mut App, ui: &mut Ui) -> View {
        let lanes = self.displayed_lanes(&app.studio);
        if lanes.is_empty() { small(ui, "No assigned lanes. Assign a pad or key, or turn off Assigned lanes."); }
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), lanes.len().max(1) as f32 * self.lane_height),
            Sense::click_and_drag(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Piano roll note grid"));
        let view = View {
            rect,
            start: self.bar as u32 * TICKS_PER_BAR,
            lanes,
            lane_height: self.lane_height,
            ticks_visible: self.bars_visible.min(app.studio.bars - self.bar) as u32 * TICKS_PER_BAR,
        };
        let duration = f64::INFINITY;
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let notes: Vec<_> = app
            .studio
            .roll_notes()
            .into_iter()
            .filter(|note| Track::of(note.input) == self.track)
            .filter_map(|note| view.note_rect(&note).map(|rect| (note, rect)))
            .collect();
        let hovered =
            pointer.and_then(|pos| notes.iter().rev().find(|(_, rect)| rect.contains(pos)));
        let pressed = (response.is_pointer_button_down_on() || response.clicked())
            && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        if pressed && let Some(pos) = pointer.filter(|&pos| view.grid().contains(pos)) {
            let shift = ui.input(|input| input.modifiers.shift);
            self.message = None;
            if let Some((note, rect)) = hovered {
                let id = note_id(note);
                if shift && self.selection.remove(&id) {
                    self.gesture = None;
                } else {
                    if !shift && !self.selection.contains(&id) {
                        self.selection.clear();
                    }
                    self.selection.insert(id);
                    let within_bar =
                        note.tick + note.length.unwrap_or(34) <= view.start + view.ticks_visible;
                    let edge = rect.right() - (rect.width() / 3.0).min(5.0);
                    self.gesture = Some(
                        if note.mode == PadMode::Gate && within_bar && pos.x >= edge {
                            Gesture::Resize {
                                start: pos,
                                note: id,
                            }
                        } else {
                            Gesture::Move {
                                start: pos,
                                note: id,
                                additive: shift,
                            }
                        },
                    );
                }
            } else if self.draw {
                self.gesture = Some(Gesture::Draw);
            } else {
                if !shift {
                    self.selection.clear();
                }
                self.gesture = Some(Gesture::Select { start: pos });
            }
            if !matches!(self.gesture, Some(Gesture::Draw)) {
                self.refresh_fields(&app.studio);
            }
        }
        let released = ui.input(|input| !input.pointer.primary_down());
        if released
            && let Some(gesture) = self.gesture.take()
            && let Some(pos) = pointer
        {
            match gesture {
                Gesture::Select { start } => {
                    let area = Rect::from_two_pos(start, pos);
                    self.selection.extend(
                        notes
                            .iter()
                            .filter(|(_, rect)| area.intersects(*rect))
                            .map(|(note, _)| note_id(note)),
                    );
                    self.refresh_fields(&app.studio);
                }
                Gesture::Move { start, .. } if start.distance(pos) >= 3.0 => {
                    self.move_notes(
                        &mut app.studio,
                        duration,
                        view.ticks(pos.x - start.x),
                        ((pos.y - start.y) / self.lane_height).round() as i32,
                        Some(&view.lanes),
                    );
                }
                Gesture::Move {
                    note,
                    additive: false,
                    ..
                } => {
                    self.selection = BTreeSet::from([note]);
                    self.refresh_fields(&app.studio);
                }
                Gesture::Resize { start, note } if (start.x - pos.x).abs() >= 3.0 => {
                    let max = i64::from(app.studio.pattern_ticks());
                    let delta = view.ticks(pos.x - start.x);
                    app.studio.edit_roll_notes(|notes| self.apply(notes, |mut event| {
                        if note_id(&event) == note {
                            event.length = Some(
                                (i64::from(event.length.unwrap_or(34)) + delta).clamp(1, max)
                                    as u32,
                            );
                        }
                        Some(event)
                     }));
                    self.refresh_fields(&app.studio);
                }
                Gesture::Draw => {
                    if hovered.is_none()
                        && let Some(note) = self.draw_note(app, &view, pos)
                    {
                        if !assigned(&app.studio, note.input, duration) {
                            self.message =
                                Some("Assign a chop to this lane before drawing hits.".into());
                        } else if app.studio.edit_roll_notes(|notes| self.insert(notes, [note])).1 > 0 {
                            self.message = Some("A hit already starts here on this lane.".into());
                        }
                    }
                }
                _ => (),
            }
        }
        let painter = ui.painter_at(rect);
        for (row, &input) in view.lanes.iter().enumerate() {
            let y = rect.top() + row as f32 * self.lane_height;
            let lane = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), self.lane_height));
            if lane.intersects(ui.clip_rect()) {
                let dark = matches!(input, ChopInput::Key(note) if matches!(note % 12, 1 | 3 | 6 | 8 | 10));
                painter.rect_filled(
                    lane,
                    0.0,
                    if dark {
                        Color32::from_rgb(20, 18, 17)
                    } else {
                        INSET
                    },
                );
                painter.hline(lane.x_range(), lane.bottom(), Stroke::new(0.6_f32, EDGE));
                painter.text(
                    lane.left_center() + vec2(7.0, 0.0),
                    Align2::LEFT_CENTER,
                    input_name(input),
                    FontId::monospace(11.0),
                    if assigned(&app.studio, input, duration) {
                        TEXT
                    } else {
                        DIM
                    },
                );
            }
        }
        let visible_steps = (view.ticks_visible / (TICKS_PER_BAR / STEPS as u32)) as usize;
        if let Some(unit) = self.grid.triplet_unit() {
            for offset in (unit..view.ticks_visible).step_by(unit as usize) {
                painter.vline(view.x(view.start + offset), view.rect.y_range(), Stroke::new(0.5_f32, EDGE));
            }
        }
        for step in 0..=visible_steps {
            let tick = view.start + swung_step(step as u64, app.studio.swing) as u32;
            if self.grid == Quantize::ThirtySecond && step < visible_steps {
                let next = view.start + swung_step(step as u64 + 1, app.studio.swing) as u32;
                painter.vline(
                    view.x((tick + next) / 2),
                    rect.y_range(),
                    Stroke::new(0.5_f32, EDGE.gamma_multiply(0.6)),
                );
            }
            painter.vline(
                view.x(tick),
                rect.y_range(),
                Stroke::new(if step % 4 == 0 { 1.4_f32 } else { 0.6_f32 }, EDGE),
            );
        }
        if let Some(step) = app
            .step
            .filter(|step| app.song_playing.is_none() && step / STEPS == self.bar)
        {
            let tick = view.start + swung_step((step % STEPS) as u64, app.studio.swing) as u32;
            painter.vline(
                view.x(tick),
                rect.y_range(),
                Stroke::new(1.0_f32, MINT.gamma_multiply(0.65)),
            );
        }
        for &(ref note, hit) in &notes {
            let selected = self.selection.contains(&note_id(note));
            let fill = if selected {
                GOLD
            } else if self.track == Track::Pads {
                BLUE
            } else {
                MINT
            };
            painter.rect_filled(
                hit,
                2.0,
                fill.gamma_multiply(if note.expression.is_some() || assigned(&app.studio, note.input, duration) {
                    0.86
                } else {
                    0.42
                }),
            );
            if note.mode == PadMode::Gate
                && note.tick + note.length.unwrap_or(34) <= view.start + view.ticks_visible
            {
                painter.vline(
                    hit.right() - 2.0,
                    hit.y_range(),
                    Stroke::new(1.0_f32, INSET),
                );
            }
        }
        if let (Some(gesture), Some(pos)) = (self.gesture, pointer) {
            match gesture {
                Gesture::Select { start } => {
                    painter.rect_stroke(
                        Rect::from_two_pos(start, pos),
                        0.0,
                        Stroke::new(1.0_f32, GOLD),
                        StrokeKind::Inside,
                    );
                }
                Gesture::Move { start, .. } => {
                    for &(ref note, hit) in &notes {
                        if self.selection.contains(&note_id(note)) {
                            let delta = pos - start;
                            painter.rect_stroke(
                                hit.translate(vec2(
                                    delta.x,
                                    (delta.y / self.lane_height).round() * self.lane_height,
                                )),
                                2.0,
                                Stroke::new(1.0_f32, GOLD),
                                StrokeKind::Inside,
                            );
                        }
                    }
                }
                Gesture::Resize { start, note } => {
                    if let Some((_, hit)) = notes.iter().find(|(event, _)| note_id(event) == note)
                    {
                        painter.rect_stroke(
                            hit.with_max_x((hit.right() + pos.x - start.x).max(hit.left() + 5.0)),
                            2.0,
                            Stroke::new(1.0_f32, GOLD),
                            StrokeKind::Inside,
                        );
                    }
                }
                Gesture::Draw => (),
            }
        }
        if self.draw
            && hovered.is_none()
            && response.hovered()
            && let Some(note) = pointer.and_then(|pos| self.draw_note(app, &view, pos))
            && assigned(&app.studio, note.input, duration)
            && let Some(hit) = view.note_rect(&note)
        {
            painter.rect_stroke(
                hit,
                2.0,
                Stroke::new(1.0_f32, GOLD.gamma_multiply(0.65)),
                StrokeKind::Inside,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if let Some((note, _)) = hovered {
            response.on_hover_text(format!(
                "{} · Tick {} · Velocity {}{}",
                input_name(note.input),
                note.tick,
                note.velocity,
                note.length
                    .map_or_else(String::new, |length| format!(" · Hold {length} ticks"))
            ));
        }
        view
    }

    fn move_notes(&mut self, studio: &mut Studio, duration: f64, ticks: i64, rows: i32, lanes: Option<&[ChopInput]>) {
        let Some(&(first, _)) = self.selection.first() else {
            return;
        };
        let &(last, _) = self.selection.last().unwrap();
        let ticks = ticks.clamp(
            -i64::from(first),
            i64::from(studio.pattern_ticks() - 1) - i64::from(last),
        );
        let track = self.track;
        let destination = |input| {
            if rows == 0 { return Some(input); }
            if let Some(lanes) = lanes {
                let row = lanes.iter().position(|lane| *lane == input)? as i32 + rows;
                return usize::try_from(row).ok().and_then(|row| lanes.get(row)).copied();
            }
            let row = Track::row(input) as i32 + rows;
            (0..track.rows() as i32)
                .contains(&row)
                .then(|| track.input(row as usize))
        };
        if rows != 0
            && studio.roll_notes().iter().filter(|note| self.selection.contains(&note_id(note))).any(|note| {
                destination(note.input).is_none_or(|input| note.expression.is_none() && !assigned(studio, input, duration))
            })
        {
            self.message = Some("Move notes to lanes with assigned chops.".into());
            return;
        }
        studio.edit_roll_notes(|notes| self.apply(notes, |mut note| {
            note.tick = (i64::from(note.tick) + ticks) as u32;
            note.input = destination(note.input).unwrap_or(note.input);
            Some(note)
         }));
    }
}

fn clipboard_notes(text: &str) -> Option<Vec<PlayedChop>> {
    let contents = text.strip_prefix(CLIPBOARD_HEADER)?;
    if contents.len() > 1024 * 1024 {
        return None;
    }
    let notes: Vec<PlayedChop> = serde_json::from_str(contents).ok()?;
    let track = Track::of(notes.first()?.input);
    let limit = MAX_BARS as u32 * TICKS_PER_BAR;
    notes
        .iter()
        .all(|note| {
            let valid_input = match note.input {
                ChopInput::Pad(pad) => usize::from(pad) < STEPS,
                ChopInput::Key(key) => (PIANO_LOW..=PIANO_HIGH).contains(&key),
            };
            valid_input
                && Track::of(note.input) == track
                && note.tick < limit
                && (1..=127).contains(&note.velocity)
                && note
                    .length
                    .is_none_or(|length| (1..=limit).contains(&length))
        })
        .then_some(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Clip;
    use std::sync::Arc;

    fn pad(tick: u32, input: u8, velocity: u8) -> PlayedChop {
        PlayedChop {
            id: Default::default(),
            tick,
            input: ChopInput::Pad(input),
            velocity,
            mode: PadMode::Gate,
            length: Some(42),
            sound: Default::default(),
            expression: None,
        }
    }

    fn render_canvas(
        context: &Context,
        roll: &mut PianoRoll,
        app: &mut App,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 320.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        roll.canvas(app, ui);
                        roll.shortcuts(app, ctx);
                    });
            },
        )
    }

    fn press(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn box_selection_moves_and_quantizes_only_the_selected_notes() {
        let context = Context::default();
        let mut app = App::new(context.clone());
        app.output = None;
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 8_000])));
        app.studio.pads[0] = Some(0.0);
        app.studio.pads[1] = Some(0.3);
        let untouched = pad(187, 2, 91);
        app.studio.performance = vec![pad(120, 0, 70), untouched.clone(), pad(249, 1, 100)];
        let mut roll = PianoRoll::new(&app);
        let render = |roll: &mut PianoRoll, app: &mut App, events| {
            render_canvas(&context, roll, app, events)
        };
        render(&mut roll, &mut app, vec![]);
        let start = pos2(96.0, 3.0);
        let end = pos2(190.0, 36.0);
        render(
            &mut roll,
            &mut app,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );
        render(
            &mut roll,
            &mut app,
            vec![egui::Event::PointerMoved(end), press(end, false)],
        );
        assert_eq!(
            roll.selection,
            BTreeSet::from([(120, ChopInput::Pad(0)), (249, ChopInput::Pad(1))])
        );
        let start = pos2(114.0, 9.0);
        let end = start + vec2(20.0, 0.0);
        render(
            &mut roll,
            &mut app,
            vec![egui::Event::PointerMoved(start), press(start, true)],
        );
        render(
            &mut roll,
            &mut app,
            vec![egui::Event::PointerMoved(end), press(end, false)],
        );
        let moved = vec![pad(172, 0, 70), untouched, pad(301, 1, 100)];
        assert_eq!(app.studio.performance, moved);
        app.observe_edits(false);
        let moved = app.studio.performance.clone();
        roll.quantize(&mut app.studio);
        assert_eq!(app.studio.performance.iter().map(|note| note.tick).collect::<Vec<_>>(), [120, 187, 360]);
        assert_eq!(app.studio.performance.iter().map(|note| note.id).collect::<Vec<_>>(),
            moved.iter().map(|note| note.id).collect::<Vec<_>>());
        app.observe_edits(false);
        app.undo_project(false);
        assert_eq!(app.studio.performance, moved);
    }

    #[test]
    fn draws_on_an_empty_roll_and_pastes_to_another_bar_without_replacing_hits() {
        let context = Context::default();
        let mut app = App::new(context.clone());
        app.output = None;
        app.library.midi.piano_view = false;
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 8_000])));
        app.studio.bars = 2;
        app.studio.swing = 1.0;
        app.studio.pads[0] = Some(0.0);
        app.studio.pads[1] = Some(0.3);
        let mut roll = PianoRoll::new(&app);
        assert!(roll.draw);
        render_canvas(&context, &mut roll, &mut app, vec![]);
        let click = |roll: &mut PianoRoll, app: &mut App, pos| {
            render_canvas(
                &context,
                roll,
                app,
                vec![egui::Event::PointerMoved(pos), press(pos, true)],
            );
            render_canvas(&context, roll, app, vec![press(pos, false)]);
        };
        click(&mut roll, &mut app, pos2(139.0, 9.0));
        assert_eq!(
            app.studio.performance[0],
            PlayedChop {
                id: Default::default(),
                tick: 180,
                input: ChopInput::Pad(0),
                mode: PadMode::Slice,
                velocity: 100,
                length: None,
                sound: Default::default(),
            expression: None,
            }
        );
        roll.snap_draw = false;
        roll.draw_gate = true;
        roll.velocity = 89;
        roll.hold_ticks = 55;
        click(&mut roll, &mut app, pos2(161.0, 28.0));
        let raw = PlayedChop {
            id: Default::default(),
            tick: 253,
            input: ChopInput::Pad(1),
            mode: PadMode::Gate,
            velocity: 89,
            length: Some(55),
            sound: Default::default(),
            expression: None,
        };
        assert_eq!(app.studio.performance[1], raw);
        click(&mut roll, &mut app, pos2(180.0, 47.0));
        assert_eq!(app.studio.performance.len(), 2);
        roll.select_all(&app.studio);
        let copied = render_canvas(&context, &mut roll, &mut app, vec![egui::Event::Copy]);
        let text = copied
            .platform_output
            .commands
            .into_iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .unwrap();
        let occupied = pad(TICKS_PER_BAR + 180, 0, 44);
        app.studio.performance.push(occupied);
        app.observe_edits(false);
        let before_paste = app.studio.performance.clone();
        let occupied = before_paste[2].clone();
        roll.bar = 1;
        app.piano_roll_clipboard.clear();
        render_canvas(
            &context,
            &mut roll,
            &mut app,
            vec![egui::Event::Paste(text)],
        );
        let pasted = PlayedChop {
            tick: TICKS_PER_BAR + raw.tick,
            ..raw
        };
        assert_eq!(&app.studio.performance[2..], &[occupied, pasted.clone()]);
        assert_eq!(roll.selection, BTreeSet::from([note_id(&pasted)]));
        app.observe_edits(false);
        assert_ne!(app.studio.performance[3].id, before_paste[1].id);
        assert!(!app.studio.performance[3].id.is_empty());
        app.undo_project(false);
        assert_eq!(app.studio.performance, before_paste);

        roll.select_all(&app.studio);
        roll.copy_selected(&mut app, &context);
        roll.bar = 1;
        roll.paste(&mut app);
        assert_eq!(app.studio.performance, before_paste);
        assert_eq!(
            roll.message.as_deref(),
            Some("Add more bars before pasting this selection.")
        );
    }

    #[test]
    fn selected_quantize_preserves_collisions_and_uses_swing() {
        let mut studio = Studio {
            swing: 1.0,
            performance: vec![pad(152, 0, 70), pad(173, 1, 94), pad(180, 0, 108)],
            ..Studio::default()
        };
        let mut roll = PianoRoll {
            track: Track::Pads,
            bar: 0,
            grid: Quantize::Sixteenth,
            strength: 100,
            bars_visible: 1,
            assigned_only: false,
            lane_height: LANE_HEIGHT,
            velocity_lane: false,
            human_ticks: 8,
            human_velocity: 6,
            draw: false,
            snap_draw: true,
            draw_gate: false,
            selection: BTreeSet::from([(152, ChopInput::Pad(0)), (173, ChopInput::Pad(1))]),
            gesture: None,
            focus_row: None,
            velocity: 100,
            hold_ticks: 34,
            message: None,
            transcription_sound: Default::default(),
        };
        roll.quantize(&mut studio);
        assert_eq!(
            studio.performance,
            vec![pad(152, 0, 70), pad(180, 0, 108), pad(180, 1, 94)]
        );
        assert_eq!(
            roll.selection,
            BTreeSet::from([(152, ChopInput::Pad(0)), (180, ChopInput::Pad(1))])
        );
        assert!(roll.message.is_some());
    }
}
