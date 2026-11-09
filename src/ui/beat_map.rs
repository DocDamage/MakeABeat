use super::{BLUE, FAINT, GOLD, MINT, action, small};
use crate::{
    analysis_worker::{self, CHECKPOINT_BYTES},
    app::App,
    model::beat_map::Beat,
};
use eframe::egui::{Color32, DragValue, Painter, Rect, Stroke, Ui};
use std::{ops::RangeInclusive, sync::Arc};

pub(super) fn panel(app: &mut App, ui: &mut Ui) {
    if app.active_clip().is_none() {
        return;
    }
    ui.collapsing("Beat map", |ui| {
        let ready = app.analysis_job().is_none();
        let duration = app.duration();
        let range = app.selection().unwrap_or((0.0, duration));
        let valid_range = (0.2..=analysis_worker::MAX_SECONDS).contains(&(range.1 - range.0));
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Analyze with pulse estimator", ready && valid_range)
                .on_disabled_hover_text("Select 0.2 seconds to 3 minutes of audio.")
                .clicked()
            {
                app.analyze_beat_map(false);
            }
            small(
                ui,
                if app.selection().is_some() {
                    "Analyzes the selection"
                } else {
                    "Analyzes the source"
                },
            );
        });
        model_setup(app, ui, ready, valid_range);
        if let Some(proposal) = app.beat_proposal() {
            let count = proposal.beats.len();
            ui.horizontal_wrapped(|ui| {
                small(ui, format!("{count} proposed beats · Blue on waveform"));
                if action(ui, "Use analysis", true).clicked() {
                    app.adopt_beat_proposal();
                }
                if action(ui, "Dismiss", true).clicked() {
                    app.discard_beat_proposal();
                }
            });
        }
        let source = app.active_source;
        let mut map = app
            .studio
            .beat_maps
            .get(&source)
            .cloned()
            .unwrap_or_default();
        let selection_id = ui.id().with(("beat_index", source));
        let mut index = ui
            .data(|data| data.get_temp::<usize>(selection_id))
            .unwrap_or(0);
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            if action(
                ui,
                "Add beat at playhead",
                (0.0..=duration).contains(&app.position),
            )
            .clicked()
                && let Some(added) = Arc::make_mut(&mut map).add(app.position, duration)
            {
                index = added;
                changed = true;
            }
            if action(ui, "Half tempo", map.beats.len() >= 3).clicked() {
                Arc::make_mut(&mut map).reinterpret(false, duration);
                changed = true;
            }
            if action(ui, "Double tempo", map.beats.len() >= 2).clicked() {
                Arc::make_mut(&mut map).reinterpret(true, duration);
                changed = true;
            }
            if action(
                ui,
                "Clear inferred beats",
                map.beats.iter().any(|beat| !beat.anchor),
            )
            .clicked()
            {
                Arc::make_mut(&mut map).clear_analysis();
                changed = true;
            }
        });
        if !map.beats.is_empty() {
            index = index.min(map.beats.len() - 1);
            ui.horizontal_wrapped(|ui| {
                let mut number = index + 1;
                ui.add(
                    DragValue::new(&mut number)
                        .range(1..=map.beats.len())
                        .prefix("Beat "),
                );
                index = number - 1;
                let beat = map.beats[index];
                let mut at = beat.seconds;
                if ui
                    .add(
                        DragValue::new(&mut at)
                            .range(0.0..=duration)
                            .speed(0.002)
                            .max_decimals(3)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    changed |= Arc::make_mut(&mut map).move_beat(index, at, duration);
                }
                if action(ui, "Show beat", true).clicked() {
                    app.seek(at);
                    app.waveform.center_on(at, duration);
                }
                if action(ui, "Set first downbeat", true).clicked() {
                    Arc::make_mut(&mut map).set_downbeat(index);
                    changed = true;
                }
                if action(ui, "Remove beat", true).clicked() {
                    Arc::make_mut(&mut map).remove(index);
                    changed = true;
                }
                if beat.anchor {
                    small(ui, "User anchor");
                } else if let Some(evidence) = beat.confidence {
                    small(
                        ui,
                        if evidence >= 0.3 {
                            "Strong transient evidence"
                        } else {
                            "Weak transient evidence"
                        },
                    );
                }
            });
            ui.horizontal_wrapped(|ui| {
                if action(
                    ui,
                    "Set suggested phrase starts",
                    map.beats.iter().any(|beat| beat.downbeat),
                )
                .clicked()
                    && let Some(clip) = app.active_clip()
                {
                    Arc::make_mut(&mut map).suggest_phrases(clip);
                    changed = true;
                }
                let phrase = map
                    .beats
                    .get(index)
                    .is_some_and(|beat| map.phrases.contains(&beat.seconds));
                if action(
                    ui,
                    if phrase {
                        "Remove phrase start"
                    } else {
                        "Mark phrase start"
                    },
                    !map.beats.is_empty(),
                )
                .clicked()
                {
                    Arc::make_mut(&mut map).toggle_phrase(index);
                    changed = true;
                }
                small(
                    ui,
                    format!(
                        "{} beats · {} phrase starts",
                        map.beats.len(),
                        map.phrases.len()
                    ),
                );
            });
        }
        ui.data_mut(|data| data.insert_temp(selection_id, index));
        if changed {
            app.studio.beat_maps.insert(source, map.clone());
        }
        if let Some(provenance) = &map.provenance {
            small(
                ui,
                format!(
                    "Last accepted analysis · {} {} · {:.2}–{:.2} s",
                    provenance.model, provenance.version, provenance.range.0, provenance.range.1
                ),
            );
        }
        if action(
            ui,
            "Preview warp from beat map",
            map.beats.len() >= 2 && !app.recording,
        )
        .clicked()
        {
            app.prepare_mapped_warp();
        }
    });
    warp_preview(app, ui);
}

fn model_setup(app: &mut App, ui: &mut Ui, ready: bool, valid_range: bool) {
    ui.collapsing("Beat This! setup", |ui| {
        small(ui, format!("small0 checkpoint · {:.1} MiB · CPU", CHECKPOINT_BYTES as f64 / 1_048_576.0));
        small(ui, "Use a separate Python environment with beat-this 1.1.0 and CPU PyTorch. Runtime installation size depends on the environment.");
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Choose Python executable…", ready).clicked()
                && let Some(path) = rfd::FileDialog::new().set_title("Select the Beat This! environment’s Python").pick_file()
            {
                app.set_beat_python(path);
            }
            if action(ui, "Check environment", ready && app.library.beat_python.is_some()).clicked() {
                app.check_beat_environment();
            }
            if app.beat_environment_checked() { small(ui, "CPU environment available"); }
        });
        let weights = analysis_worker::checkpoint_present();
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Download verified weights", ready).clicked() {
                app.download_beat_weights();
            }
            if weights { small(ui, "Checkpoint cached"); }
            if action(ui, "Analyze with Beat This!", ready && valid_range && weights && app.beat_environment_checked()).clicked() {
                app.analyze_beat_map(true);
            }
        });
    });
}

fn warp_preview(app: &mut App, ui: &mut Ui) {
    let Some(preview) = app.warp_preview() else {
        return;
    };
    let (start, end) = preview.plan.range;
    small(
        ui,
        format!(
            "Warp preview · {start:.3}–{end:.3} s · {:.0} beats at {} BPM · {} interior markers",
            preview.plan.beats,
            preview.bpm,
            preview.plan.markers.len()
        ),
    );
    ui.horizontal_wrapped(|ui| {
        let can_listen = app.output.is_some() && !app.recording;
        if action(ui, "Audition original", can_listen).clicked() {
            app.audition_warp(false);
        }
        if action(ui, "Audition warp", can_listen).clicked() {
            app.audition_warp(true);
        }
        if action(
            ui,
            "Keep as new source",
            app.can_add_source() && !app.recording,
        )
        .clicked()
        {
            app.keep_warp_preview();
        }
        if action(ui, "Edit bounds and markers", app.warp_preview().is_some()).clicked() {
            app.edit_warp_proposal();
        }
        if action(ui, "Dismiss preview", app.warp_preview().is_some()).clicked() {
            app.discard_warp_preview();
        }
    });
}

pub(super) fn paint(app: &App, painter: &Painter, rect: Rect, visible: RangeInclusive<f64>) {
    let span = visible.end() - visible.start();
    if span <= 0.0 {
        return;
    }
    let x = |at: f64| rect.left() + ((at - visible.start()) / span) as f32 * rect.width();
    let draw = |beats: &[Beat], proposed: bool| {
        let mut previous = f32::NEG_INFINITY;
        let first = beats.partition_point(|beat| beat.seconds < *visible.start());
        for beat in beats[first..]
            .iter()
            .take_while(|beat| beat.seconds <= *visible.end())
        {
            let x = x(beat.seconds);
            if x - previous < 5.0 && !beat.downbeat && !beat.anchor {
                continue;
            }
            previous = x;
            let color = if proposed {
                BLUE.gamma_multiply(0.6)
            } else if beat.anchor {
                MINT
            } else if beat.downbeat {
                GOLD
            } else {
                FAINT.gamma_multiply(0.8)
            };
            let strong = beat.downbeat || beat.anchor;
            painter.vline(
                x,
                rect.top()..=if strong {
                    rect.bottom()
                } else {
                    rect.top() + rect.height() * 0.45
                },
                Stroke::new(if strong { 1.4_f32 } else { 1.0_f32 }, color),
            );
        }
    };
    if let Some(proposal) = app.beat_proposal() {
        draw(&proposal.beats, true);
    }
    if let Some(map) = app.studio.beat_maps.get(&app.active_source) {
        draw(&map.beats, false);
        for &at in &map.phrases {
            if visible.contains(&at) {
                painter.vline(
                    x(at),
                    rect.top()..=rect.top() + 10.0,
                    Stroke::new(3.0_f32, Color32::WHITE),
                );
            }
        }
    }
    if let Some(preview) = app.warp_preview() {
        for marker in &preview.plan.markers {
            if visible.contains(&marker.source_seconds) {
                painter.vline(
                    x(marker.source_seconds),
                    rect.bottom() - 12.0..=rect.bottom(),
                    Stroke::new(2.0_f32, BLUE),
                );
            }
        }
    }
}
