use super::{action, sample_controls, small};
use crate::{
    app::App,
    model::{
        SampleControls,
        instrument::{HitSound, Layer, Layers, VelocityRange},
    },
};
use eframe::egui::{ComboBox, DragValue, Ui};

pub(super) fn hit_sound(ui: &mut Ui, sound: &mut HitSound) -> bool {
    let before = *sound;
    ui.menu_button("Hit sound…", |ui| {
        ui.add(
            DragValue::new(&mut sound.pitch_cents)
                .range(-2400..=2400)
                .prefix("Pitch ")
                .suffix(" cents"),
        );
        ui.add(
            DragValue::new(&mut sound.start_percent)
                .range(0..=99)
                .prefix("Start offset ")
                .suffix("%"),
        );
        let mut filtered = sound.cutoff_hz.is_some();
        if ui.checkbox(&mut filtered, "Low-pass filter").changed() {
            sound.cutoff_hz = filtered.then_some(2_000);
        }
        if let Some(cutoff) = &mut sound.cutoff_hz {
            ui.add(
                DragValue::new(cutoff)
                    .range(20..=20_000)
                    .speed(10)
                    .prefix("Cutoff ")
                    .suffix(" Hz"),
            );
        }
        if action(ui, "Reset hit sound", true).clicked() {
            *sound = HitSound::default();
        }
        small(
            ui,
            "Pitch is relative to the instrument. Start offset follows the playback direction.",
        );
    });
    *sound != before
}

pub(super) fn edit(ui: &mut Ui, app: &App, layers: &mut Layers) {
    ui.collapsing("Sample layers", |ui| {
        small(ui, "Overlapping velocity ranges play together.");
        velocity(ui, "Base layer", &mut layers.base);
        let selected = app.selection().or_else(|| {
            let duration = app.source_duration(app.media.active_source());
            (duration > 0.0).then_some((0.0, duration))
        });
        for (index, slot) in layers.extra.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                let title = format!("Layer {}", index + 2);
                let Some(layer) = slot else {
                    if action(ui, &format!("Add {title}"), selected.is_some()).clicked()
                        && let Some((start, end)) = selected
                    {
                        *slot = Some(Layer {
                            source: app.media.active_source(),
                            start,
                            end,
                            controls: SampleControls::default(),
                            velocity: VelocityRange::default(),
                        });
                    }
                    return;
                };
                let mut remove = false;
                ui.collapsing(title, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let old = layer.source;
                        let name = app
                            .source_record(old)
                            .map_or("Project audio", |record| &record.title);
                        ComboBox::from_id_salt("layer_source")
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                for source in 0..=app.project.studio.sources.len() {
                                    if app.source_duration(source) > 0.0 {
                                        let title = app
                                            .source_record(source)
                                            .map_or("Project audio", |record| &record.title);
                                        ui.selectable_value(&mut layer.source, source, title);
                                    }
                                }
                            });
                        if layer.source != old {
                            layer.start = 0.0;
                            layer.end = app.source_duration(layer.source);
                        }
                        if action(ui, "Use selection", app.selection().is_some()).clicked()
                            && let Some((start, end)) = app.selection()
                        {
                            layer.source = app.media.active_source();
                            layer.start = start;
                            layer.end = end;
                        }
                        remove = action(ui, "Remove", true).clicked();
                    });
                    let duration = app.source_duration(layer.source);
                    let gap = duration.min(0.001);
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            DragValue::new(&mut layer.start)
                                .range(0.0..=(layer.end - gap).max(0.0))
                                .prefix("Start ")
                                .suffix(" s")
                                .speed(0.001),
                        );
                        ui.add(
                            DragValue::new(&mut layer.end)
                                .range((layer.start + gap).min(duration)..=duration)
                                .prefix("End ")
                                .suffix(" s")
                                .speed(0.001),
                        );
                    });
                    velocity(ui, "Velocity", &mut layer.velocity);
                    sample_controls(ui, &mut layer.controls);
                });
                if remove {
                    *slot = None;
                }
            });
        }
    });
}

fn velocity(ui: &mut Ui, title: &str, range: &mut VelocityRange) {
    ui.horizontal_wrapped(|ui| {
        ui.label(title);
        ui.add(
            DragValue::new(&mut range.min)
                .range(1..=range.max)
                .prefix("Min velocity "),
        );
        ui.add(
            DragValue::new(&mut range.max)
                .range(range.min..=127)
                .prefix("Max velocity "),
        );
    });
}
