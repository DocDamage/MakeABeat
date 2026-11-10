use super::{action, primary, small};
use crate::{
    app::App,
    model::Source,
    separation::sam::{MAX_PROMPT, Part},
};
use eframe::egui::{self, Ui};

pub(super) fn panel(app: &mut App, ui: &mut Ui) {
    small(ui, "SAM Audio Small runs locally on a CUDA GPU. Select 0.10–6 seconds. The previews and saved sources are mono.");
    if let Some(Source::Extracted(origin)) = app.source_record(app.media.active_source()).map(|record| &record.source) {
        let rate = f64::from(origin.sample_rate);
        small(ui, format!("{} from {} · {:.2}–{:.2} s · {}",
            if origin.residual { "Residual" } else { "Target" },
            origin.parent_title, origin.start_frame as f64 / rate, origin.end_frame as f64 / rate, origin.prompt));
    }
    ui.collapsing("SAM Audio setup", |ui| {
        let (python, ffmpeg) = app.extraction_setup();
        small(ui, python.as_deref().map_or_else(
            || "No SAM Audio Python environment selected".into(),
            |path| path.display().to_string(),
        ));
        let idle = app.extraction_job().is_none();
        if action(ui, "Choose Python executable…", idle).clicked() {
            app.choose_extraction_setup(false);
        }
        small(ui, ffmpeg.as_deref().map_or_else(
            || "Shared FFmpeg DLLs from PATH".into(),
            |path| path.display().to_string(),
        ));
        if action(ui, "Choose shared FFmpeg bin folder…", idle).clicked() {
            app.choose_extraction_setup(true);
        }
        small(ui, "Prepare the local Python environment and cache the checkpoint and T5 encoder before extracting.");
        ui.hyperlink_to("SAM Audio checkpoint access", "https://huggingface.co/facebook/sam-audio-small");
    });
    if let Some((start, end)) = app.selection() {
        small(ui, format!("Selection {:.2}–{:.2} s · {:.2} s", start, end, end - start));
    } else {
        small(ui, "Drag on the waveform or set the selection start and end.");
    }
    let job = app.extraction_job();
    ui.label("Sound to extract");
    if ui.add_enabled(job.is_none(), egui::TextEdit::singleline(&mut app.extraction.prompt)
        .char_limit(MAX_PROMPT).hint_text("e.g. bass guitar or backing vocals")).changed()
    {
        app.clear_extraction();
    }
    if let Some((label, state, _)) = job {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            small(ui, if state.cancelled { "Cancelling…" } else { state.stage.unwrap_or(label) });
            if action(ui, "Cancel", !state.cancelled).clicked() {
                app.clear_extraction();
            }
        });
    } else if primary(ui, "Extract selection", app.can_extract()).clicked() {
        app.start_extraction();
    }
    if let Some((message, error)) = &app.extraction.status {
        if *error {
            ui.colored_label(ui.visuals().error_fg_color, message);
        } else {
            small(ui, message);
        }
    }
    if app.extraction_ready() {
        ui.horizontal_wrapped(|ui| {
            let can_play = app.audio.output().is_some() && !app.recording && app.input_recording.is_none();
            for (label, part) in [
                ("Original", Part::Original), ("Target", Part::Target), ("Residual", Part::Residual),
            ] {
                if action(ui, label, can_play).on_hover_text("Preview from the selection start").clicked() {
                    app.preview_extraction(part);
                }
            }
            if action(ui, "Stop preview", app.extraction.playing).clicked() {
                app.stop_extraction_preview();
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (label, parts) in [
                ("Save target", [Part::Target].as_slice()),
                ("Save residual", [Part::Residual].as_slice()),
                ("Save both", [Part::Target, Part::Residual].as_slice()),
            ] {
                if action(ui, label, app.can_save_extraction(parts.len())).clicked() {
                    app.save_extraction(parts);
                }
            }
        });
        small(ui, "Saving adds new sources to the project. Use Save Project to keep them in the .flip file.");
    }
}
