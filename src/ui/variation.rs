use super::{action, responsive_modal, small};
use crate::{
    app::App,
    model::{
        Sequence,
        grid_edit::{GridRect, lane_name},
        variation::{Settings, generate},
    },
};
use eframe::egui::{Context, DragValue, Slider};

pub(crate) struct Dialog {
    base: Sequence,
    settings: Settings,
    selection: Option<(usize, GridRect)>,
    candidates: Option<[Sequence; 4]>,
    choice: usize,
    audition: Option<u64>,
    name: String,
    error: bool,
}

impl Dialog {
    pub fn new(base: Sequence, selection: Option<(usize, GridRect)>) -> Self {
        Self {
            base,
            settings: Settings {
                region: selection,
                ..Settings::default()
            },
            selection,
            candidates: None,
            choice: 0,
            audition: None,
            name: "Variation 1".into(),
            error: false,
        }
    }

    fn stop(&mut self, app: &mut App) {
        if self.audition.is_some() && app.song_playing == self.audition {
            app.stop_song();
        }
        self.audition = None;
    }

    pub fn show(&mut self, app: &mut App, ctx: &Context) -> bool {
        if app.recording || app.studio.active_sequence() != self.base {
            self.stop(app);
            return false;
        }
        let mut close = false;
        let modal = responsive_modal(ctx, "pattern_variations", 570.0, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Pattern variations");
                close = action(ui, "Close", true).clicked();
            });
            small(
                ui,
                "Vary hats, percussion, and programmed pad hits. Kick, snare, and recorded notes stay in place.",
            );
            let before = self.settings.clone();
            ui.horizontal_wrapped(|ui| {
                ui.add(Slider::new(&mut self.settings.density, -100..=100).text("Density"));
                ui.add(Slider::new(&mut self.settings.amount, 0..=100).text("Variation"));
                ui.add(Slider::new(&mut self.settings.fill, 0..=100).text("Fill chance per bar"));
                ui.add(
                    DragValue::new(&mut self.settings.seed)
                        .speed(1)
                        .prefix("Seed "),
                );
            });
            ui.collapsing("Lock lanes", |ui| {
                ui.horizontal_wrapped(|ui| {
                    for lane in [0, 4, 5, 6, 7, 8, 9, 10] {
                        ui.checkbox(&mut self.settings.lanes[lane], lane_name(lane));
                    }
                });
            });
            if let Some((bar, region)) = self.selection {
                let mut locked = self.settings.region.is_some();
                let text = format!("Lock selected cells in bar {}", bar + 1);
                if ui.checkbox(&mut locked, text).changed() {
                    self.settings.region = locked.then_some((bar, region));
                }
            }
            if self.settings != before {
                self.stop(app);
                self.candidates = None;
            }
            ui.horizontal_wrapped(|ui| {
                if action(ui, "Generate 4 candidates", true).clicked() {
                    self.stop(app);
                    self.candidates = Some(generate(&self.base, &self.settings));
                    self.choice = 0;
                }
                let ready = app.can_play_section();
                if action(ui, "Audition original", ready).clicked() {
                    self.audition = app.audition_pattern(self.base.clone());
                }
                if action(ui, "Stop audition", self.audition.is_some()).clicked() {
                    self.stop(app);
                }
            });
            if let Some(candidates) = &self.candidates {
                ui.horizontal_wrapped(|ui| {
                    for index in 0..4 {
                        ui.selectable_value(
                            &mut self.choice,
                            index,
                            format!("Candidate {}", index + 1),
                        );
                    }
                    let seed = self.settings.seed.wrapping_add(self.choice as u64);
                    small(ui, format!("Seed {seed}"));
                });
                let selected = &candidates[self.choice];
                ui.horizontal_wrapped(|ui| {
                    if action(ui, "Audition candidate", app.can_play_section()).clicked() {
                        self.audition = app.audition_pattern(selected.clone());
                    }
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.name);
                    if action(ui, "Keep as new pattern", !self.name.trim().is_empty()).clicked() {
                        close = app.keep_variation(&self.name, selected.clone());
                        self.error = !close;
                    }
                });
            }
            if self.error {
                small(
                    ui,
                    "Use a unique name and make sure the project has room for a new pattern.",
                );
            }
        });
        if close || modal.should_close() {
            self.stop(app);
            return false;
        }
        true
    }
}
