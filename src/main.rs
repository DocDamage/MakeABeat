#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod analysis_jobs;
mod analysis_worker;
mod app;
mod audio;
mod audition;
mod background;
mod catalog;
mod daw;
mod files;
mod groove;
mod harmony;
mod input_audio;
mod library_index;
mod midi;
mod midi_out;
mod model;
mod performance_capture;
mod recommendations;
#[cfg(windows)]
mod platform;
mod search;
mod search_worker;
mod separation;
mod stretch;
mod tempo;
mod transcription;
mod ui;
mod waveform;
mod youtube;

use eframe::egui;

fn main() -> eframe::Result<()> {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--audio-analysis-worker") {
        if let Err(error) = analysis_worker::entry() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 750.0])
            .with_min_inner_size([800.0, 590.0])
            .with_icon(ui::icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Lost Samples • Desktop",
        options,
        Box::new(|cc| {
            ui::style(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc.egui_ctx.clone())))
        }),
    )
}
