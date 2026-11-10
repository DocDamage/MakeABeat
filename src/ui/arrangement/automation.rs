use super::super::{DIM, EDGE, GOLD, INSET, MINT, action, small};
use crate::model::{
    Automation, AutomationPoint, AutomationTarget, Curve, STEPS, Studio, TICKS_PER_BAR, TICKS_PER_STEP,
};
use eframe::egui::{
    self, Align2, ComboBox, CursorIcon, FontId, Pos2, Sense, Stroke, Ui, pos2, vec2,
};
use std::{collections::BTreeSet, ops::Range};

const HEIGHT: f32 = 110.0;
const PADDING: f32 = 9.0;

#[derive(Default)]
pub(super) struct AutomationClipboard {
    automation: Automation,
    source_ticks: u32,
}

impl AutomationClipboard {
    fn copy(studio: &Studio, index: usize, lanes: &[AutomationTarget]) -> Self {
        let range = studio.section_range(index).unwrap();
        let source = &studio.sections[index].automation;
        let mut automation = Automation::default();
        for &target in lanes {
            automation.set_points(target, source.points(target).to_vec());
        }
        Self {
            automation,
            source_ticks: range.end - range.start,
        }
    }
}

enum PasteTiming {
    Original,
    Fit,
}

pub(super) struct Editor {
    target: AutomationTarget,
    section_scope: bool,
    copy_lanes: [bool; AutomationTarget::ALL.len()],
    stroke: Option<StrokeEdit>,
    selected_points: BTreeSet<u32>,
    snap: u32,
    live: bool,
    live_value: u8,
    ghost: Vec<AutomationPoint>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            target: AutomationTarget::MasterVolume,
            section_scope: false,
            copy_lanes: Default::default(),
            stroke: None,
            selected_points: BTreeSet::new(),
            snap: TICKS_PER_STEP / 2,
            live: false,
            live_value: 100,
            ghost: Vec::new(),
        }
    }
}

struct StrokeEdit {
    scope: Scope,
    points: Vec<AutomationPoint>,
    mode: EditMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Song,
    Section(usize),
}

impl Scope {
    fn automation(self, studio: &Studio) -> &Automation {
        match self {
            Self::Song => &studio.automation,
            Self::Section(index) => &studio.sections[index].automation,
        }
    }

    fn automation_mut(self, studio: &mut Studio) -> &mut Automation {
        match self {
            Self::Song => &mut studio.automation,
            Self::Section(index) => &mut studio.sections[index].automation,
        }
    }

    fn range(self, studio: &Studio) -> Range<u32> {
        let limit = crate::model::MAX_SONG_BARS as u32 * TICKS_PER_BAR;
        match self {
            Self::Song => 0..(studio.song_bars() as u32 * TICKS_PER_BAR).min(limit),
            Self::Section(index) => {
                let range = studio.section_range(index).unwrap_or_default();
                range.start.min(limit)..range.end.min(limit)
            }
        }
    }
}

pub(super) struct LaneView {
    pub width: f32,
    pub bar_width: f32,
    pub playhead: Option<usize>,
    pub editable: bool,
    pub selected: Option<usize>,
}

enum EditMode {
    Draw(AutomationPoint),
    Move { index: usize, origin: Pos2 },
}

impl Editor {
    pub fn cancel_stroke(&mut self) {
        self.stroke = None;
    }

    pub fn sections_changed(&mut self) {
        self.cancel_stroke();
        self.selected_points.clear();
    }

    fn scope(&self, studio: &Studio, selected: Option<usize>) -> Scope {
        if self.section_scope
            && let Some(index) = selected.filter(|&index| index < studio.sections.len())
        {
            Scope::Section(index)
        } else {
            Scope::Song
        }
    }

    pub fn controls(
        &mut self,
        ui: &mut Ui,
        studio: &mut Studio,
        selected: &mut Option<usize>,
        clipboard: &mut AutomationClipboard,
        editable: bool,
    ) {
        self.ghost.clear();
        ui.horizontal_wrapped(|ui| {
            let prior_scope = self.section_scope;
            ui.selectable_value(&mut self.section_scope, false, "Song");
            let section = selected.filter(|&index| index < studio.sections.len());
            if let Some(index) = section {
                let name = studio.sections[index].kind.label();
                ui.selectable_value(&mut self.section_scope, true, format!("Section · {name}"))
                    .on_hover_text("Section curves move and copy with this block and override the song lane during it.");
            } else {
                self.section_scope = false;
            }
            let previous = self.target;
            ComboBox::from_id_salt("automation_target")
                .selected_text(self.target.label())
                .show_ui(ui, |ui| {
                    for target in AutomationTarget::ALL {
                        ui.selectable_value(&mut self.target, target, target.label());
                    }
                })
                .response
                .on_hover_text(
                    "Values glide between points and hold before the first and after the last.",
                );
            if previous != self.target || prior_scope != self.section_scope {
                self.cancel_stroke();
                self.selected_points.clear();
            }
            let can_link = section.is_some_and(|index| {
                !studio.automation.points(self.target).is_empty()
                    && studio.sections[index].automation.points(self.target).is_empty()
                    && !Scope::Section(index).range(studio).is_empty()
            });
            if action(ui, "Link lane to section", editable && can_link)
                .on_hover_text("Move this part of the song curve into the selected section. Clear the section lane first if it already has points.")
                .clicked()
                && let Some(index) = section
            {
                self.link_lane(studio, index);
            }
            let scope = self.scope(studio, *selected);
            if action(
                ui,
                "Clear lane",
                editable && !scope.automation(studio).points(self.target).is_empty(),
            )
            .clicked()
            {
                self.commit(studio, scope, Vec::new());
            }
            small(
                ui,
                if self.target == AutomationTarget::LowPass {
                    "100% is fully open. Draw or drag points, right-click to remove."
                } else {
                    "Draw or drag points. Right-click a point to remove it."
                },
            );
        });
        if let Scope::Section(index) = self.scope(studio, *selected) {
            ui.horizontal_wrapped(|ui| {
                let automation = &studio.sections[index].automation;
                if action(ui, "Copy lane", !automation.points(self.target).is_empty())
                    .on_hover_text("Copy only the selected lane. Paste replaces that same lane in the destination section.")
                    .clicked()
                {
                    *clipboard = AutomationClipboard::copy(studio, index, &[self.target]);
                }
                if let Some(lanes) = self.pick_copy_lanes(ui, automation) {
                    *clipboard = AutomationClipboard::copy(studio, index, &lanes);
                }
                if action(ui, "Copy automation", !automation.is_empty())
                    .on_hover_text("Copy every linked lane in this section.")
                    .clicked()
                {
                    *clipboard = AutomationClipboard::copy(studio, index, &AutomationTarget::ALL);
                }
                let has_copy = !clipboard.automation.is_empty();
                let can_paste = editable && has_copy;
                for (title, timing) in [("Paste automation", PasteTiming::Original), ("Paste to fit", PasteTiming::Fit)] {
                    let response = action(ui, title, can_paste)
                        .on_hover_text("Preview the copied curve before replacing the matching lanes.");
                    if response.hovered() {
                        let points = clipboard.automation.points(self.target);
                        self.ghost = match timing {
                            PasteTiming::Original => points.to_vec(),
                            PasteTiming::Fit => fit_points(points, clipboard.source_ticks,
                                Scope::Section(index).range(studio).len() as u32),
                        };
                    }
                    if response.clicked() { self.paste_section(studio, index, clipboard, timing); }
                }
                if has_copy {
                    let copied = AutomationTarget::ALL
                        .into_iter()
                        .filter(|&target| !clipboard.automation.points(target).is_empty())
                        .map(AutomationTarget::label)
                        .collect::<Vec<_>>()
                        .join(", ");
                    small(ui, format!("Clipboard · {copied}"));
                }
            });
        } else {
            self.copy_lanes.fill(false);
        }
    }

    pub fn point_controls(&mut self, ui: &mut Ui, studio: &mut Studio, selected: Option<usize>, editable: bool) {
        let scope = self.scope(studio, selected);
        let mut points = scope.automation(studio).points(self.target).to_vec();
        self.selected_points.retain(|tick| points.iter().any(|point| point.tick == *tick));
        ui.horizontal_wrapped(|ui| {
            let grids = [(1, "Off"), (60, "1/32"), (120, "1/16"), (480, "Beat"), (TICKS_PER_BAR, "Bar")];
            let grid = grids.iter().find(|(ticks, _)| *ticks == self.snap).map_or("Off", |(_, label)| *label);
            ComboBox::from_id_salt("automation_snap").selected_text(format!("Snap · {grid}")).show_ui(ui, |ui| {
                for (ticks, label) in grids { ui.selectable_value(&mut self.snap, ticks, label); }
            });
            let ownership = match scope {
                Scope::Song => "Song lane",
                Scope::Section(_) if points.is_empty() => "Inherited from song",
                Scope::Section(_) => "Section override",
            };
            small(ui, ownership);
            ComboBox::from_id_salt("automation_select")
                .selected_text(format!("{} selected", self.selected_points.len())).show_ui(ui, |ui| {
                    for point in &points {
                        let active = self.selected_points.contains(&point.tick);
                        if ui.selectable_label(active, format!("Tick {} · {}%", point.tick, point.percent)).clicked() {
                            if active { self.selected_points.remove(&point.tick); }
                            else { self.selected_points.insert(point.tick); }
                        }
                    }
                });
            if action(ui, "Select all points", !points.is_empty()).clicked() {
                self.selected_points = points.iter().map(|point| point.tick).collect();
            }
        });
        let Some(first) = points.iter().find(|point| self.selected_points.contains(&point.tick)).copied() else { return };
        ui.add_enabled_ui(editable, |ui| ui.horizontal_wrapped(|ui| {
            let mut change = first;
            let end = scope.range(studio).len() as u32;
            ui.add(egui::DragValue::new(&mut change.percent).range(0..=100).suffix("%"));
            if self.selected_points.len() == 1 {
                ui.add(egui::DragValue::new(&mut change.tick).range(0..=end).prefix("Tick "));
            }
            ComboBox::from_id_salt("automation_shape").selected_text(change.curve.label()).show_ui(ui, |ui| {
                for curve in Curve::ALL { ui.selectable_value(&mut change.curve, curve, curve.label()); }
            });
            if change != first {
                if change.tick != first.tick && points.iter().any(|point| point.tick == change.tick) {
                    small(ui, "A point already occupies that tick.");
                } else {
                    for point in &mut points {
                        if self.selected_points.contains(&point.tick) {
                            point.curve = change.curve;
                            if change.percent != first.percent { point.percent = change.percent; }
                            if self.selected_points.len() == 1 { point.tick = change.tick; }
                        }
                    }
                    if change.tick != first.tick {
                        self.selected_points.clear();
                        self.selected_points.insert(change.tick);
                    }
                    points.sort_by_key(|point| point.tick);
                    self.commit(studio, scope, points.clone());
                }
            }
            if action(ui, "Delete selected", true).clicked() {
                points.retain(|point| !self.selected_points.contains(&point.tick));
                self.selected_points.clear();
                self.commit(studio, scope, points);
            }
        }));
    }

    pub fn live_controls(&mut self, ui: &mut Ui, studio: &mut Studio, selected: Option<usize>, step: Option<usize>) -> bool {
        let scope = self.scope(studio, selected);
        let range = scope.range(studio);
        let tick = step.map(|step| step as u32 * TICKS_PER_STEP).filter(|tick| range.contains(tick));
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.live, "Write automation during playback");
            if ui.add_enabled(self.live && tick.is_some(), egui::Slider::new(&mut self.live_value, 0..=100).suffix("%"))
                .changed() && let Some(tick) = tick
            {
                let mut points = scope.automation(studio).points(self.target).to_vec();
                insert(&mut points, AutomationPoint { tick: tick - range.start, percent: self.live_value, curve: Curve::Linear });
                self.commit(studio, scope, points);
                changed = true;
            }
        });
        changed
    }

    fn pick_copy_lanes(
        &mut self,
        ui: &mut Ui,
        source: &Automation,
    ) -> Option<Vec<AutomationTarget>> {
        let menu = ui
            .add_enabled_ui(!source.is_empty(), |ui| {
                ui.menu_button("Copy lanes…", |ui| {
                    let mut selected = Vec::new();
                    for (target, checked) in
                        AutomationTarget::ALL.into_iter().zip(&mut self.copy_lanes)
                    {
                        if !source.points(target).is_empty() {
                            ui.checkbox(checked, target.label());
                            if *checked {
                                selected.push(target);
                            }
                        }
                    }
                    if action(ui, "Copy selected lanes", !selected.is_empty()).clicked() {
                        ui.close_menu();
                        Some(selected)
                    } else {
                        None
                    }
                })
            })
            .inner;
        let open = menu.inner.is_some();
        let picked = menu.inner.flatten();
        if !open || picked.is_some() {
            self.copy_lanes.fill(false);
        }
        picked
    }

    fn paste_section(
        &mut self,
        studio: &mut Studio,
        index: usize,
        clipboard: &AutomationClipboard,
        timing: PasteTiming,
    ) {
        let range = studio.section_range(index).unwrap();
        let destination_ticks = range.end - range.start;
        let automation = &mut studio.sections[index].automation;
        let mut after = automation.clone();
        for target in AutomationTarget::ALL {
            let points = clipboard.automation.points(target);
            if !points.is_empty() {
                let points = match timing {
                    PasteTiming::Original => points.to_vec(),
                    PasteTiming::Fit => {
                        fit_points(points, clipboard.source_ticks, destination_ticks)
                    }
                };
                after.set_points(target, points);
            }
        }
        if after != *automation {
            *automation = after;
            if clipboard.automation.points(self.target).is_empty()
                && let Some(target) = AutomationTarget::ALL
                    .into_iter()
                    .find(|&target| !clipboard.automation.points(target).is_empty())
            {
                self.target = target;
            }
        }
    }

    fn link_lane(&mut self, studio: &mut Studio, index: usize) {
        let scope = Scope::Section(index);
        let range = scope.range(studio);
        let song = studio.automation.points(self.target).to_vec();
        let baseline = self.target.baseline(studio.sample_level, studio.drum_level);
        let at = |tick| {
            (100.0 * Automation::interpolate(&song, f64::from(tick), baseline)).round() as u8
        };
        let start = at(range.start);
        let end = at(range.end);
        let mut section = vec![AutomationPoint {
            tick: 0,
            percent: start, curve: Default::default() }];
        section.extend(
            song.iter()
                .filter(|point| point.tick > range.start && point.tick < range.end)
                .map(|point| AutomationPoint {
                    tick: point.tick - range.start,
                    ..*point
                }),
        );
        section.push(AutomationPoint {
            tick: range.end - range.start,
            percent: end, curve: Default::default() });
        let mut outside: Vec<_> = song
            .iter()
            .copied()
            .filter(|point| point.tick < range.start || point.tick > range.end)
            .collect();
        if outside.iter().any(|point| point.tick < range.start) {
            insert(
                &mut outside,
                AutomationPoint {
                    tick: range.start,
                    percent: start, curve: Default::default() },
            );
        }
        if outside.iter().any(|point| point.tick > range.end) {
            insert(
                &mut outside,
                AutomationPoint {
                    tick: range.end,
                    percent: end, curve: Default::default() },
            );
        }
        self.commit(studio, scope, section);
        studio.automation.set_points(self.target, outside);
        self.section_scope = true;
    }

    fn commit(&mut self, studio: &mut Studio, scope: Scope, points: Vec<AutomationPoint>) {
        let automation = scope.automation_mut(studio);
        automation.set_points(self.target, points);
    }

    pub fn lane(&mut self, ui: &mut Ui, studio: &mut Studio, view: LaneView) {
        let LaneView {
            width,
            bar_width,
            playhead,
            editable,
            selected,
        } = view;
        let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
        let bounds = rect.shrink2(vec2(0.0, PADDING));
        let scope = self.scope(studio, selected);
        let range = scope.range(studio);
        let end = range.end - range.start;
        if self
            .stroke
            .as_ref()
            .is_some_and(|stroke| stroke.scope != scope)
        {
            self.cancel_stroke();
        }
        let pixels_per_tick = bar_width / TICKS_PER_BAR as f32;
        let x = |tick: u32| bounds.left() + (tick + range.start) as f32 * pixels_per_tick;
        let y = |level: f32| bounds.bottom() - level * bounds.height();
        let graph = rect.with_min_x(x(0)).with_max_x(x(end));
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let original = scope.automation(studio).points(self.target);
        let hit = pointer.filter(|pos| graph.contains(*pos)).and_then(|pos| {
            original
                .iter()
                .enumerate()
                .filter(|(_, point)| point.tick <= end)
                .filter_map(|(index, point)| {
                    let distance =
                        pos.distance(pos2(x(point.tick), y(f32::from(point.percent) * 0.01)));
                    (distance <= 7.0).then_some((index, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index)
        });
        let spacing = if ui.input(|input| input.modifiers.alt) { 1 } else { self.snap };
        let to_point = |pos: Pos2| {
            let tick = ((pos.x - graph.left()) / pixels_per_tick / spacing as f32).round() as i32;
            AutomationPoint {
                tick: (tick * spacing as i32).clamp(0, end as i32) as u32,
                percent: ((bounds.bottom() - pos.y) * 100.0 / bounds.height())
                    .round()
                    .clamp(0.0, 100.0) as u8, curve: Default::default() }
        };

        if editable && response.hovered() && pointer.is_some_and(|pos| graph.contains(pos)) {
            ui.ctx().set_cursor_icon(if hit.is_some() {
                CursorIcon::Grab
            } else {
                CursorIcon::Crosshair
            });
        }
        if editable
            && response.secondary_clicked()
            && let Some(index) = hit
        {
            let mut points = original.to_vec();
            self.selected_points.remove(&points[index].tick);
            points.remove(index);
            self.commit(studio, scope, points);
        } else if editable
            && (response.is_pointer_button_down_on() || response.clicked())
            && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary))
            && let Some(pos) = pointer.filter(|pos| graph.contains(*pos))
        {
            let mut points = original.to_vec();
            let add = ui.input(|input| input.modifiers.shift);
            if !add { self.selected_points.clear(); }
            let mode = if let Some(index) = hit {
                self.selected_points.insert(points[index].tick);
                EditMode::Move { index, origin: pos }
            } else {
                let point = to_point(pos);
                self.selected_points.insert(point.tick);
                insert(&mut points, point);
                EditMode::Draw(point)
            };
            self.stroke = Some(StrokeEdit {
                scope,
                points,
                mode,
            });
        }
        if let Some(stroke) = &mut self.stroke
            && let Some(pos) = pointer
        {
            let point = to_point(pos);
            match &mut stroke.mode {
                EditMode::Draw(last) if *last != point => {
                    let range = last.tick.min(point.tick)..=last.tick.max(point.tick);
                    stroke.points.retain(|point| !range.contains(&point.tick));
                    insert(&mut stroke.points, *last);
                    insert(&mut stroke.points, point);
                    *last = point;
                }
                EditMode::Move { index, origin } => {
                    stroke.points.clear();
                    stroke
                        .points
                        .extend_from_slice(scope.automation(studio).points(self.target));
                    if origin.distance(pos) >= 3.0 {
                        let removed = stroke.points.remove(*index);
                        insert(&mut stroke.points, AutomationPoint { curve: removed.curve, ..point });
                        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                    }
                }
                _ => {}
            }
            if ui.input(|input| input.pointer.primary_down()) {
                let clip = ui.clip_rect();
                let scroll = (clip.left() + 20.0 - pos.x).clamp(0.0, 26.0)
                    - (pos.x - clip.right() + 20.0).clamp(0.0, 26.0);
                if scroll != 0.0 {
                    ui.scroll_with_delta(vec2(scroll, 0.0));
                    ui.ctx().request_repaint();
                }
            }
        }
        if ui.input(|input| !input.pointer.primary_down())
            && let Some(stroke) = self.stroke.take()
        {
            self.commit(studio, stroke.scope, stroke.points);
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, INSET);
        let clip = ui.clip_rect();
        let first_bar = ((clip.left() - bounds.left()) / bar_width).floor().max(0.0) as usize;
        let last_bar =
            ((clip.right().min(graph.right()) - bounds.left()) / bar_width).ceil() as usize;
        for bar in first_bar..=last_bar {
            painter.vline(
                bounds.left() + bar as f32 * bar_width,
                rect.y_range(),
                Stroke::new(0.8_f32, EDGE),
            );
        }
        for percent in [0.0, 0.5, 1.0] {
            painter.hline(graph.x_range(), y(percent), Stroke::new(0.6_f32, EDGE));
            painter.text(
                pos2(clip.left().max(rect.left()) + 4.0, y(percent)),
                Align2::LEFT_CENTER,
                format!("{:.0}%", percent * 100.0),
                FontId::monospace(9.0),
                DIM,
            );
        }
        let points = self
            .stroke
            .as_ref()
            .map_or(scope.automation(studio).points(self.target), |stroke| {
                &stroke.points
            });
        let baseline = self.target.baseline(studio.sample_level, studio.drum_level);
        let (display, offset) = if points.is_empty() && matches!(scope, Scope::Section(_)) {
            (studio.automation.points(self.target), range.start)
        } else {
            (points, 0)
        };
        let color = if points.is_empty() { DIM } else { GOLD };
        let left = clip.left().max(graph.left());
        let right = clip.right().min(graph.right());
        if right > left {
            let mut last = None;
            for column in 0..=((right - left) / 2.0).ceil() as usize {
                let px = (left + column as f32 * 2.0).min(right);
                let tick = ((px - graph.left()) / pixels_per_tick).clamp(0.0, end as f32);
                let point = pos2(px, y(Automation::interpolate(display, f64::from(tick) + f64::from(offset), baseline)));
                if let Some(last) = last { painter.line_segment([last, point], Stroke::new(2.0_f32, color)); }
                last = Some(point);
            }
        }
        for point in points.iter().filter(|point| point.tick <= end) {
            let pos = pos2(x(point.tick), y(f32::from(point.percent) * 0.01));
            if clip.contains(pos) {
                painter.circle_filled(pos, 3.6, if self.selected_points.contains(&point.tick) { MINT } else { GOLD });
            }
        }
        for pair in self.ghost.windows(2).filter(|pair| pair[1].tick <= end) {
            let point = |point: AutomationPoint| pos2(x(point.tick), y(f32::from(point.percent) * 0.01));
            painter.line_segment([point(pair[0]), point(pair[1])], Stroke::new(1.5_f32, MINT.gamma_multiply(0.6)));
        }
        if let Some(step) = playhead {
            let x = bounds.left() + step as f32 / STEPS as f32 * bar_width;
            painter.vline(x, rect.y_range(), Stroke::new(1.5_f32, MINT));
        }
    }
}

fn fit_points(
    points: &[AutomationPoint],
    source_ticks: u32,
    destination_ticks: u32,
) -> Vec<AutomationPoint> {
    let visible = points.partition_point(|point| point.tick <= source_ticks);
    let source_length = u64::from(source_ticks);
    let mut fitted = Vec::with_capacity(visible);
    for point in &points[..visible] {
        let scaled = u64::from(point.tick) * u64::from(destination_ticks);
        let tick = ((scaled + source_length / 2) / source_length) as u32;
        insert(&mut fitted, AutomationPoint { tick, ..*point });
    }
    // Preserve the curve at the section boundary if a resize left points beyond it.
    if visible < points.len() && (visible == 0 || points[visible - 1].tick < source_ticks) {
        let value = Automation::interpolate(points, f64::from(source_ticks), 0.0);
        insert(
            &mut fitted,
            AutomationPoint {
                tick: destination_ticks,
                percent: (value * 100.0).round() as u8, curve: Default::default() },
        );
    }
    fitted
}

fn insert(points: &mut Vec<AutomationPoint>, point: AutomationPoint) {
    match points.binary_search_by_key(&point.tick, |point| point.tick) {
        Ok(index) => points[index] = point,
        Err(index) => points.insert(index, point),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Section, SectionKind};

    #[test]
    fn pasting_only_automation_preserves_offsets_and_other_lanes_and_can_be_undone() {
        let mut studio = Studio {
            bars: 2,
            ..Studio::default()
        };
        studio.save_pattern("Intro", false);
        studio.save_pattern("Hook", true);
        studio.sections = vec![
            Section::new(SectionKind::Intro, 0, 1),
            Section::new(SectionKind::Hook, 1, 1),
        ];
        let volume = AutomationTarget::MasterVolume;
        let reverb = AutomationTarget::Reverb;
        let delay = AutomationTarget::Delay;
        let point = |tick, percent| AutomationPoint { tick, percent, curve: Default::default() };
        studio.sections[0]
            .automation
            .set_points(volume, vec![point(0, 0), point(2 * TICKS_PER_BAR, 100)]);
        studio.sections[0]
            .automation
            .set_points(delay, vec![point(120, 30)]);
        studio.sections[1]
            .automation
            .set_points(volume, vec![point(0, 80)]);
        studio.sections[1]
            .automation
            .set_points(reverb, vec![point(0, 50)]);
        studio.automation.set_points(volume, vec![point(0, 91)]);
        let before = studio.clone();
        let clipboard = AutomationClipboard::copy(&studio, 0, &AutomationTarget::ALL);
        let mut editor = Editor::default();
        editor.paste_section(&mut studio, 1, &clipboard, PasteTiming::Original);
        let mut expected = before.clone();
        expected.sections[1]
            .automation
            .set_points(volume, clipboard.automation.points(volume).to_vec());
        expected.sections[1]
            .automation
            .set_points(delay, clipboard.automation.points(delay).to_vec());
        assert_eq!(studio, expected);
        let song = studio.song_sequence().unwrap();
        let (hook, tick) = song
            .section_automation_at(f64::from(2 * TICKS_PER_BAR + TICKS_PER_BAR / 2))
            .unwrap();
        assert!((hook.value(volume, tick, 1.0) - 0.25).abs() < 1e-6);
        let mut app = crate::app::App::new(egui::Context::default());
        app.project.studio = before.clone();
        app.observe_edits(false);
        let before = app.project.studio.clone();
        app.project.studio = studio;
        app.observe_edits(false);
        app.undo_project(false);
        assert_eq!(app.project.studio, before);
    }

    #[test]
    fn fitted_paste_resizes_curves_to_the_destination_section() {
        let mut studio = Studio::default();
        studio.save_pattern("Beat", true);
        studio.sections = vec![
            Section::new(SectionKind::Intro, 0, 2),
            Section::new(SectionKind::Hook, 0, 1),
        ];
        let bar = TICKS_PER_BAR;
        let volume = AutomationTarget::MasterVolume;
        let delay = AutomationTarget::Delay;
        let point = |tick, percent| AutomationPoint { tick, percent, curve: Default::default() };
        studio.sections[0]
            .automation
            .set_points(volume, vec![point(0, 0), point(4 * bar, 100)]);
        studio.sections[0]
            .automation
            .set_points(delay, vec![point(119, 25), point(120, 30)]);
        let before = studio.clone();
        let clipboard = AutomationClipboard::copy(&studio, 0, &AutomationTarget::ALL);
        let mut editor = Editor::default();

        editor.paste_section(&mut studio, 1, &clipboard, PasteTiming::Fit);
        assert_eq!(
            studio.sections[1].automation.points(volume),
            &[point(0, 0), point(bar, 50)]
        );
        assert_eq!(
            studio.sections[1].automation.points(delay),
            &[point(60, 30)]
        );
        assert_eq!(studio.sections[0], before.sections[0]);

        let clipboard = AutomationClipboard::copy(&studio, 1, &AutomationTarget::ALL);
        studio = before;
        editor.paste_section(&mut studio, 0, &clipboard, PasteTiming::Fit);
        assert_eq!(
            studio.sections[0].automation.points(volume),
            &[point(0, 0), point(2 * bar, 50)]
        );
        assert_eq!(
            studio.sections[0].automation.points(delay),
            &[point(120, 30)]
        );
    }

    #[test]
    fn fitted_paste_only_replaces_the_selected_lanes() {
        let mut studio = Studio::default();
        studio.save_pattern("Beat", true);
        studio.sections = vec![
            Section::new(SectionKind::Intro, 0, 1),
            Section::new(SectionKind::Hook, 0, 2),
        ];
        let filter = AutomationTarget::LowPass;
        let delay = AutomationTarget::Delay;
        let bar = TICKS_PER_BAR;
        let point = |tick, percent| AutomationPoint { tick, percent, curve: Default::default() };
        studio.sections[0].automation.set_points(
            filter,
            vec![point(0, 25), point(bar / 2, 60), point(bar, 100)],
        );
        studio.sections[1]
            .automation
            .set_points(filter, vec![point(0, 50)]);
        studio.sections[0]
            .automation
            .set_points(delay, vec![point(bar / 4, 25), point(bar, 75)]);
        studio.sections[1]
            .automation
            .set_points(delay, vec![point(0, 90)]);
        for target in [AutomationTarget::MasterVolume, AutomationTarget::Reverb] {
            studio.sections[0]
                .automation
                .set_points(target, vec![point(0, 20)]);
            studio.sections[1]
                .automation
                .set_points(target, vec![point(0, 70)]);
        }
        let before = studio.clone();
        let mut expected = before.clone();
        expected.sections[1].automation.set_points(
            filter,
            vec![point(0, 25), point(bar, 60), point(2 * bar, 100)],
        );
        let mut editor = Editor::default();
        for lanes in [&[filter][..], &[filter, delay]] {
            if lanes.contains(&delay) {
                expected.sections[1]
                    .automation
                    .set_points(delay, vec![point(bar / 2, 25), point(2 * bar, 75)]);
            }
            let clipboard = AutomationClipboard::copy(&studio, 0, lanes);
            editor.paste_section(&mut studio, 1, &clipboard, PasteTiming::Fit);
            assert_eq!(studio, expected);
            assert_eq!(editor.target, filter);

        }
    }

    #[test]
    fn dragging_draws_moves_and_links_a_fade() {
        let ctx = egui::Context::default();
        let mut studio = Studio {
            bars: 2,
            ..Studio::default()
        };
        studio.save_pattern("Intro", false);
        studio.sections.push(Section::new(SectionKind::Intro, 0, 1));
        let mut editor = Editor::default();
        let render = |studio: &mut Studio, editor: &mut Editor, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(300.0, 160.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            editor.lane(
                                ui,
                                studio,
                                LaneView {
                                    width: 300.0,
                                    bar_width: 72.0,
                                    playhead: None,
                                    editable: true,
                                    selected: Some(0),
                                },
                            );
                        });
                },
            );
        };
        let drag = |studio: &mut Studio, editor: &mut Editor, start, end| {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            render(
                studio,
                editor,
                vec![egui::Event::PointerMoved(start), button(start, true)],
            );
            render(
                studio,
                editor,
                vec![egui::Event::PointerMoved(end), button(end, false)],
            );
        };
        render(&mut studio, &mut editor, vec![]);
        drag(&mut studio, &mut editor, pos2(0.5, 101.0), pos2(72.0, 9.0));
        assert_eq!(
            studio.automation.points(AutomationTarget::MasterVolume),
            [
                AutomationPoint {
                    tick: 0,
                    percent: 0, curve: Default::default() },
                AutomationPoint {
                    tick: TICKS_PER_BAR,
                    percent: 100, curve: Default::default() },
            ]
        );
        drag(&mut studio, &mut editor, pos2(72.0, 9.0), pos2(108.0, 32.0));
        editor.link_lane(&mut studio, 0);
        assert_eq!(
            studio.sections[0]
                .automation
                .points(AutomationTarget::MasterVolume),
            [
                AutomationPoint {
                    tick: 0,
                    percent: 0, curve: Default::default() },
                AutomationPoint {
                    tick: TICKS_PER_BAR * 3 / 2,
                    percent: 75, curve: Default::default() },
                AutomationPoint {
                    tick: 2 * TICKS_PER_BAR,
                    percent: 75, curve: Default::default() },
            ]
        );
        drag(
            &mut studio,
            &mut editor,
            pos2(144.0, 32.0),
            pos2(144.0, 55.0),
        );
        assert!(studio.automation.is_empty());
        assert_eq!(
            studio.sections[0]
                .automation
                .points(AutomationTarget::MasterVolume)
                .last()
                .unwrap()
                .percent,
            50
        );
    }
}
