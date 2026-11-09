use super::{action, named_number, small};
use crate::model::{
    STEPS, Studio,
    grid_edit::{Cell, GridClipboard, GridRect, LANES, clear_cells, lane_name},
};
use eframe::egui::{ComboBox, DragValue, Ui};

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

    pub fn controls(&mut self, ui: &mut Ui, studio: &mut Studio, bar: usize, editable: bool) {
        if self.bar != bar {
            self.bar = bar;
            self.anchor = None;
            self.corner = None;
            self.dragging = false;
        }
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.checkbox(&mut self.enabled, "Select rectangle").changed() {
                    self.dragging = false;
                }
                if !self.enabled {
                    return;
                }
                let region = self.region();
                if action(ui, "Copy cells", region.is_some()).clicked() && let Some(region) = region {
                    self.clipboard = Some(GridClipboard::copy(studio, bar, region));
                }
                if action(ui, "Cut cells", region.is_some()).clicked() && let Some(region) = region {
                    self.clipboard = Some(GridClipboard::copy(studio, bar, region));
                    clear_cells(studio, bar, region);
                }
                let target = region.map(|region| region.first);
                let can_paste = self.clipboard.as_ref().zip(target)
                    .is_some_and(|(clipboard, target)| clipboard.destination(target).is_some());
                if action(ui, "Paste cells", can_paste)
                    .on_hover_text("Replace cells starting at the upper left of the selection")
                    .on_disabled_hover_text("Copy cells, then select a compatible destination with enough room")
                    .clicked()
                    && let Some((clipboard, target)) = self.clipboard.as_ref().zip(target)
                    && let Some(result) = clipboard.paste(studio, bar, target)
                {
                    self.anchor = Some(result.first);
                    self.corner = Some(result.last);
                }
                if action(ui, "Clear cells", region.is_some()).clicked() && let Some(region) = region {
                    clear_cells(studio, bar, region);
                }
                if let Some(region) = self.region() {
                    small(ui, format!("{} lanes × {} steps", region.lanes().count(), region.steps().count()));
                }
            });
            if self.enabled {
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
                small(ui, "Drag across cells or set the two corners. Paste replaces the destination rectangle.");
            }
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
