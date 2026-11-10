use super::{action, primary, small};
use crate::{app::App, files::WavOptions};
use eframe::egui::{ComboBox, Context, DragValue};

pub(super) fn midi_mode(ui: &mut eframe::egui::Ui, mode: &mut crate::transcription::midi::Mode) {
    use crate::transcription::midi::Mode;
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(mode, Mode::PerNote, "Per-note MIDI bends");
        ui.selectable_value(mode, Mode::Quantized, "Quantized pitch");
    });
    small(ui, "Per-note bends use channels 3–9 and 11–16. Use Quantized pitch for a single keys channel or WAV to keep the bends in audio.");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Wave,
    Cut,
    Midi,
    Daw,
    #[cfg(windows)]
    Drag,
}

pub(crate) struct Dialog {
    pattern_only: bool,
    format: Format,
    wav: WavOptions,
    midi: crate::transcription::midi::Mode,
    pad: bool,
    processed: bool,
    #[cfg(windows)]
    delete_drag: bool,
}

impl Dialog {
    pub fn new(app: &App) -> Self {
        Self {
            pattern_only: app.project.studio.sections.is_empty(),
            format: Format::Wave,
            wav: WavOptions::default(),
            midi: Default::default(),
            pad: false,
            processed: false,
            #[cfg(windows)]
            delete_drag: false,
        }
    }

    pub fn show(&mut self, app: &mut App, ctx: &Context) -> bool {
        let mut close = false;
        let modal = super::responsive_modal(ctx, "project_export", 430.0, |ui| {
            ui.heading("Export project audio");
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.format, Format::Wave, "WAV mix");
                ui.selectable_value(&mut self.format, Format::Cut, "WAV cut");
                ui.selectable_value(&mut self.format, Format::Midi, "MIDI");
                ui.selectable_value(&mut self.format, Format::Daw, "DAW pack");
                #[cfg(windows)]
                ui.selectable_value(&mut self.format, Format::Drag, "Drag WAV");
            });
            #[cfg(windows)]
            if self.format == Format::Drag {
                self.drag_controls(app, ui);
                close = action(ui, "Close", true).clicked();
                return;
            }
            let has_song = !app.project.studio.sections.is_empty();
            self.pattern_only |= !has_song;
            ui.horizontal_wrapped(|ui| {
                ui.label("Export range");
                if self.format == Format::Cut {
                    ui.selectable_value(&mut self.pad, false, "Waveform selection");
                    ui.selectable_value(&mut self.pad, true, "Selected pad");
                    ui.checkbox(&mut self.processed, "Processed");
                } else {
                    ui.selectable_value(&mut self.pattern_only, true, "Current pattern");
                    ui.add_enabled_ui(has_song, |ui| {
                        ui.selectable_value(&mut self.pattern_only, false, "Song");
                    });
                }
            });
            let bars = if self.pattern_only {
                app.project.studio.bars
            } else {
                app.project.studio.song_bars()
            };
            if matches!(self.format, Format::Wave | Format::Cut | Format::Daw) {
                ui.horizontal_wrapped(|ui| {
                    ComboBox::from_id_salt("wav_rate")
                        .selected_text(format!("{} Hz", self.wav.rate))
                        .show_ui(ui, |ui| {
                            for rate in [44_100, 48_000, 96_000] {
                                ui.selectable_value(&mut self.wav.rate, rate, format!("{rate} Hz"));
                            }
                        });
                    ComboBox::from_id_salt("wav_bits")
                        .selected_text(if self.wav.bits == 32 {
                            "32-bit float".into()
                        } else {
                            format!("{} bit", self.wav.bits)
                        })
                        .show_ui(ui, |ui| {
                            for (bits, title) in
                                [(16, "16 bit"), (24, "24 bit"), (32, "32-bit float")]
                            {
                                ui.selectable_value(&mut self.wav.bits, bits, title);
                            }
                        });
                    if self.format != Format::Cut {
                    ui.add(
                        DragValue::new(&mut self.wav.tail_seconds)
                            .range(0.0..=10.0)
                            .speed(0.1)
                            .prefix("Tail ")
                            .suffix(" s"),
                    )
                    .on_hover_text("Allow release envelopes and effects to ring out.");
                    }

                });
                if self.format == Format::Cut {
                    let range = if self.pad { app.pad_range(app.selected_pad) } else { app.selection() };
                    small(ui, range.map_or_else(|| "Select a pad or waveform range first.".into(),
                        |(start, end)| format!("Source range · {:.3} seconds", end - start)));
                } else {
                    let seconds = 240.0 * bars as f64 / f64::from(app.project.studio.bpm) + f64::from(self.wav.tail_seconds);
                    small(ui, format!("{bars} bars · {seconds:.2} seconds with tail"));
                }
            } else {
                small(
                    ui,
                    "MIDI contains trigger notes and tempo. Use a DAW pack for the audio.",
                );
            }
            if matches!(self.format, Format::Midi | Format::Daw) && app.project.studio.uses_transcription() {
                midi_mode(ui, &mut self.midi);
            }
            let has_notes = app.project.studio.has_pattern() || (!self.pattern_only && has_song);
            let has_audio = has_notes || app.project.studio.active_loop().is_some();
            let ready = app.busy.is_none() && (1..=crate::model::MAX_SONG_BARS).contains(&bars);
            ui.horizontal(|ui| {
                match self.format {
                    Format::Cut if primary(ui, "Choose WAV file…", app.busy.is_none() &&
                        if self.pad { app.pad_range(app.selected_pad).is_some() } else { app.selection().is_some() }).clicked() => {
                        app.export_cut(self.pad, self.processed, self.wav);
                        close = app.busy.is_some();
                    }
                    Format::Wave
                        if primary(ui, "Choose WAV file…", ready && has_audio).clicked() =>
                    {
                        app.export_configured("wav", self.pattern_only, self.wav);
                        close = app.busy.is_some();
                    }
                    Format::Midi
                        if primary(ui, "Choose MIDI file…", ready && has_notes).clicked() =>
                    {
                        app.export_configured_mode("mid", self.pattern_only, self.wav, self.midi);
                        close = app.busy.is_some();
                    }
                    Format::Daw
                        if primary(
                            ui,
                            "Choose destinations…",
                            ready && (has_audio || app.media.root().is_some()),
                        )
                        .clicked() =>
                    {
                        app.daw_pattern_only = self.pattern_only;
                        app.daw_options.wav = self.wav;
                        app.daw_options.midi_mode = self.midi;
                        app.daw_dialog = true;
                        close = true;
                    }
                    _ => {}
                }
                close |= action(ui, "Close", true).clicked();
            });
        });
        close || modal.should_close()
    }

    #[cfg(windows)]
    fn drag_controls(&mut self, app: &mut App, ui: &mut eframe::egui::Ui) {
        use crate::platform::drag_export::{self, Export};
        use eframe::egui::{Button, PointerButton, Sense};
        small(ui, "Prepare a WAV, then drag it into the DAW. Pattern renders include a one-second tail.");
        let ready = app.busy.is_none() && !app.recording && app.input_recording.is_none();
        let wav = WavOptions { tail_seconds: 1.0, ..WavOptions::default() };
        ui.horizontal_wrapped(|ui| {
            let selection = app.selection();
            if action(ui, "Prepare raw cut", ready && selection.is_some()).clicked()
                && let Some(range) = selection
            {
                app.prepare_drag_export(Export::Selection { source: app.media.active_source(), range }, wav);
            }
            if action(ui, "Prepare processed pad", ready && app.pad_range(app.selected_pad).is_some()).clicked() {
                app.prepare_drag_export(Export::Pad(app.selected_pad), wav);
            }
            if action(ui, "Prepare pattern", ready && (app.project.studio.has_pattern() || app.project.studio.active_loop().is_some())).clicked() {
                app.prepare_drag_export(Export::Pattern, wav);
            }
        });
        if let Some(path) = app.drag_export.clone() {
            ui.label(path.file_name().unwrap_or_default().to_string_lossy());
            ui.horizontal_wrapped(|ui| {
                let drag = ui.add_enabled(app.busy.is_none() && !self.delete_drag,
                    Button::new("Drag WAV into DAW").sense(Sense::drag()));
                if drag.drag_started_by(PointerButton::Primary) {
                    match drag_export::drag(&path) {
                        Ok(true) => app.status = Some(("WAV dropped.".into(), false)),
                        Ok(false) => (),
                        Err(error) => app.status = Some((format!("Could not drag the WAV: {error}"), true)),
                    }
                    ui.ctx().stop_dragging();
                    ui.ctx().request_repaint();
                }
                if action(ui, "Reveal in folder", true).clicked()
                    && let Err(error) = drag_export::reveal(&path)
                {
                    app.status = Some((format!("Could not reveal the WAV: {error}"), true));
                }
                if action(ui, "Delete export…", !self.delete_drag && app.busy.is_none()).clicked() {
                    self.delete_drag = true;
                }
            });
            if self.delete_drag {
                small(ui, "Delete this file only if your DAW project no longer uses it.");
                ui.horizontal_wrapped(|ui| {
                    if action(ui, "Delete this WAV", true).clicked() {
                        app.delete_drag_export();
                        self.delete_drag = false;
                    }
                    if action(ui, "Keep file", true).clicked() {
                        self.delete_drag = false;
                    }
                });
            }
        }
    }
}
