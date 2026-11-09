use super::{App, DIM, action, label, small};
use crate::model::{MAX_SAMPLE_NAME, STEPS, grid_edit, groove::MAX_OFFSET};
use eframe::egui::{self, ComboBox, DragValue, Ui};

pub fn panel(app: &mut App, ui: &mut Ui, sample_view: bool) {
    let mut controls = |ui: &mut Ui| {
        let mut selected = None;
        ui.horizontal_wrapped(|ui| {
            if sample_view && action(ui, "Extract from selected break", app.selection().is_some()).clicked() {
                app.extract_groove();
            }
            if action(ui, "Import groove…", true).clicked() {
                app.import_groove();
            }
            ComboBox::from_id_salt("groove_library").selected_text(
                app.groove_editor.saved.and_then(|i| app.studio.grooves.get(i))
                    .map_or("Saved grooves", |groove| groove.name.as_str()),
            ).show_ui(ui, |ui| {
                for (i, groove) in app.studio.grooves.iter().enumerate() {
                    if ui.selectable_label(app.groove_editor.saved == Some(i), &groove.name).clicked() {
                        selected = Some(i);
                    }
                }
            });
        });
        if let Some(index) = selected {
            app.select_groove(index);
        }
        let Some(draft) = &mut app.groove_editor.draft else {
            small(ui, "Select complete bars on the sample waveform using the beat map, or open a saved groove.");
            return;
        };
        let mut save = false;
        let mut export = false;
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.name).hint_text("Groove name")
                .desired_width(160.0).char_limit(MAX_SAMPLE_NAME));
            save = action(ui, if app.groove_editor.saved.is_some() { "Save changes" } else { "Save groove" },
                !draft.name.trim().is_empty()).clicked();
            export = action(ui, "Export groove…", !draft.name.trim().is_empty()).clicked();
        });
        small(ui, format!("{} bars · {} · {:.3}–{:.3} s",
            draft.bars, draft.source.title, draft.source.range.0, draft.source.range.1));
        ui.collapsing("Adjust extracted anchors", |ui| {
            small(ui, "Move an anchor earlier or later in ticks. An accent of 1.00× keeps the original velocity.");
            egui::ScrollArea::vertical().id_salt("groove_anchors").max_height(165.0).show(ui, |ui| {
                egui::Grid::new("groove_slots").striped(true).show(ui, |ui| {
                    for (step, slot) in draft.slots.iter_mut().enumerate() {
                        ui.label(format!("Bar {} · Step {}", step / STEPS + 1, step % STEPS + 1));
                        ui.add(DragValue::new(&mut slot.offset).range(-MAX_OFFSET..=MAX_OFFSET).suffix(" ticks"));
                        ui.add(DragValue::new(&mut slot.accent).range(0.0..=4.0).speed(0.01)
                            .fixed_decimals(2).suffix("×"));
                        ui.end_row();
                    }
                });
            });
        });
        if save { app.save_groove(); }
        if export { app.export_groove(); }
        let transfer = &mut app.groove_editor.transfer;
        ui.horizontal_wrapped(|ui| {
            ui.add(DragValue::new(&mut transfer.timing).range(0..=100).prefix("Timing ").suffix("%"));
            ui.add(DragValue::new(&mut transfer.velocity).range(0..=100).prefix("Velocity ").suffix("%"));
            ui.checkbox(&mut transfer.recorded, "Include recorded hits on selected pad and key lanes");
        });
        ui.horizontal_wrapped(|ui| {
            for (row, selected) in transfer.lanes.iter_mut().enumerate() {
                ui.checkbox(selected, grid_edit::lane_name(row));
            }
        });
        let can_listen = app.can_play_section() && app.studio.has_pattern();
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Play before", can_listen).clicked() { app.preview_groove(false); }
            if action(ui, "Play after", can_listen).clicked() { app.preview_groove(true); }
            if app.groove_preview_playing() && action(ui, "Stop comparison", true).clicked() { app.stop_groove_preview(); }
            if action(ui, "Apply groove", !app.recording && app.studio.has_pattern()).clicked() { app.apply_groove(); }
        });
    };
    controls(ui);
}

pub fn audition(app: &mut App, ui: &mut Ui) {
    let Some(preview) = &app.audition.preview else {
        return;
    };
    let mut bars = preview.bars;
    let mut fit = None;
    let mut gain = (app.audition.gain * 100.0).round() as u8;
    let mut start = false;
    let mut stop = false;
    let mapped = preview.prepared.as_ref().is_some_and(|p| p.mapped);
    let fitted = preview
        .prepared
        .as_ref()
        .is_some_and(|p| p.fitted.is_some());
    let can_play = app.output.is_some() && !app.recording;
    ui.horizontal_wrapped(|ui| {
        ui.label(label("AUDITION WITH BEAT", 12.0, DIM).strong());
        small(ui, &preview.title);
        if !mapped && can_play && !preview.external {
            ComboBox::from_id_salt("audition_manual_bars")
                .selected_text(
                    bars.map_or_else(|| "Choose cut length".into(), |bars| format!("{bars} bars")),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut bars, None, "Choose cut length");
                    for count in 1..=crate::audition::MAX_BARS {
                        ui.selectable_value(
                            &mut bars,
                            Some(count),
                            if count == 1 {
                                "1 bar".into()
                            } else {
                                format!("{count} bars")
                            },
                        );
                    }
                });
        }
        if action(ui, "Original", can_play && preview.prepared.is_some()).clicked() {
            fit = Some(false);
        }
        if action(ui, "Fit", can_play && fitted).clicked() {
            fit = Some(true);
        }
        ui.add(
            DragValue::new(&mut gain)
                .range(0..=100)
                .prefix("Preview ")
                .suffix("%"),
        );
        if !app.playing && !preview.external {
            start = action(ui, "Play beat", can_play).clicked();
        }
        stop = action(ui, "Close audition", true).clicked();
    });
    let transport = app
        .output
        .as_ref()
        .map_or(0, |output| output.meters.beat_audition());
    let state = if preview.preparing {
        "Preparing audio…"
    } else if !app.playing && can_play && preview.prepared.is_some() {
        "Ready to join when the beat plays"
    } else if transport == 1 {
        "Joining on the next bar"
    } else if transport == 2 && preview.fit && fitted {
        "Playing fitted audio"
    } else if transport == 2 {
        "Playing at original speed"
    } else {
        ""
    };
    ui.horizontal_wrapped(|ui| {
        if !state.is_empty() {
            small(ui, state);
        }
        if let Some(prepared) = &preview.prepared {
            small(
                ui,
                format!(
                    "{:.3}–{:.3} s{}",
                    prepared.range.0,
                    prepared.range.1,
                    if mapped { " · Beat map downbeat" } else { "" }
                ),
            );
        }
        if let Some(status) = &preview.status {
            small(ui, status);
        }
    });
    if stop {
        app.stop_beat_audition();
    } else {
        if bars != preview.bars {
            app.set_beat_audition_bars(bars);
        } else if let Some(fit) = fit {
            app.set_beat_audition_fit(fit);
        }
        let gain = f32::from(gain) / 100.0;
        if gain != app.audition.gain {
            app.set_beat_audition_gain(gain);
        }
        if start {
            app.start_beat_audition_transport();
        }
    }
}
