use super::{App, DIM, TEXT, action, label, primary, small, time};
use crate::{
    app::search_workflow::UseHit,
    library_index::{Kind, MAX_PASSAGES},
    model::MAX_NAMED_SAMPLES,
};
use eframe::egui::{self, ComboBox, DragValue, Ui};

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.label(label("Search local samples", 26.0, TEXT).strong());
    small(
        ui,
        "Describe the sound you want. Search uses audio already on this computer.",
    );
    ui.horizontal_wrapped(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(&mut app.search.text)
                .hint_text("Dusty drum break with a bright snare")
                .desired_width(380.0)
                .char_limit(256),
        );
        let enter = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if primary(ui, "Search", !app.search.text.trim().is_empty()).clicked() || enter {
            app.search_sample_text();
        }
    });
    ui.collapsing("Filters", |ui| {
        let filters = &mut app.search.filters;
        ui.horizontal_wrapped(|ui| {
            ComboBox::from_id_salt("search_source").selected_text(filters.source.map_or("Any source", Kind::label))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.source, None, "Any source");
                    for kind in Kind::ALL { ui.selectable_value(&mut filters.source, Some(kind), kind.label()); }
                });
            ui.add(DragValue::new(&mut filters.min_seconds).range(0.0..=12.0).speed(0.1).prefix("Min ").suffix(" s"));
            ui.add(DragValue::new(&mut filters.max_seconds).range(0.0..=12.0).speed(0.1).prefix("Max ").suffix(" s"));
            ui.add(DragValue::new(&mut filters.min_bpm).range(0..=300).prefix("Min BPM "));
            ui.add(DragValue::new(&mut filters.max_bpm).range(0..=300).prefix("Max BPM "));
            ui.add(egui::TextEdit::singleline(&mut filters.instrument).hint_text("Instrument tag")
                .desired_width(135.0).char_limit(60));
        });
        small(ui, "Zero leaves a bound open. Tempo and instrument filters use indexed metadata. Run the search again to apply filters.");
        if action(ui, "Clear filters", true).clicked() { *filters = Default::default(); }
    });
    ui.collapsing("Index and model setup", |ui| index(app, ui));
    if let Some((label, state, queued)) = app.search_job() {
        ui.horizontal_wrapped(|ui| {
            if !app.library.interface.reduce_motion {
                ui.spinner();
            }
            small(ui, state.stage.unwrap_or(label));
            if let Some(progress) = state.progress {
                ui.add(
                    egui::ProgressBar::new(progress.clamp(0.0, 1.0))
                        .desired_width(125.0)
                        .show_percentage(),
                );
            }
            if queued > 0 {
                small(ui, format!("{queued} queued"));
            }
            if action(ui, "Pause", !state.cancelled).clicked() {
                app.pause_sample_search();
            }
        });
    }
    if let Some((message, is_error)) = &app.search.status {
        ui.label(label(
            message,
            13.0,
            if *is_error {
                egui::Color32::LIGHT_RED
            } else {
                DIM
            },
        ));
    }
    ui.separator();
    if let Some(description) = &app.search.results_for {
        ui.label(label(description, 16.0, TEXT).strong());
        if app.search.results.is_empty() {
            small(
                ui,
                "No indexed passages matched. Check the filters and index status.",
            );
        }
    } else if app.search.summary.passages == 0 {
        small(ui, "Open Index and model setup to add a source.");
    }
    let pad = Some(app.selected_pad)
        .filter(|&i| app.studio.pads[i].is_none())
        .or_else(|| app.studio.pads.iter().position(Option::is_none));
    let can_add = app.can_add_source() && !app.recording;
    let mut pending = None;
    for (index, hit) in app.search.results.iter().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    label(format!("{}. {}", index + 1, hit.location.title), 15.0, TEXT).strong(),
                );
                let (start, end) = (
                    hit.range.0 + hit.location.timeline,
                    hit.range.1 + hit.location.timeline,
                );
                small(
                    ui,
                    format!(
                        "{}–{} · {}",
                        time(start, true),
                        time(end, true),
                        hit.location.kind.label()
                    ),
                );
                if let Some(bpm) = hit.location.bpm {
                    small(ui, format!("{bpm} BPM"));
                }
                if !hit.location.instrument.is_empty() {
                    small(ui, &hit.location.instrument);
                }
                ui.label(label(format!("Score {:.3}", hit.score), 12.0, DIM))
                    .on_hover_text("A ranking score within this search");
            });
            ui.horizontal_wrapped(|ui| {
                if action(ui, "Audition with beat", app.output.is_some()).clicked() {
                    pending = Some((hit.clone(), UseHit::Audition));
                }
                if let Some(pad) = pad
                    && action(ui, &format!("Assign to pad {}", pad + 1), can_add).clicked()
                {
                    pending = Some((hit.clone(), UseHit::Assign(pad)));
                }
                if action(
                    ui,
                    "Save cut",
                    can_add && app.samples.len() < MAX_NAMED_SAMPLES,
                )
                .clicked()
                {
                    pending = Some((hit.clone(), UseHit::Save));
                }
            });
            ui.separator();
        });
    }
    if let Some((hit, action)) = pending {
        app.use_search_hit(hit, action);
    }
}

fn index(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        let python = app.search_python();
        small(ui, format!("Python · {}", python.display()));
        if action(ui, "Choose Python…", true).clicked() {
            app.choose_search_python();
        }
        if action(ui, "Download CLAP model", app.search_job().is_none()).clicked() {
            app.setup_sample_search();
        }
    });
    small(
        ui,
        "Install the pinned Python packages from the worker README before downloading the model.",
    );
    let idle = app.search_job().is_none();
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.search.tag)
                .hint_text("Instrument tag for new sources")
                .desired_width(185.0)
                .char_limit(60),
        );
        if action(ui, "Add folder…", idle).clicked() {
            app.index_search_folder();
        }
        if action(ui, "Index saved cuts", idle && !app.samples.is_empty()).clicked() {
            app.index_search_cuts();
        }
        if action(ui, "Index cached downloads", idle).clicked() {
            app.index_search_downloads();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let summary = &app.search.summary;
        small(
            ui,
            format!(
                "{} / {} passages · {} pending files · {} failed",
                summary.passages, MAX_PASSAGES, summary.pending, summary.failed
            ),
        );
        let has_roots = !summary.roots.is_empty();
        if action(ui, "Rescan / resume", idle && has_roots).clicked() {
            app.resume_sample_index();
        }
        if action(ui, "Rebuild index", idle && has_roots).clicked() {
            app.rebuild_sample_search();
        }
    });
    if !app.search.summary.errors.is_empty() {
        ui.collapsing("Indexing issues", |ui| {
            for (title, error) in &app.search.summary.errors {
                small(ui, format!("{title} · {error}"));
            }
        });
    }
    let mut remove = None;
    for root in &app.search.summary.roots {
        ui.push_id(&root.key, |ui| {
            ui.horizontal_wrapped(|ui| {
                small(ui, &root.name);
                if action(ui, "Remove from index", idle).clicked() {
                    remove = Some(root.key.clone());
                }
            });
        });
    }
    if let Some(key) = remove {
        app.remove_search_root(key);
    }
}
