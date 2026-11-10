use crate::{
    app::{App, Page, Pool, StudioView, actions::Action},
    audio::{Command, PadMode},
    midi::{Route, Target},
    model::{
        DRUMS, GENRES, KeyChop, MAX_NAMED_SAMPLES, MAX_SAMPLE_NAME,
        MIN_LOOP_SECONDS, PAD_LABELS, PIANO_HIGH, PIANO_LOW, Quantize, ReleaseInfo, STEPS,
        SampleControls, Source, Studio, Verdict, piano_note,
    },
    waveform::{Snap, WaveformView},
};
use eframe::egui::{
    self, Align, Align2, Button, Color32, ComboBox, Context, CornerRadius, DragValue, FontId,
    Frame, Id, Label, Layout, Margin, Mesh, Painter, Pos2, Rect, Response, RichText, ScrollArea,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

pub(crate) mod arrangement;
pub(crate) mod drum_templates;
pub(crate) mod variation;
pub(crate) mod snapshots;
mod beat_map;
mod extraction;
mod groove;
mod search;
mod transcription;
mod layers;
pub(crate) mod export;
pub(crate) mod piano_roll;
pub(crate) mod preferences;
pub(crate) mod sequencer;
pub(crate) mod sample;
mod step_grid;

const BG: Color32 = Color32::from_rgb(13, 12, 11);
const PANEL: Color32 = Color32::from_rgb(19, 17, 16);
const INSET: Color32 = Color32::from_rgb(27, 24, 22);
const EDGE: Color32 = Color32::from_rgb(52, 46, 42);
const TEXT: Color32 = Color32::from_rgb(243, 240, 237);
const DIM: Color32 = Color32::from_rgb(162, 153, 146);
const FAINT: Color32 = Color32::from_rgb(153, 143, 136);
const GREEN: Color32 = Color32::from_rgb(0, 195, 133);
const MINT: Color32 = Color32::from_rgb(123, 231, 187);
const GREEN_BG: Color32 = Color32::from_rgb(10, 49, 35);
const GOLD: Color32 = Color32::from_rgb(234, 190, 92);
const PAD_COLORS: [(&str, Color32); 5] = [
    ("Mint", MINT),
    ("Amber", GOLD),
    ("Blue", Color32::from_rgb(117, 197, 238)),
    ("Violet", Color32::from_rgb(185, 153, 252)),
    ("Rose", Color32::from_rgb(247, 154, 169)),
];

#[derive(Clone)]
struct CutDrag {
    name: String,
    source: usize,
    range: (f64, f64),
}

const BLUE: Color32 = Color32::from_rgb(117, 197, 238);

pub fn icon() -> egui::IconData {
    let size = 64;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let distance = ((x as f32 - 31.5).powi(2) + (y as f32 - 31.5).powi(2)).sqrt();
            let color = if distance > 30.0 {
                Color32::TRANSPARENT
            } else if distance < 2.0 {
                BG
            } else if distance < 11.0 {
                GREEN
            } else if (distance % 4.0) < 1.0 {
                EDGE
            } else {
                INSET
            };
            rgba.extend(color.to_array());
        }
    }
    egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}

pub fn style(ctx: &Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(12, 11, 10);
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = GREEN_BG;
    visuals.selection.stroke = Stroke::new(1.0_f32, GREEN);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.bg_fill = INSET;
        widget.weak_bg_fill = INSET;
        widget.bg_stroke = Stroke::new(1.0_f32, EDGE);
        widget.fg_stroke = Stroke::new(1.0_f32, TEXT);
        widget.corner_radius = CornerRadius::same(6);
    }
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(42, 36, 33);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, FAINT);
    visuals.widgets.active.weak_bg_fill = GREEN_BG;
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.interact_size.y = 28.0;
    });
}

fn label(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text).size(size).color(color)
}

fn small(ui: &mut Ui, text: impl Into<String>) {
    ui.label(label(text, 12.0, DIM));
}

fn focus_outline(ui: &Ui, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(response.rect, 5.0, Stroke::new(2.0_f32, MINT), StrokeKind::Inside);
    }
}

fn named_number(response: Response, name: &str, value: f64) -> Response {
    response.widget_info(|| WidgetInfo {
        label: Some(name.to_owned()),
        ..WidgetInfo::drag_value(response.enabled(), value)
    });
    response
}

fn action(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let response = ui.add_enabled(enabled, Button::new(label(text, 13.0, TEXT)));
    focus_outline(ui, &response);
    response
}

fn command_button(ui: &mut Ui, app: &App, command: Action, text: &str) -> Response {
    let reason = command.unavailable(app);
    let response = action(ui, text, reason.is_none());
    if let Some(reason) = reason {
        response.on_disabled_hover_text(reason)
    } else if !command.shortcut_text().is_empty() {
        response.on_hover_text(command.shortcut_text())
    } else {
        response
    }
}

fn primary(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let response = ui.add_enabled(
        enabled,
        Button::new(label(text, 14.0, BG).strong())
            .fill(GREEN)
            .stroke(Stroke::NONE),
    );
    focus_outline(ui, &response);
    response
}

fn pill(ui: &mut Ui, text: &str, active: bool) -> Response {
    let response = ui.add(
        Button::new(label(text, 12.0, if active { MINT } else { TEXT }))
            .min_size(vec2(0.0, 26.0))
            .corner_radius(12)
            .fill(if active { GREEN_BG } else { Color32::from_black_alpha(105) })
            .stroke(Stroke::new(1.0_f32, if active { GREEN } else { EDGE })),
    );
    response.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, response.enabled(), active, text));
    focus_outline(ui, &response);
    response
}

pub(crate) fn responsive_modal<R>(
    ctx: &Context, id: &'static str, preferred_width: f32, contents: impl FnOnce(&mut Ui) -> R,
) -> egui::ModalResponse<R> {
    let screen = ctx.screen_rect();
    let width = preferred_width.min(screen.width() - 38.0).max(200.0);
    egui::Modal::new(Id::new(id)).show(ctx, |ui| {
        ui.set_width(width);
        ScrollArea::vertical()
            .id_salt("dialog_contents")
            .max_height((screen.height() - 42.0).max(140.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_width((width - 10.0).max(190.0));
                contents(ui)
            }).inner
    })
}

fn panel() -> Frame {
    Frame::new()
        .fill(Color32::from_black_alpha(54))
        .stroke(Stroke::new(1.0_f32, EDGE))
        .corner_radius(8)
        .inner_margin(Margin::same(12))
}

pub fn draw(app: &mut App, ctx: &Context) {
    if app.page != Page::Studio || app.studio_view != StudioView::Sequence {
        app.sequence_tool = None;
    }
    let had_import_dialog = app.youtube_url.is_some();
    egui::TopBottomPanel::top("navigation")
        .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(14, 8)))
        .show(ctx, |ui| {
            header(app, ui);
            ui.separator();
            studio_transport(app, ui);
            if app.audition.preview.is_some() {
                ui.separator();
                groove::audition(app, ui);
            }
            if app.page == Page::Studio {
                ui.separator();
                studio_header(app, ui);
            }
        });
    if app.busy.is_some()
        || app.status.is_some()
        || app.audio_error.is_some()
        || app.analysis_job().is_some()
        || app.library_recovery.is_some()
        || app.crate_save_error().is_some()
    {
        egui::TopBottomPanel::bottom("status")
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(14, 6)))
            .show(ctx, |ui| notice(app, ui));
    }
    egui::CentralPanel::default()
        .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(14, 8)))
        .show(ctx, |ui| {
            if app.page == Page::Studio {
                studio(app, ui);
                return;
            }
            if !app.library.interface.reduce_motion && !app.library.interface.high_contrast { backdrop(ui); }
            ScrollArea::vertical()
                .id_salt(match app.page {
                    Page::Dig => "dig",
                    Page::Queue => "queue",
                    Page::Search => "search",
                    Page::Crate => "crate",
                    Page::Studio => unreachable!(),
                })
                .auto_shrink([false, false])
                .show(ui, |ui| match app.page {
                    Page::Dig => dig(app, ui),
                    Page::Queue => queue(app, ui),
                    Page::Search => search::show(app, ui),
                    Page::Crate => {
                        ui.label(label("Your crate", 26.0, TEXT).strong());
                        small(ui, "Saved records, loop points, tempo and flip settings.");
                        crate_panel(app, ui);
                    }
                    Page::Studio => unreachable!(),
                });
        });
    if app.show_commands {
        preferences::show(app, ctx);
    } else if app.youtube_url.is_some() {
        youtube_dialog(app, ctx, !had_import_dialog);
    } else if app.daw_dialog {
        daw_dialog(app, ctx);
    } else if app.midi_dialog {
        midi_dialog(app, ctx);
    } else if let Some(mut browser) = app.drum_templates.take() {
        if !browser.show(app, ctx) {
            app.drum_templates = Some(browser);
        }
    } else if let Some(mut dialog) = app.variations.take() {
        if dialog.show(app, ctx) {
            app.variations = Some(dialog);
        }
    } else if let Some(mut dialog) = app.snapshot_dialog.take() {
        if dialog.show(app, ctx) {
            app.snapshot_dialog = Some(dialog);
        }
    } else if let Some(mut dialog) = app.export_dialog.take() {
        if !dialog.show(app, ctx) {
            app.export_dialog = Some(dialog);
        }
    } else if let Some(mut editor) = app.transcription.editor.take() {
        if transcription::show(app, ctx, &mut editor) {
            app.close_transcription_editor(&mut editor);
        } else {
            app.transcription.editor = Some(editor);
        }
    } else if let Some(mut roll) = app.piano_roll.take()
        && !roll.show(app, ctx)
    {
        app.piano_roll = Some(roll);
    } else if app.page == Page::Studio && app.studio_view == StudioView::Sequence {
        sequencer::dialog(app, ctx);
    }
}

fn midi_dialog(app: &mut App, ctx: &Context) {
    let mut close = false;
    let mut mapping_file = None;
    let mut changed = false;
    if app.midi_selected.is_none() {
        app.midi_selected = app.midi_devices.first().map(|device| device.id.clone());
    }
    let modal = responsive_modal(ctx, "midi_setup", 580.0, |ui| {
        ui.label(label("MIDI devices", 21.0, TEXT).strong());
        ui.collapsing("Sequencer output", |ui| {
            let ports = crate::midi_out::devices();
            let current = app.library.midi.output.as_ref().and_then(|id| {
                ports.iter().find(|(port, _)| port == id).map(|(_, name)| name.as_str())
            });
            ui.horizontal_wrapped(|ui| {
                let mut output = app.library.midi.output.clone();
                ComboBox::from_id_salt("midi_output").selected_text(current.unwrap_or("Choose output"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut output, None, "Disabled");
                        for (id, name) in &ports {
                            ui.selectable_value(&mut output, Some(id.clone()), name);
                        }
                    });
                if app.library.midi.output != output {
                    app.library.midi.output = output;
                    app.midi_output_status = Some("Apply to change the active MIDI connection.".into());
                }
                if ui.checkbox(&mut app.library.midi.send_clock, "Send MIDI clock").changed() {
                    app.midi_output_status = Some("Apply to update clock sending.".into());
                }
                if action(ui, "Apply MIDI output", true).clicked() {
                    app.connect_midi_output();
                    changed = true;
                }
            });
            small(ui, "Pads use channel 1, keys channel 2, and drums channel 10. Clock is suppressed when following an external clock.");
            if let Some(status) = &app.midi_output_status { small(ui, status); }
        });
        ui.separator();
        ui.label("MIDI inputs");
        if let Some(error) = &app.midi_error {
            small(ui, error);
        } else if app.midi_devices.is_empty() {
            small(
                ui,
                "No MIDI inputs detected. Connect your controller and its port will appear here.",
            );
        }
        let mut selected = app.midi_selected.clone();
        ScrollArea::vertical().max_height(108.0).show(ui, |ui| {
            for device in &app.midi_devices {
                let status = if device.connection.is_some() {
                    "Connected"
                } else if device.error.is_some() {
                    "Unavailable"
                } else {
                    "Off"
                };
                ui.push_id(&device.id, |ui| {
                    ui.selectable_value(
                        &mut selected,
                        Some(device.id.clone()),
                        format!("{}  ·  {status}", device.name),
                    )
                    .on_hover_text(&device.id);
                });
            }
        });
        if selected != app.midi_selected {
            app.midi_selected = selected;
            app.midi_learn = None;
        }
        if let Some(device) = app
            .midi_devices
            .iter()
            .find(|device| Some(&device.id) == app.midi_selected.as_ref())
            .cloned()
        {
            ui.separator();
            small(ui, device.profile.label());
            if let Some(error) = &device.error {
                small(ui, format!("Could not open this port. {error}"));
            }
            let settings = app.library.midi.ports.entry(device.id.clone()).or_default();
            ui.horizontal_wrapped(|ui| {
                let mut enabled = !settings.disabled;
                if ui.checkbox(&mut enabled, "Receive MIDI").changed() {
                    settings.disabled = !enabled;
                    changed = true;
                }
                let route = settings.route;
                ComboBox::from_id_salt("midi_route")
                    .selected_text(route.label())
                    .show_ui(ui, |ui| {
                        for route in Route::ALL {
                            ui.selectable_value(&mut settings.route, route, route.label());
                        }
                    });
                changed |= settings.route != route;
                let mut clock = app.library.midi.clock.as_ref() == Some(&device.id);
                if ui
                    .add_enabled(
                        enabled,
                        egui::Checkbox::new(&mut clock, "Follow clock from this input"),
                    )
                    .changed()
                {
                    app.library.midi.clock = clock.then(|| device.id.clone());
                    changed = true;
                }
            });
            ui.collapsing("Input response and mappings", |ui| {
                ui.horizontal_wrapped(|ui| {
                    let old = (settings.channel, settings.velocity, settings.cc_mode);
                    ComboBox::from_id_salt("midi_channel").selected_text(settings.channel.map_or_else(|| "All channels".into(), |ch| format!("Channel {}", ch + 1)))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut settings.channel, None, "All channels");
                            for ch in 0..16 { ui.selectable_value(&mut settings.channel, Some(ch), format!("Channel {}", ch + 1)); }
                        });
                    ComboBox::from_id_salt("velocity_curve").selected_text(settings.velocity.label()).show_ui(ui, |ui| {
                        for curve in crate::midi::VelocityCurve::ALL {
                            ui.selectable_value(&mut settings.velocity, curve, curve.label());
                        }
                    });
                    ComboBox::from_id_salt("encoder_mode").selected_text(settings.cc_mode.label()).show_ui(ui, |ui| {
                        for mode in crate::midi::CcMode::ALL { ui.selectable_value(&mut settings.cc_mode, mode, mode.label()); }
                    });
                    changed |= old != (settings.channel, settings.velocity, settings.cc_mode);
                    changed |= ui.checkbox(&mut settings.soft_takeover, "Soft takeover").changed();
                    if action(ui, "Import mapping…", app.busy.is_none()).clicked() { mapping_file = Some((device.id.clone(), false)); }
                    if action(ui, "Export mapping…", app.busy.is_none()).clicked() { mapping_file = Some((device.id.clone(), true)); }
                });
            });
            if app.library.midi.clock.as_ref() == Some(&device.id) {
                small(
                    ui,
                    app.midi_clock_bpm.map_or_else(
                        || "Waiting for MIDI clock".into(),
                        |bpm| format!("External clock · {bpm:.1} BPM"),
                    ),
                );
            }
            if let Some((connection, signal, value)) = app.midi_last
                && device.connection == Some(connection)
            {
                small(
                    ui,
                    format!("Last input  {} · Value {value}", signal.label()),
                );
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ComboBox::from_id_salt("midi_learn_target")
                    .selected_text(app.midi_target.label())
                    .show_ui(ui, |ui| {
                        for target in Target::FIXED
                            .into_iter()
                            .chain((0..STEPS).map(Target::Pad))
                            .chain((0..DRUMS.len()).map(Target::Drum))
                        {
                            ui.selectable_value(&mut app.midi_target, target, target.label());
                        }
                    });
                if let Some(target) = app.midi_learn {
                    small(
                        ui,
                        format!("Move or press a control for {}", target.label()),
                    );
                    if action(ui, "Cancel", true).clicked() {
                        app.midi_learn = None;
                    }
                } else if action(
                    ui,
                    "MIDI Learn",
                    device.connection.is_some() && !settings.disabled,
                )
                .clicked()
                {
                    app.midi_learn = Some(app.midi_target);
                }
            });
            let mut remove = None;
            ScrollArea::vertical()
                .id_salt("midi_bindings")
                .max_height(105.0)
                .show(ui, |ui| {
                    for (index, (signal, target)) in settings.bindings.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.horizontal(|ui| {
                                small(ui, format!("{}  →  {}", signal.label(), target.label()));
                                if action(ui, "Remove", true).clicked() {
                                    remove = Some(index);
                                }
                            });
                        });
                    }
                });
            if let Some(index) = remove {
                settings.bindings.remove(index);
                changed = true;
            }
        }
        if let Some((job, _)) = &app.busy {
            ui.horizontal_wrapped(|ui| {
                small(ui, *job);
                job_controls(app, ui);
            });
        } else if let Some((message, error)) = &app.status {
            ui.label(label(message, 12.0, if *error { GOLD } else { TEXT }));
            if app.can_retry_job() && action(ui, "Retry last operation", true).clicked() {
                app.retry_background_job();
            }
        }
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Release all notes", true).clicked() {
                app.midi_panic();
            }
            close = action(ui, "Close", true).clicked();
        });
    });
    if changed {
        app.midi_settings_changed();
    }
    if let Some((id, export)) = mapping_file { app.midi_mapping_file(id, export); }
    if close || modal.should_close() {
        app.midi_dialog = false;
        app.midi_learn = None;
    }
}

fn daw_dialog(app: &mut App, ctx: &Context) {
    let ready = app.busy.is_none();
    let mut close = false;
    let has_melody = app.studio.uses_transcription();
    let modal = responsive_modal(ctx, "daw_export", 510.0, |ui| {
        ui.label(label("Export DAW pack", 21.0, TEXT).strong());
        ui.add_enabled_ui(ready, |ui| ui.collapsing("Pack contents", |ui| {
            let options = &mut app.daw_options;
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut options.mix, "Stereo mix");
                ui.checkbox(&mut options.sample_bus, "Sample bus");
                ui.checkbox(&mut options.drums_bus, "Drums bus");
                ui.checkbox(&mut options.individual_drums, "Individual drums");
                ui.checkbox(&mut options.separate_chops, "Separate pads and keys");
                ui.checkbox(&mut options.midi, "MIDI");
                ui.checkbox(&mut options.raw, "Raw cuts");
                ui.checkbox(&mut options.processed, "Processed cuts");
                ui.checkbox(&mut options.dry_stems, "Bypass effects on stems");
            });
            if options.midi && has_melody { export::midi_mode(ui, &mut options.midi_mode); }
        }));
        small(ui, "Each checked folder gets the same WAV and MIDI pack.");
        if action(ui, "Add destination folders…", ready).clicked() {
            app.add_daw_targets();
        }
        let mut remove = None;
        let mut changed = false;
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                for (index, target) in app.library.daw_targets.iter_mut().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.horizontal(|ui| {
                            let include = ui
                                .add_enabled(
                                    ready,
                                    egui::Checkbox::without_text(&mut target.enabled),
                                )
                                .on_hover_text("Include this folder in the export");
                            include.widget_info(|| WidgetInfo::selected(
                                WidgetType::Checkbox, ready, target.enabled, format!("Include {}", target.path.display()),
                            ));
                            changed |= include.changed();
                            if action(ui, "Remove", ready).clicked() {
                                remove = Some(index);
                            }
                            let path = target.path.display().to_string();
                            ui.add(Label::new(&path).truncate()).on_hover_text(path);
                        });
                    });
                }
            });
        if let Some(index) = remove {
            app.library.daw_targets.remove(index);
            changed = true;
        }
        if changed {
            app.persist_library();
        }
        if let Some(result) = &app.daw_delivery {
            for path in &result.written {
                ui.horizontal(|ui| {
                    #[cfg(windows)]
                    if action(ui, "Open folder", true).clicked() {
                        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
                    }
                    ui.add(Label::new(path.display().to_string()).truncate());
                });
            }
            for failure in &result.failed {
                ui.label(label(failure, 12.0, Color32::from_rgb(249, 144, 124)));
            }
        } else if let Some((message, error)) = &app.status {
            ui.label(label(message, 12.0, if *error { GOLD } else { TEXT }));
        }
        ui.horizontal(|ui| {
            let count = app
                .library
                .daw_targets
                .iter()
                .filter(|target| target.enabled)
                .count();
            let export = if count > 1 {
                format!("Export to {count} folders")
            } else {
                "Export".into()
            };
            if primary(ui, &export, ready && count > 0).clicked() {
                app.export_daw();
            }
            if ready {
                if app.can_retry_job() && action(ui, "Retry last operation", true).clicked() {
                    app.retry_background_job();
                }
                close = action(ui, "Close", true).clicked();
            } else {
                if !app.library.interface.reduce_motion { ui.spinner(); }
                job_controls(app, ui);
            }
        });
    });
    if ready && (close || modal.should_close()) {
        app.daw_dialog = false;
    }
}

fn youtube_dialog(app: &mut App, ctx: &Context, first_frame: bool) {
    let busy = app.busy.is_some();
    let mut submit = false;
    let mut cancel = false;
    let modal = responsive_modal(ctx, "youtube_import", 460.0, |ui| {
        ui.label(label("Import from YouTube", 21.0, TEXT).strong());
        small(
            ui,
            "Paste a video link. Timestamped links open at the selected time.",
        );
        let input = app
            .youtube_url
            .as_mut()
            .expect("YouTube import dialog open");
        let response = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(input)
                .hint_text("https://www.youtube.com/watch?v=…")
                .desired_width(f32::INFINITY)
                .char_limit(2048),
        );
        if first_frame {
            response.request_focus();
        }
        let ready = !busy && !input.trim().is_empty();
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                job_controls(app, ui);
            });
        } else if let Some((message, error)) = &app.status {
            ui.label(label(message, 12.0, if *error { GOLD } else { TEXT }));
        }
        ui.horizontal(|ui| {
            submit = primary(ui, "Import audio", ready).clicked()
                || (ready
                    && response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter)));
            cancel = action(ui, "Cancel", !busy).clicked();
        });
    });
    if submit {
        let input = app.youtube_url.clone().unwrap_or_default();
        app.import_youtube(&input);
    } else if !busy && (cancel || modal.should_close()) {
        app.youtube_url = None;
        app.status = None;
    }
}

fn backdrop(ui: &Ui) {
    let rect = ui.max_rect();
    let columns = 32;
    let rows = 23;
    let mut mesh = Mesh::default();
    for y in 0..=rows {
        let fy = y as f32 / rows as f32;
        for x in 0..=columns {
            let fx = x as f32 / columns as f32;
            let glow = |cx: f32, cy: f32, sx: f32, sy: f32| {
                (-(((fx - cx) / sx).powi(2) + ((fy - cy) / sy).powi(2))).exp() * (1.0 - fy)
            };
            let green = glow(0.20, 0.11, 0.37, 0.40);
            let purple = glow(0.86, 0.06, 0.27, 0.33);
            mesh.colored_vertex(
                pos2(rect.left() + fx * rect.width(), rect.top() + fy * 540.0),
                Color32::from_rgb(
                    (13.0 + 28.0 * purple) as u8,
                    (12.0 + 48.0 * green + 20.0 * purple) as u8,
                    (11.0 + 32.0 * green + 44.0 * purple) as u8,
                ),
            );
            if x < columns && y < rows {
                let i = y * (columns + 1) + x;
                mesh.add_triangle(i, i + 1, i + columns + 1);
                mesh.add_triangle(i + 1, i + columns + 2, i + columns + 1);
            }
        }
    }
    ui.painter().add(Shape::mesh(mesh));
}

fn vinyl(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    for halo in (1..=10).rev() {
        painter.circle_filled(
            center,
            radius * (1.0 + halo as f32 * 0.031),
            Color32::from_rgba_unmultiplied(color.r() / 3, color.g() / 3, color.b() / 3, 9),
        );
    }
    painter.circle_filled(center, radius, Color32::from_rgb(24, 21, 20));
    for ring in [0.91, 0.78, 0.64, 0.49] {
        painter.circle_stroke(
            center,
            radius * ring,
            Stroke::new(1.0_f32, Color32::from_rgb(48, 42, 39)),
        );
    }
    painter.circle_filled(center, radius * 0.34, color);
    painter.circle_filled(center, radius * 0.06, BG);
    painter.circle_filled(
        center + vec2(radius * 0.11, -radius * 0.13),
        radius * 0.032,
        BG,
    );
}

fn header(app: &mut App, ui: &mut Ui) {
    let wide = ui.available_width() >= 760.0;
    let navigation = |ui: &mut Ui, app: &mut App| {
        if wide {
            let (rect, _) = ui.allocate_exact_size(vec2(123.0, 38.0), Sense::hover());
        let painter = ui.painter();
        vinyl(
            painter,
            pos2(rect.left() + 13.0, rect.center().y),
            12.0,
            GREEN,
        );
        painter.text(
            pos2(rect.left() + 34.0, rect.top() + 4.0),
            Align2::LEFT_TOP,
            "L O S T",
            FontId::monospace(11.0),
            TEXT,
        );
        painter.text(
            pos2(rect.left() + 34.0, rect.top() + 19.0),
            Align2::LEFT_TOP,
            "S A M P L E S",
            FontId::monospace(11.0),
            GREEN,
        );
        ui.add_space(8.0);
        }
        for (page, title) in [
            (Page::Dig, "Dig"),
            (Page::Queue, "Queue"),
            (Page::Studio, "Studio"),
            (Page::Search, "Search"),
            (Page::Crate, "Crate"),
        ] {
            if pill(ui, title, app.page == page).clicked() {
                Action::Page(page).run(app);
            }
        }
    };
    let controls = |ui: &mut Ui, app: &mut App| {
            if action(ui, "Help", true).on_hover_text("Commands and settings · Ctrl+K").clicked() {
                Action::Commands.run(app);
            }
            if primary(ui, "+ Import audio", Action::Import.unavailable(app).is_none())
                .on_hover_text("Import or drop WAV, MP3, FLAC, Ogg or M4A")
                .clicked()
            {
                Action::Import.run(app);
            }
            if command_button(ui, app, Action::ImportYouTube, "YouTube URL").clicked() {
                Action::ImportYouTube.run(app);
            }
            ui.add_enabled_ui(Action::Open.unavailable(app).is_none(), |ui| {
                ui.menu_button("Project", |ui| {
                    for command in [Action::Open, Action::Save, Action::SaveAs, Action::NewFromTemplate, Action::SaveTemplate, Action::Recover] {
                        if command_button(ui, app, command, command.title()).clicked() {
                            ui.close_menu();
                            command.run(app);
                        }
                    }
                    ui.separator();
                    ui.label("Recent projects");
                    if app.library.recent_projects.is_empty() {
                        small(ui, "No recent projects");
                    }
                    let mut open = None;
                    for path in &app.library.recent_projects {
                        let title = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                        if ui.button(title).on_hover_text(path.display().to_string()).clicked() {
                            open = Some(path.clone());
                        }
                    }
                    if let Some(path) = open {
                        ui.close_menu();
                        app.open(path);
                    }
                });
            });
        };
    if wide {
        ui.horizontal(|ui| {
            navigation(ui, app);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| controls(ui, app));
        });
    } else {
        ui.horizontal_wrapped(|ui| navigation(ui, app));
        ui.horizontal_wrapped(|ui| controls(ui, app));
    }
}

pub(crate) fn job_controls(app: &App, ui: &mut Ui) {
    if let Some(control) = &app.job_control {
        let state = control.state();
        if let Some(stage) = state.stage {
            small(ui, stage);
        }
        if let Some(progress) = state.progress {
            ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).desired_width(115.0).show_percentage());
        }
        if !state.publishing && action(ui, if state.cancelled { "Cancelling…" } else { "Cancel job" }, !state.cancelled).clicked() {
            control.cancel();
        }
    }
}

fn notice(app: &mut App, ui: &mut Ui) {
    if let Some(recovery) = &app.library_recovery {
        let restore = ui
            .horizontal_wrapped(|ui| {
                ui.label(label(&recovery.message, 13.0, GOLD));
                recovery.has_backup()
                    && ui.add_enabled(
                        !app.restoring_library(),
                        egui::Button::new(if app.restoring_library() {
                            "Restoring crate…"
                        } else {
                            "Restore crate backup"
                        }).small(),
                    ).clicked()
            })
            .inner;
        if restore {
            app.restore_library();
        }
    }
    if let Some(error) = app.crate_save_error() {
        let retry = ui.horizontal_wrapped(|ui| {
            ui.label(label(format!("Could not save crate. {error}"), 13.0, GOLD));
            ui.small_button("Retry crate save").clicked()
        }).inner;
        if retry {
            app.retry_crate_save();
        }
    }
    if let Some((job, state, queued)) = app.analysis_job() {
        ui.horizontal_wrapped(|ui| {
            if !app.library.interface.reduce_motion { ui.spinner(); }
            small(ui, state.stage.unwrap_or(job));
            if queued > 0 { small(ui, format!("{queued} queued")); }
            if let Some(progress) = state.progress {
                ui.add(egui::ProgressBar::new(progress.clamp(0.0, 1.0)).desired_width(115.0).show_percentage());
            }
            if action(ui, if state.cancelled { "Cancelling…" } else { "Cancel analysis" }, !state.cancelled).clicked() {
                app.cancel_analysis();
            }
        });
    }
    if let Some((job, _)) = app.busy.as_ref() {
        ui.horizontal_wrapped(|ui| {
            if !app.library.interface.reduce_motion { ui.spinner(); }
            small(ui, *job);
            job_controls(app, ui);
        });
    } else if let Some((message, error)) = &app.status {
        let text = message.clone();
        let color = if *error {
            Color32::from_rgb(249, 144, 124)
        } else {
            MINT
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(label(text, 13.0, color));
            if app.can_retry_job() && ui.small_button("Retry last operation").clicked() {
                app.retry_background_job();
            }
            if ui.small_button("Dismiss").clicked() {
                app.dismiss_job_status();
            }
        });
    }
    if let Some(error) = &app.audio_error {
        let message = format!("Audio output unavailable: {error}");
        ui.horizontal_wrapped(|ui| {
            ui.label(label(message, 12.0, GOLD));
            if ui.small_button("Retry audio").clicked() {
                app.retry_audio();
            }
        });
    }
}

fn dig(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        let current = match app.pool {
            Pool::YouTube => "YouTube records",
            Pool::Demo => "Demo records",
            Pool::Local => "Local folder",
            Pool::Commons => "Wikimedia Commons",
        };
        ComboBox::from_id_salt("dig_source").selected_text(current).width(170.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.pool, Pool::YouTube, "YouTube records");
            ui.selectable_value(&mut app.pool, Pool::Demo, "Demo records");
            ui.selectable_value(&mut app.pool, Pool::Commons, "Wikimedia Commons");
            ui.selectable_value(&mut app.pool, Pool::Local, "Local folder");
        });
        if app.pool == Pool::Local && action(ui, "Choose folder…", app.busy.is_none()).clicked() {
            app.pick_folder();
        }
        if app.record.is_none() && action(ui, "Try an offline demo", app.busy.is_none()).clicked() {
            app.pool = Pool::Demo;
            app.genres.fill(false);
            app.page = Page::Studio;
            app.dig();
        }
        ui.checkbox(&mut app.unheard_only, "Unheard only");
        ui.checkbox(&mut app.exclude_saved, "Exclude saved");
        ui.checkbox(&mut app.exclude_skipped, "Exclude skipped");
        if action(ui, "Fill audition queue", app.busy.is_none()).clicked() {
            app.build_queue(None);
        }
    });
    if app.pool == Pool::YouTube {
        ui.collapsing("Source setup", |ui| {
            if let Some(path) = crate::youtube::downloader() {
                small(ui, format!("yt-dlp found at {}", path.display()));
            } else {
                small(ui, "yt-dlp is missing. Put it beside the app or add it to PATH.");
            }
            if std::env::var_os("DISCOGS_TOKEN").is_some() {
                small(ui, "Discogs token is configured.");
            } else {
                small(ui, "Catalog requests may require DISCOGS_TOKEN. Set it in the environment and restart the app.");
            }
            small(ui, "Use a demo or local folder for offline discovery.");
        });
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(7.0, 6.0);
        for (i, genre) in GENRES.into_iter().enumerate() {
            let active = if i == 0 {
                !app.genres.iter().any(|&selected| selected)
            } else {
                app.genres[i]
            };
            if pill(ui, genre, active).clicked() {
                if i == 0 {
                    app.genres.fill(false);
                } else {
                    app.genres[i] = !app.genres[i];
                }
            }
        }
    });
    if app.pool == Pool::YouTube {
        ui.horizontal_wrapped(|ui| {
            small(ui, "Decade");
            ComboBox::from_id_salt("dig_decade")
                .selected_text(
                    app.dig_decade
                        .map_or_else(|| "Any decade".into(), |decade| format!("{decade}s")),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.dig_decade, None, "Any decade");
                    for decade in (1920..=2020).step_by(10) {
                        ui.selectable_value(
                            &mut app.dig_decade,
                            Some(decade),
                            format!("{decade}s"),
                        );
                    }
                })
                .response
                .on_hover_text("Uses years and decade tags in video titles.");
            ui.add_space(8.0);
            small(ui, "Max views");
            let edit = ui
                .add(
                    egui::TextEdit::singleline(&mut app.dig_max_views)
                        .hint_text("No limit")
                        .desired_width(96.0)
                        .char_limit(28),
                )
                .on_hover_text("Enter a number such as 5,000. Leave it blank for no limit.");
            if edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                app.dig();
            }
            if action(
                ui,
                "Reset",
                app.dig_decade.is_some()
                    || !app.dig_max_views.is_empty()
                    || !app.release_year.is_empty()
                    || !app.release_country.is_empty()
                    || !app.release_label.is_empty(),
            )
            .clicked()
            {
                app.dig_decade = None;
                app.dig_max_views.clear();
                app.release_year.clear();
                app.release_country.clear();
                app.release_label.clear();
            }
        });
        ui.collapsing("Discogs release filters", |ui| ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.release_year)
                    .hint_text("Year")
                    .desired_width(64.0)
                    .char_limit(4),
            );
            ui.add(
                egui::TextEdit::singleline(&mut app.release_country)
                    .hint_text("Country · Brazil, US…")
                    .desired_width(145.0)
                    .char_limit(60),
            );
            ui.add(
                egui::TextEdit::singleline(&mut app.release_label)
                    .hint_text("Label · Motown…")
                    .desired_width(145.0)
                    .char_limit(90),
            );
        }));
        ui.menu_button("Filter presets…", |ui| {
            let mut load = None;
            for (index, preset) in app.library.dig_presets.iter().enumerate() {
                if ui.button(&preset.name).clicked() { load = Some(index); }
            }
            if let Some(index) = load {
                app.apply_dig_preset(index);
                ui.close_menu();
            }
            ui.add(egui::TextEdit::singleline(&mut app.preset_name).hint_text("Preset name").char_limit(64));
            if action(ui, "Save current filters", !app.preset_name.trim().is_empty()).clicked() {
                app.save_dig_preset();
                ui.close_menu();
            }
        });
    }
    ui.add_space(5.0);
    let width = ui.available_width();
    let browse = |app: &mut App, ui: &mut Ui| {
        hero(app, ui);
        ui.add_space(6.0);
        transport(app, ui);
        if app.record.is_some() {
            ui.add_space(4.0);
            record_details(app, ui);
        }
        small(ui, format!("{} heard in total", app.library.heard));
    };
    if width < 710.0 {
        browse(app, ui);
        history(app, ui);
    } else {
        let sidebar = (width * 0.285).clamp(225.0, 320.0);
        let main = width - sidebar - 16.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(main, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(main);
                browse(app, ui);
            });
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(vec2(sidebar, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(sidebar);
                history(app, ui);
            });
        });
    }
    ui.add_space(10.0);
    crate_panel(app, ui);
}

fn queue(app: &mut App, ui: &mut Ui) {
    ui.label(label("Audition queue", 26.0, TEXT).strong());
    small(
        ui,
        "Preview from inside the track, then keep it, skip it, or come back later.",
    );
    ui.horizontal_wrapped(|ui| {
        if primary(ui, "Add more tracks", app.busy.is_none()).clicked() {
            app.build_queue(None);
        }
        if action(ui, "Undo decision", app.library.taste.can_undo()).clicked() {
            app.undo_queue();
        }
        ui.checkbox(&mut app.queue_advance, "Advance focus");
        ui.checkbox(&mut app.queue_level_match, "Match preview levels");
        ui.add(DragValue::new(&mut app.queue_preview_seconds).range(3.0..=45.0).prefix("Preview ").suffix(" s"));
        let mut point = app.queue_preview_point * 100.0;
        if ui.add(DragValue::new(&mut point).range(0.0..=100.0).prefix("Start ").suffix("%")).changed() {
            app.queue_preview_point = point / 100.0;
        }
        ui.checkbox(&mut app.queue_similar, "Similar matches only");
    });
    taste_controls(app, ui);
    ui.horizontal_wrapped(|ui| {
        for (verdict, title) in [
            (Verdict::Pending, "To hear"),
            (Verdict::Later, "Later"),
            (Verdict::Keep, "Kept"),
            (Verdict::Skip, "Skipped"),
        ] {
            let count = app
                .library
                .queue
                .iter()
                .filter(|entry| entry.verdict == verdict)
                .count();
            if pill(ui, &format!("{title} · {count}"), app.queue_tab == verdict).clicked() {
                app.queue_tab = verdict;
            }
        }
    });
    if let Some((position, length)) = app.queue_audition() {
        let scrub_id = ui.id().with("queue_scrub");
        let mut cursor = ui.data(|data| data.get_temp::<f64>(scrub_id)).unwrap_or(position);
        let response = ui.add(egui::Slider::new(&mut cursor, 0.0..=length).text("Audition position (s)"));
        if response.dragged() {
            ui.data_mut(|data| data.insert_temp(scrub_id, cursor));
        }
        if response.drag_stopped() || (response.changed() && !response.dragged()) {
            ui.data_mut(|data| data.remove::<f64>(scrub_id));
            app.scrub_queue(cursor);
        }
    }
    let marked = app.library.queue.iter().filter(|entry| entry.verdict == app.queue_tab
        && (!app.queue_similar || entry.similarity.is_some()) && app.queue_marked.contains(&entry.record.source)).count();
    if marked > 0 {
        ui.horizontal_wrapped(|ui| {
            small(ui, format!("{marked} marked"));
            for (label, verdict) in [("Keep marked", Verdict::Keep), ("Skip marked", Verdict::Skip), ("Save for later", Verdict::Later)] {
                if action(ui, label, app.busy.is_none()).clicked() { app.queue_decide_marked(verdict); }
            }
            if action(ui, "Clear marks", true).clicked() { app.queue_marked.clear(); }
        });
    }
    let mut choice = None;
    let mut any = false;
    let ready = app.busy.is_none();
    let focused = app.queue_focused();
    for index in app.queue_indices() {
        let rank = app.queue_ranking(index).cloned();
        let entry = &app.library.queue[index];
        any = true;
        ui.push_id(index, |ui| {
            panel().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let mut marked = app.queue_marked.contains(&entry.record.source);
                    if ui.checkbox(&mut marked, "").on_hover_text("Mark track for a batch decision").changed() {
                        if marked { app.queue_marked.insert(entry.record.source.clone()); }
                        else { app.queue_marked.remove(&entry.record.source); }
                    }
                    if ui.selectable_label(focused == Some(index), label(&entry.record.title, 15.0, TEXT).strong())
                        .on_hover_text("Set keyboard focus. Space previews; Enter opens.").clicked()
                    {
                        choice = Some((index, "focus"));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    small(
                        ui,
                        format!("{} · {}", entry.record.artist, entry.record.genre),
                    );
                    if let Some(views) = entry.views {
                        small(ui, views_label(views));
                    }
                    if entry.auditioned {
                        small(ui, "Heard");
                    }
                    if let Some(score) = entry.similarity {
                        ui.label(label(format!("Sound match {score}/100"), 12.0, DIM))
                            .on_hover_text("Compares energy, spectral balance, and rhythmic features of the candidate passage with your cut.");
                    }
                    if let Some([rhythm, brightness, energy, texture]) = entry.similarity_aspects {
                        small(ui, format!("Rhythm {rhythm}%  ·  Brightness {brightness}%  ·  Energy {energy}%  ·  Texture {texture}%"));
                    }
                    if let Some((start, end)) = entry.match_range {
                        small(ui, format!("{} – {}", time(start, true), time(end, true)));
                    }
                });
                if let Some(release) = &entry.record.release {
                    release_details(ui, release);
                }
                if let Some(rank) = &rank {
                    ui.label(label(&rank.explanation, 12.0, DIM))
                        .on_hover_text(format!("Rank score {:.3}", rank.score));
                }
                ui.horizontal_wrapped(|ui| {
                    let playing = app.queue_playing.as_ref() == Some(&entry.record.source);
                    if action(
                        ui,
                        if playing { "Stop" } else { "Preview" },
                        ready && app.output.is_some(),
                    )
                    .clicked()
                    {
                        choice = Some((index, "preview"));
                    }
                    if action(ui, "Audition with beat", ready && app.output.is_some()).clicked() {
                        choice = Some((index, "beat"));
                    }
                    if primary(ui, "Open in Studio", ready).clicked() {
                        choice = Some((index, "open"));
                    }
                    for (verdict, title, operation) in [
                        (Verdict::Keep, "Keep", "keep"),
                        (Verdict::Skip, "Skip", "skip"),
                        (Verdict::Later, "Later", "later"),
                        (Verdict::Pending, "Requeue", "requeue"),
                    ] {
                        if verdict != entry.verdict && action(ui, title, ready).clicked() {
                            choice = Some((index, operation));
                        }
                    }
                    if app.library.taste.enabled && entry.verdict != Verdict::Skip && ready {
                        ui.menu_button("Skip with reason", |ui| {
                            if ui.button("Doesn’t fit this beat").clicked() {
                                choice = Some((index, "skip_context"));
                                ui.close_menu();
                            }
                            if ui.button("Dislike the sound").clicked() {
                                choice = Some((index, "skip_sound"));
                                ui.close_menu();
                            }
                        });
                    }
                });
            })
        });
    }
    if !any {
        small(
            ui,
            "Nothing in this queue view. Add tracks using the filters on Dig.",
        );
    }
    if let Some((index, operation)) = choice {
        match operation {
            "focus" => app.queue_focus = Some(app.library.queue[index].record.source.clone()),
            "preview" => app.preview_queue(index),
            "beat" => app.audition_queue_with_beat(index),
            "open" => app.open_queue(index),
            "keep" => app.queue_decide(index, Verdict::Keep),
            "skip" => app.queue_decide(index, Verdict::Skip),
            "skip_context" => app.queue_decide_with_reason(index, Verdict::Skip, crate::recommendations::SkipReason::Context),
            "skip_sound" => app.queue_decide_with_reason(index, Verdict::Skip, crate::recommendations::SkipReason::Sound),
            "later" => app.queue_decide(index, Verdict::Later),
            _ => app.queue_decide(index, Verdict::Pending),
        }
    }
}

fn taste_controls(app: &mut App, ui: &mut Ui) {
    ui.collapsing("Taste profile", |ui| {
        let mut enabled = app.library.taste.enabled;
        let mut adventure = app.library.taste.adventure;
        ui.horizontal_wrapped(|ui| {
            let toggled = ui.checkbox(&mut enabled, "Personalize queue").changed();
            small(ui, "Familiar");
            let slider = ui.add_enabled(enabled, egui::Slider::new(&mut adventure, 0..=100).show_value(false));
            small(ui, "Adventurous");
            if toggled || slider.changed() || slider.drag_stopped() {
                app.set_taste_options(enabled, adventure, toggled || slider.drag_stopped() || !slider.dragged());
            }
            if action(ui, "Rebuild ranking", enabled).clicked() { app.invalidate_taste(); }
            if action(ui, "Reset feedback", !app.library.taste.feedback.is_empty()).clicked() { app.reset_taste(); }
        });
        let (kept, skipped) = app.library.taste.counts();
        small(ui, format!("{kept} keeps · {skipped} skips used for ranking"));
        small(ui, app.taste_status());
        small(ui, "Keeps and skips can teach the profile while personalization is on. A skip counts after two seconds of audition. Use Doesn’t fit this beat to keep a skip neutral.");
    });
}

fn release_details(ui: &mut Ui, release: &ReleaseInfo) {
    ui.horizontal_wrapped(|ui| {
        small(
            ui,
            format!(
                "{} · {} · {} · {}",
                release.title, release.year, release.country, release.label
            ),
        );
        if let Ok(id) = release.id.parse::<u64>() {
            ui.hyperlink_to("Discogs ↗", format!("https://www.discogs.com/release/{id}"));
        }
    });
}

fn hero(app: &mut App, ui: &mut Ui) {
    let width = ui.available_width();
    let height = (width * 0.42).clamp(215.0, 350.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect(
        rect,
        8.0,
        Color32::BLACK,
        Stroke::new(1.0_f32, Color32::from_rgb(33, 29, 27)),
        StrokeKind::Inside,
    );
    let center = rect.center();
    if let Some(record) = &app.record {
        let tint = if matches!(
            record.source,
            Source::Commons { .. } | Source::YouTube { .. }
        ) {
            GOLD
        } else {
            GREEN
        };
        let art_center = center - vec2(0.0, 22.0);
        let radius = (rect.height() * 0.25).max(32.0);
        vinyl(&p, art_center, radius, tint);
        let top = Rect::from_min_size(rect.min + vec2(18.0, 16.0), vec2(rect.width() - 36.0, 50.0));
        let clipped = p.with_clip_rect(top);
        clipped.text(
            top.min,
            Align2::LEFT_TOP,
            &record.title,
            FontId::proportional(18.0),
            TEXT,
        );
        clipped.text(
            top.min + vec2(0.0, 25.0),
            Align2::LEFT_TOP,
            &record.artist,
            FontId::proportional(12.0),
            DIM,
        );
        let play_rect = Rect::from_center_size(art_center, vec2(48.0, 48.0));
        let play = ui.interact(play_rect, Id::new("record_player"), Sense::click());
        play.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Button,
                true,
                if app.preview {
                    "Pause record"
                } else {
                    "Play record"
                },
            )
        });
        if play.has_focus() {
            p.circle_stroke(art_center, 25.0, Stroke::new(2.0_f32, MINT));
        }
        p.circle_filled(
            art_center,
            23.0,
            if play.hovered() {
                BG
            } else {
                Color32::from_black_alpha(190)
            },
        );
        if app.preview {
            for offset in [-4.0, 4.0] {
                p.rect_filled(
                    Rect::from_center_size(art_center + vec2(offset, 0.0), vec2(4.0, 16.0)),
                    1.0,
                    TEXT,
                );
            }
        } else {
            p.add(Shape::convex_polygon(
                vec![
                    art_center + vec2(-5.0, -8.0),
                    art_center + vec2(8.0, 0.0),
                    art_center + vec2(-5.0, 8.0),
                ],
                TEXT,
                Stroke::NONE,
            ));
        }
        if play.has_focus() {
            p.circle_stroke(art_center, 24.0, Stroke::new(2.0_f32, MINT));
        }
        if play.clicked() {
            app.toggle_preview();
        }
        timeline_at(
            app,
            ui,
            Rect::from_min_max(
                pos2(rect.left() + 18.0, rect.bottom() - 60.0),
                pos2(rect.right() - 18.0, rect.bottom() - 25.0),
            ),
            "hero_wave",
            false,
        );
        p.text(
            pos2(rect.left() + 18.0, rect.bottom() - 10.0),
            Align2::LEFT_BOTTOM,
            format!(
                "{} / {}",
                time(app.position, false),
                time(app.duration(), false)
            ),
            FontId::monospace(11.0),
            DIM,
        );
        p.text(
            pos2(rect.right() - 18.0, rect.bottom() - 10.0),
            Align2::RIGHT_BOTTOM,
            "AUDIO PREVIEW",
            FontId::monospace(10.0),
            FAINT,
        );
    } else {
        vinyl(&p, center + vec2(0.0, -77.0), 27.0, GREEN);
        p.text(
            center + vec2(0.0, -23.0),
            Align2::CENTER_CENTER,
            "Dig forgotten records.",
            FontId::proportional(26.0),
            TEXT,
        );
        p.text(
            center + vec2(0.0, 8.0),
            Align2::CENTER_CENTER,
            "Loop them, chop them across 16 pads, and put drums under them.",
            FontId::proportional(14.0),
            DIM,
        );
        if ui
            .put(
                Rect::from_center_size(center + vec2(0.0, 57.0), vec2(154.0, 46.0)),
                Button::new(label("Dig a record", 15.0, BG).strong())
                    .fill(GREEN)
                    .stroke(Stroke::NONE),
            )
            .clicked()
        {
            app.dig();
        }
        p.text(
            center + vec2(0.0, 102.0),
            Align2::CENTER_CENTER,
            "or press  D",
            FontId::monospace(11.0),
            FAINT,
        );
    }
}

fn transport(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 40.0;
        let ready = app.busy.is_none();
        if action(
            ui,
            "← Prev   O",
            ready && app.history_cursor.is_some_and(|i| i > 0),
        )
        .clicked()
        {
            app.prev();
        }
        if primary(ui, "Dig   D", ready).clicked() {
            app.dig();
        }
        if action(ui, "Jump   F", app.clip.is_some()).clicked() {
            app.jump();
        }
        if action(ui, "Next →   P", ready).clicked() {
            app.next();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let text = if app.is_saved() {
                "★ Update   W"
            } else {
                "☆ Save   W"
            };
            if ui
                .add_enabled(
                    app.record.is_some(),
                    Button::new(label(text, 13.0, GOLD))
                        .fill(Color32::from_rgb(41, 32, 17))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(93, 70, 29))),
                )
                .on_hover_text("Save this record, its loop, pads and pattern to your crate")
                .clicked()
            {
                app.save_record();
            }
        });
    });
}

fn history(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(label(format!("HISTORY  {}", app.history.len()), 13.0, DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !app.history.is_empty() && app.busy.is_none(),
                        Button::new(label("Clear", 11.0, DIM)).frame(false),
                    )
                    .clicked()
                {
                    app.history.clear();
                    app.history_cursor = None;
                }
            });
        });
        ui.separator();
        if app.history.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(5.0);
                small(ui, "Records you play show up here.");
                ui.add_space(3.0);
            });
        } else {
            ScrollArea::vertical()
                .id_salt("dig_history")
                .max_height(291.0)
                .show(ui, |ui| {
                    let mut load = None;
                    for (index, record) in app.history.iter().enumerate().rev() {
                        let current = app.history_cursor == Some(index);
                        let response = record_row(
                            ui,
                            &record.title,
                            &record.artist,
                            current,
                            app.busy.is_none(),
                        );
                        if response.clicked() {
                            load = Some(index);
                        }
                    }
                    if let Some(index) = load {
                        app.select_history(index);
                    }
                });
        }
    });

}

fn record_row(ui: &mut Ui, title: &str, subtitle: &str, active: bool, enabled: bool) -> Response {
    let size = vec2(ui.available_width(), 50.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, active, format!("{title} · {subtitle}")));
    let p = ui.painter_at(rect);
    let fill = if active {
        GREEN_BG
    } else if response.hovered() {
        INSET
    } else {
        Color32::from_black_alpha(52)
    };
    p.rect(
        rect,
        6.0,
        fill,
        Stroke::new(
            1.0_f32,
            if response.has_focus() { MINT }
            else if active { Color32::from_rgb(28, 89, 65) }
            else { EDGE },
        ),
        StrokeKind::Inside,
    );
    vinyl(
        &p,
        rect.left_center() + vec2(24.0, 0.0),
        14.0,
        if active { GREEN } else { FAINT },
    );
    let text = p.with_clip_rect(Rect::from_min_max(rect.min, rect.max - vec2(8.0, 0.0)));
    text.text(
        rect.left_top() + vec2(47.0, 9.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(13.0),
        TEXT,
    );
    text.text(
        rect.left_top() + vec2(47.0, 27.0),
        Align2::LEFT_TOP,
        subtitle,
        FontId::proportional(11.0),
        DIM,
    );
    response.on_hover_text(title)
}

fn record_details(app: &mut App, ui: &mut Ui) {
    if let Some(record) = &app.record {
        ui.add(Label::new(label(&record.title, 19.0, TEXT).strong()).wrap());
        ui.horizontal_wrapped(|ui| {
            small(ui, format!("{}  ·  {}", record.artist, record.genre));
            if let Some(views) = app.view_count {
                small(ui, views_label(views));
            }
            if let Source::Commons { page, .. } = &record.source
                && page.starts_with("https://commons.wikimedia.org/")
            {
                ui.hyperlink_to(label("Source & license ↗", 12.0, MINT), page);
            }
            if let Source::YouTube { id } = &record.source
                && let Some(url) = crate::youtube::watch_url(id)
            {
                ui.hyperlink_to(label("Open on YouTube ↗", 12.0, MINT), url);
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        small(ui, "SPEED");
        for (speed, name) in [
            (0.5, "0.5×"),
            (0.75, "0.75×"),
            (1.0, "1×"),
            (1.25, "1.25×"),
            (1.5, "1.5×"),
            (2.0, "2×"),
        ] {
            if pill(ui, name, app.studio.speed == speed).clicked() {
                app.studio.speed = speed;
            }
        }
        if action(ui, "Tap   B", true).clicked() {
            app.tap();
        }
        small(ui, format!("{} BPM", app.studio.bpm));
    });
    ui.add_space(4.0);
    if ui
        .add_sized(
            [ui.available_width(), 39.0],
            Button::new(label("Flip this record   S", 14.0, MINT).strong())
                .fill(GREEN_BG)
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(28, 91, 69))),
        )
        .clicked()
    {
        app.page = Page::Studio;
    }
    small(
        ui,
        "Turn it into a beat. Loop a section, chop it across 16 pads, and sequence drums.",
    );
}

fn crate_panel(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let has_records = !app.library.records.is_empty() && app.busy.is_none();
        ui.horizontal(|ui| {
            ui.label(label("CRATE", 13.0, DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if action(ui, "Export JSON", has_records).clicked() {
                    app.export("json");
                }
                if action(ui, "Export CSV", has_records).clicked() {
                    app.export("csv");
                }
            });
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if pill(
                ui,
                &format!("All saved  {}", app.library.records.len()),
                app.collection.is_none(),
            )
            .clicked()
            {
                app.collection = None;
            }
            for name in &app.library.collections {
                if pill(ui, name, app.collection.as_ref() == Some(name)).clicked() {
                    app.collection = Some(name.clone());
                }
            }
            if action(ui, "+ New", true).clicked() {
                app.new_collection = Some(String::new());
            }
        });
        if let Some(value) = &mut app.new_collection {
            let mut add = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text("Crate name")
                        .char_limit(48),
                );
                add = action(ui, "Create", !value.trim().is_empty()).clicked()
                    || (response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                cancel = action(ui, "Cancel", true).clicked();
            });
            if add && !value.trim().is_empty() {
                let name = value.trim().to_owned();
                if !app.library.collections.contains(&name) {
                    app.library.collections.push(name.clone());
                }
                app.collection = Some(name);
                app.new_collection = None;
                app.persist_library();
            } else if cancel {
                app.new_collection = None;
            }
        }
        let filter_id = ui.id().with("crate_filter");
        let mark_id = ui.id().with("crate_marked");
        let mut marked = ui.data(|data| data.get_temp::<Vec<Source>>(mark_id)).unwrap_or_default();
        marked.retain(|source| app.library.records.iter().any(|saved| &saved.record.source == source));
        let favorite_id = ui.id().with("crate_favorites");
        let mut filter = ui.data(|data| data.get_temp::<String>(filter_id)).unwrap_or_default();
        let sort_labels = ["Recently saved", "Title", "Artist", "BPM"];
        let mut sort = app.library.interface.crate_sort.min(sort_labels.len() - 1);
        let mut favorites = ui.data(|data| data.get_temp::<bool>(favorite_id)).unwrap_or(false);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search title, artist, genre or tags"));
            ComboBox::from_id_salt("crate_order").selected_text(sort_labels[sort]).show_ui(ui, |ui| {
                for (index, label) in sort_labels.into_iter().enumerate() {
                    ui.selectable_value(&mut sort, index, label);
                }
            });
            ui.checkbox(&mut favorites, "Favorites only");
        });
        let search = filter.trim().to_lowercase();
        ui.data_mut(|data| {
            data.insert_temp(filter_id, filter);
            data.insert_temp(favorite_id, favorites);
        });
        let mut indices: Vec<_> = app.library.records.iter().enumerate().filter(|(_, saved)| {
            saved.in_collection(app.collection.as_deref()) && (!favorites || saved.favorite)
                && [saved.record.title.as_str(), &saved.record.artist, &saved.record.genre, &saved.tags, &saved.notes]
                    .into_iter().any(|field| field.to_lowercase().contains(&search))
        }).map(|(index, _)| index).collect();
        match sort {
            1 => indices.sort_by_cached_key(|&index| app.library.records[index].record.title.to_lowercase()),
            2 => indices.sort_by_cached_key(|&index| app.library.records[index].record.artist.to_lowercase()),
            3 => indices.sort_by_key(|&index| app.library.records[index].studio.bpm),
            _ => {}
        }
        let mut changed = app.library.interface.crate_sort != sort;
        app.library.interface.crate_sort = sort;
        if !marked.is_empty() {
            ui.horizontal_wrapped(|ui| {
                small(ui, format!("{} marked", marked.len()));
                if action(ui, "Clear marks", true).clicked() {
                    marked.clear();
                }
                ui.menu_button("Add marked to crate…", |ui| {
                    if app.library.collections.is_empty() {
                        small(ui, "Create a crate with + New first.");
                    }
                    for name in &app.library.collections {
                        if ui.button(name).clicked() {
                            for saved in &mut app.library.records {
                                if marked.contains(&saved.record.source) && !saved.in_collection(Some(name)) {
                                    saved.toggle_collection(name);
                                    changed = true;
                                }
                            }
                            marked.clear();
                            ui.close_menu();
                        }
                    }
                });
            });
        }
        let mut load = None;
        let mut remove = None;
        let mut relink = None;
        let any = !indices.is_empty();
        let ready = app.busy.is_none();
        for index in indices {
            let saved = &mut app.library.records[index];
            ui.push_id(index, |ui| ui.horizontal(|ui| {
                let source = &saved.record.source;
                let mut checked = marked.contains(source);
                let mark = ui.checkbox(&mut checked, "");
                mark.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, checked, format!("Mark {}", saved.record.title)));
                if mark.changed() {
                    if checked { marked.push(source.clone()); }
                    else { marked.retain(|item| item != source); }
                }
                let width = (ui.available_width() - 232.0).max(180.0);
                let missing = matches!(&saved.record.source, Source::File(path) if !path.is_file());
                ui.allocate_ui_with_layout(vec2(width, 50.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(width);
                    let subtitle = format!("{} · {} BPM{}{}", saved.record.artist, saved.studio.bpm,
                        if saved.tags.is_empty() { "" } else { " · " }, saved.tags);
                    if record_row(ui, &saved.record.title, &subtitle,
                        app.record.as_ref() == Some(&saved.record), ready && !missing).clicked()
                    {
                        load = Some(saved.clone());
                    }
                });
                if missing && action(ui, "Relink…", ready).clicked() {
                    relink = Some(saved.record.source.clone());
                }
                ui.menu_button("Organize", |ui| {
                    changed |= ui.checkbox(&mut saved.favorite, "Favorite").changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut saved.tags).hint_text("Tags").char_limit(140)).changed();
                    changed |= ui.add(egui::TextEdit::multiline(&mut saved.notes).hint_text("Notes").char_limit(500)).changed();
                    for collection in &app.library.collections {
                        let mut belongs = saved.in_collection(Some(collection));
                        if ui.checkbox(&mut belongs, collection).changed() {
                            saved.toggle_collection(collection);
                            changed = true;
                        }
                    }
                    if ui.button("Remove from library").clicked() {
                        remove = Some(index);
                        ui.close_menu();
                    }
                });
            }));
        }
        ui.data_mut(|data| data.insert_temp(mark_id, marked));
        if let Some(source) = relink {
            app.relink_crate_audio(source);
        }
        if changed {
            app.persist_library();
        }
        if !any {
            let message = if search.is_empty() && !favorites {
                "Nothing saved here yet. Press W on a record worth keeping."
            } else {
                "No saved records match these filters."
            };
            small(ui, message);
        }
        if let Some(index) = remove {
            app.library.records.remove(index);
            app.persist_library();
        }
        if let Some(saved) = load {
            if app.page == Page::Crate {
                app.page = Page::Dig;
            }
            app.load(saved.record, Some(saved.studio), None);
        }
    });
}

fn studio_transport(app: &mut App, ui: &mut Ui) {
    let pattern = app.playing && app.song_playing.is_none();
    let song = app.song_playing.is_some();
    ui.horizontal_wrapped(|ui| {
        for (command, title) in [
            (Action::Preview, if app.preview { "Pause source" } else { "Preview source" }),
            (Action::Pattern, if pattern { "Stop pattern" } else { "Play pattern" }),
            (Action::Song, if song { "Stop song" } else { "Play song" }),
            (Action::Stop, "Stop all"),
        ] {
            if command_button(ui, app, command, title).clicked() {
                command.run(app);
            }
        }
        if let Some(take) = &app.input_recording {
            ui.label(label(format!("Recording input · {:.1} s", take.seconds()), 12.0, GOLD));
            ui.add(egui::ProgressBar::new(take.peak().min(1.0)).desired_width(75.0))
                .on_hover_text("Audio input peak");
            if action(ui, "Finish input", true).clicked() {
                app.finish_input();
            }
        }
        if app.page == Page::Studio && ui.checkbox(&mut app.fill, "Fill").on_hover_text("Activate fill-only sequencer steps during playback").changed() {
            app.send(crate::audio::Command::Fill(app.fill));
        }
        if app.recording {
            let beats = app.output.as_ref().map_or(0, |output| output.meters.count_in_beats());
            let status = if beats > 0 { format!("Count-in · {beats}") }
                else if app.playing { "Recording".into() } else { "Armed · waiting for MIDI clock".into() };
            ui.label(label(status, 13.0, GOLD));
        }
        if let Some(step) = app.step {
            small(ui, format!("Bar {} · Beat {}", step / STEPS + 1, step % STEPS / 4 + 1));
        }
        if let Some(output) = &app.output && app.page == Page::Studio {
            let peak = output.meters.peak();
            ui.add(egui::ProgressBar::new(peak.min(1.0)).desired_width(84.0)
                .fill(if peak >= 1.0 { GOLD } else { GREEN }))
                .on_hover_text(format!("Master peak {:.1} dBFS before limiting", 20.0 * peak.max(1e-6).log10()));
            if peak >= 1.0 {
                ui.label(label("Peak", 11.0, GOLD));
            }
        }
        if app.keyboard && app.page == Page::Studio {
            ui.label(label("Computer keyboard playing", 12.0, GOLD));
        }
    });
}

fn studio_header(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        let suffix = if app.project_dirty() { " · Unsaved" } else { "" };
        ui.add(Label::new(label(format!("{}{suffix}", app.project_name()), 14.0, TEXT).strong()).truncate());
        for (view, title) in [
            (StudioView::Sample, "Sample"),
            (StudioView::Perform, "Perform"),
            (StudioView::Sequence, "Sequence"),
            (StudioView::Arrange, "Arrange"),
            (StudioView::Mix, "Mix"),
        ] {
            if pill(ui, title, app.studio_view == view).clicked() {
                Action::Studio(view).run(app);
            }
        }
        project_history(app, ui);
        let ready = app.busy.is_none();
        if command_button(ui, app, Action::Export, "Export…").clicked() {
            Action::Export.run(app);
        }
        if action(ui, "Dig another", ready).clicked() {
            app.dig();
        }
    });
}

fn studio(app: &mut App, ui: &mut Ui) {
    let view = app.studio_view;
    if view == StudioView::Sequence {
        sequencer::show(app, ui);
        return;
    }
    if view == StudioView::Sample {
        sample::show(app, ui);
        return;
    }
    ScrollArea::vertical()
        .id_salt(match view {
            StudioView::Sample => unreachable!(),
            StudioView::Perform => "perform",
            StudioView::Sequence => unreachable!(),
            StudioView::Arrange => "arrangement",
            StudioView::Mix => "mix",
        })
        .auto_shrink([false, false])
        .show(ui, |ui| match view {
            StudioView::Sample => unreachable!(),
            StudioView::Perform => performance_panel(app, ui),
            StudioView::Sequence => unreachable!(),
            StudioView::Arrange => arrangement::show(app, ui),
            StudioView::Mix => mixer_panel(app, ui),
        });
}

fn performance_panel(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        if pill(ui, "16 pads", !app.library.midi.piano_view).clicked() {
            app.set_piano_view(false);
        }
        if pill(ui, "88-key chop piano", app.library.midi.piano_view).clicked() {
            app.set_piano_view(true);
        }
        let connected = app
            .midi_devices
            .iter()
            .filter(|device| device.connection.is_some())
            .count();
        let midi = if connected == 0 {
            "MIDI devices".into()
        } else {
            format!("MIDI devices · {connected}")
        };
        if command_button(ui, app, Action::MidiSetup, &midi).clicked() {
            Action::MidiSetup.run(app);
        }
        if app.recording && pill(ui, "● Finish recording", true).clicked() {
            app.toggle_recording();
        }
    });
    retrospective_capture(app, ui);
    if app.library.midi.piano_view {
        piano(app, ui);
    } else {
        pads(app, ui);
    }
}

fn retrospective_capture(app: &mut App, ui: &mut Ui) {
    let Some(output) = &app.output else { return };
    let available = output.performance.has_notes();
    let bars = output.performance.bars();
    let overflowed = output.performance.overflowed();
    let mut request = None;
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(available, |ui| {
            ui.menu_button("Capture last…", |ui| {
                for bars in [4, 8, 16] {
                    if ui.button(format!("{bars} bars")).clicked() {
                        request = Some(bars);
                        ui.close_menu();
                    }
                }
            });
        });
        let status = if !available { "Play a pad or key to start capture".into() }
            else if bars >= 16.0 { "Up to 16 bars of history".into() }
            else { format!("{bars:.1} bars since first captured hit") };
        small(ui, status);
        if overflowed {
            ui.label(label("Capture history overflowed", 12.0, GOLD));
        }
    });
    if let Some(bars) = request {
        app.preview_performance(bars);
    }
    let Some(preview) = app.output.as_ref().and_then(|output| output.performance.preview())
    else { return };
    let mut keep = false;
    let mut dismiss = false;
    ui.horizontal_wrapped(|ui| {
        small(ui, format!("{}-bar preview · {}", preview.bars,
            if preview.synced { "Aligned to transport bars" } else { "Free timing" }));
        egui::ComboBox::from_id_salt("capture_quantize")
            .selected_text(format!("Quantize {}", app.capture_quantize.label()))
            .show_ui(ui, |ui| {
                for quantize in crate::model::Quantize::ALL {
                    ui.selectable_value(&mut app.capture_quantize, quantize, quantize.label());
                }
            });
        keep = action(ui, "Capture to new pattern", !preview.is_empty() && !app.recording).clicked();
        dismiss = action(ui, "Dismiss", true).clicked();
    });
    if preview.overflowed || preview.lost_bend {
        let notice = if preview.overflowed && preview.lost_bend {
            "Partial history. Pitch bends are omitted."
        } else if preview.lost_bend {
            "Pitch bends are omitted."
        } else {
            "Preview contains partial history."
        };
        small(ui, notice);
    }
    let notes = preview.notes(app.capture_quantize, app.studio.swing);
    let width = ui.available_width().clamp(40.0, 480.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let end = (preview.bars as u32 * crate::model::TICKS_PER_BAR) as f32;
    for bar in 0..=preview.bars {
        let x = egui::lerp(rect.x_range(), bar as f32 / preview.bars as f32);
        painter.vline(x, rect.y_range(), egui::Stroke::new(1.0_f32, ui.visuals().weak_text_color()));
    }
    for note in notes {
        let x = egui::lerp(rect.x_range(), note.tick as f32 / end);
        let length = note.length.unwrap_or(12) as f32;
        let x_end = egui::lerp(rect.x_range(), (note.tick as f32 + length) / end);
        let y = if matches!(note.input, crate::model::ChopInput::Pad(_)) {
            rect.top() + 6.0
        } else {
            rect.bottom() - 6.0
        };
        painter.hline(x..=(x + 2.0).max(x_end), y, egui::Stroke::new(2.0_f32, GOLD));
    }
    if keep {
        app.keep_performance();
    } else if dismiss && let Some(output) = &app.output {
        output.performance.invalidate_preview();
    }
}

fn loop_panel(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let ready = app.clip.is_some();
        ui.horizontal(|ui| {
            if command_button(
                ui, app, Action::Preview,
                if app.preview {
                    "Ⅱ Pause"
                } else {
                    "▶ Preview"
                },
            )
            .clicked()
            {
                Action::Preview.run(app);
            }
            let title = app
                .source_record(app.active_source)
                .map_or("Import audio or dig a record to start chopping", |record| {
                    &record.title
                });
            ui.add(Label::new(label(title, 15.0, TEXT).strong()).truncate());
        });
        if app.active_source == 0 && let Some(views) = app.view_count {
            small(ui, views_label(views));
        }
        if let Some(release) = app.source_record(app.active_source).and_then(|record| record.release.as_ref())
        {
            release_details(ui, release);
        }
        if ready {
            waveform_zoom(app, ui);
            if ui.checkbox(&mut app.library.interface.stereo_waveform, "Stereo channels").changed() {
                app.persist_library();
            }
            waveform_ruler(app, ui);
        }
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), app.wave_height), Sense::hover());
        let colors = app.library.interface.control_colors();
        ui.painter().rect(
            rect,
            6.0,
            BG,
            Stroke::new(1.0_f32, colors.edge),
            StrokeKind::Inside,
        );
        timeline_at(app, ui, rect.shrink(6.0), "studio_wave", true);
        if ready && app.waveform.zoom > 1.0 {
            waveform_overview(app, ui);
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(label(time(app.position, true), 12.0, DIM).monospace());
            if let Some((start, end)) = app.selection() {
                small(ui, format!("Selection {:.3} s", end - start));
            }
            if ready {
                small(ui, "Click to seek · Drag to select or trim a loop");
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (start, end) = app.waveform.range(app.duration());
                let visible = if app.waveform.zoom > 1.0 {
                    format!("{} – {}", time(start, true), time(end, true))
                } else {
                    time(end, true)
                };
                ui.label(label(visible, 11.0, DIM).monospace());
            });
        });
        ui.horizontal_wrapped(|ui| {
            if command_button(ui, app, Action::LoopIn, "In   Z").clicked() {
                Action::LoopIn.run(app);
            }
            let loop_button = Button::new(label(
                "Loop   X",
                13.0,
                if app.studio.looping { MINT } else { TEXT },
            ))
            .fill(if app.studio.looping { GREEN_BG } else { INSET });
            if ui
                .add_enabled(Action::ToggleLoop.unavailable(app).is_none(), loop_button)
                .clicked()
            {
                Action::ToggleLoop.run(app);
            }
            if command_button(ui, app, Action::LoopOut, "Out   C").clicked() {
                Action::LoopOut.run(app);
            }
            if action(ui, "Use full sample", ready).clicked() {
                app.set_loop_range((0.0, app.duration()));
            }
            if action(
                ui,
                "Clear",
                app.studio.loop_source == app.active_source
                    && (app.studio.loop_in.is_some() || app.studio.loop_out.is_some()),
            )
            .clicked()
            {
                app.studio.loop_in = None;
                app.studio.loop_out = None;
                app.studio.looping = false;
            }
            if command_button(ui, app, Action::ExportLoop, "Export loop WAV")
            .on_hover_text("Export the selected source audio at its original sample rate")
            .clicked()
            {
                Action::ExportLoop.run(app);
            }
        });
        let duration = app.duration();
        if app.studio.loop_source == app.active_source {
            ui.horizontal_wrapped(|ui| {
                time_control(ui, "IN", &mut app.studio.loop_in, duration);
                ui.add_space(10.0);
                time_control(ui, "OUT", &mut app.studio.loop_out, duration);
            });
        }
    });
}

fn waveform_zoom(app: &mut App, ui: &mut Ui) {
    let duration = app.duration();
    ui.horizontal_wrapped(|ui| {
        let mut playhead = app.position;
        if ui.add(DragValue::new(&mut playhead).range(0.0..=duration)
            .speed(0.01).fixed_decimals(3).prefix("Seek ").suffix(" s")).changed()
        {
            app.seek(playhead);
        }
        small(ui, "Snap");
        ComboBox::from_id_salt("wave_snap").selected_text(app.wave_snap.label()).show_ui(ui, |ui| {
            for snap in Snap::ALL {
                ui.selectable_value(&mut app.wave_snap, snap, snap.label());
            }
        }).response.on_hover_text("Hold Alt to temporarily bypass snapping");
        ui.checkbox(&mut app.wave_beats, "Beat ruler").on_hover_text("Shows beats from source start at the project tempo");
        ui.add(DragValue::new(&mut app.wave_height).range(72.0..=340.0).prefix("Height "));
    });
    ui.horizontal_wrapped(|ui| {
        small(ui, "Zoom");
        if action(ui, "−", app.waveform.zoom > 1.0)
            .on_hover_text("Zoom out around the playhead")
            .clicked()
        {
            app.waveform.zoom_by(0.5, app.position, duration);
        }
        small(ui, format!("{:.1}×", app.waveform.zoom));
        if action(ui, "+", app.waveform.zoom < WaveformView::MAX_ZOOM)
            .on_hover_text("Zoom in around the playhead")
            .clicked()
        {
            app.waveform.zoom_by(2.0, app.position, duration);
        }
        if action(ui, "Fit", app.waveform.zoom > 1.0)
            .on_hover_text("Show the entire source")
            .clicked()
        {
            app.waveform = WaveformView::default();
        }
        let selection = app.selection();
        if action(ui, "Zoom to selection", selection.is_some()).clicked()
            && let Some(range) = selection
        {
            app.waveform.fit(range, duration);
        }
    });
}

fn waveform_ruler(app: &App, ui: &mut Ui) {
    let text_color = app.library.interface.control_colors().secondary_text;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let (start, end) = app.waveform.range(app.duration());
    let span = end - start;
    if span <= 0.0 {
        return;
    }
    let desired = span / f64::from((rect.width() / 90.0).max(1.0));
    let beat = 60.0 / f64::from(app.studio.bpm);
    let step = if app.wave_beats {
        (desired / beat).ceil().max(1.0) * beat
    } else {
        let decade = 10.0_f64.powf(desired.log10().floor());
        [1.0, 2.0, 5.0, 10.0].into_iter()
            .map(|factor| factor * decade)
            .find(|&step| step >= desired)
            .unwrap_or(10.0 * decade)
    };
    let painter = ui.painter_at(rect);
    let first = (start / step).ceil() as u64;
    let last = (end / step).floor() as u64;
    for index in first..=last {
        let at = index as f64 * step;
        let x = rect.left() + 6.0 + ((at - start) / span) as f32 * (rect.width() - 12.0);
        let text = if app.wave_beats {
            let beat_number = (at / beat).round() as u64;
            format!("{}.{}", beat_number / 4 + 1, beat_number % 4 + 1)
        } else if step < 1.0 {
            time(at, true)
        } else {
            time(at, false)
        };
        painter.vline(x, rect.bottom() - 5.0..=rect.bottom(), Stroke::new(1.0_f32, text_color));
        painter.text(pos2(x + 3.0, rect.top()), Align2::LEFT_TOP, text, FontId::monospace(11.0), text_color);
    }
}

fn waveform_overview(app: &mut App, ui: &mut Ui) {
    let Some(clip) = app.active_clip() else { return };
    let interface = app.library.interface;
    let colors = interface.control_colors();
    let trace = if interface.high_contrast { colors.secondary_text } else { FAINT };
    let duration = clip.duration();
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 4.0, BG);
    let cy = rect.center().y;
    for (index, peak) in clip.peaks.iter().enumerate() {
        let x = rect.left() + (index as f32 + 0.5) / clip.peaks.len() as f32 * rect.width();
        let amplitude = (peak.sqrt() * rect.height() * 0.43).max(0.6);
        p.vline(
            x,
            cy - amplitude..=cy + amplitude,
            Stroke::new(1.0_f32, trace),
        );
    }
    let (start, end) = app.waveform.range(duration);
    let at = |seconds: f64| rect.left() + (seconds / duration) as f32 * rect.width();
    let visible = Rect::from_min_max(pos2(at(start), rect.top()), pos2(at(end), rect.bottom()));
    p.rect(
        visible,
        3.0,
        Color32::from_black_alpha(28),
        Stroke::new(colors.emphasis, colors.accent),
        StrokeKind::Inside,
    );
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let time = f64::from((pointer.x - rect.left()) / rect.width()) * duration;
        app.waveform.center_on(time, duration);
    }
    response.on_hover_text("Click or drag to move around the source");
}

enum SampleAction {
    Select(usize),
    Play(usize),
    BeatAudition(usize),
    Export(usize),
    AssignPad(usize),
    AssignKey(usize),
    Rename(usize),
    CommitName(usize, String),
    CancelRename,
    Remove(usize),
}

fn samples_panel(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(label("SAMPLES", 13.0, DIM));
            small(ui, format!("{}", app.samples.len()));
            let selection = app.selection();
            let can_save = selection.is_some() && app.samples.len() < MAX_NAMED_SAMPLES;
            let name = ui.add_enabled(
                can_save,
                egui::TextEdit::singleline(&mut app.sample_name)
                    .hint_text("Sample name (optional)")
                    .desired_width(177.0)
                    .char_limit(MAX_SAMPLE_NAME),
            );
            if action(ui, "Save selection", can_save).clicked()
                || (can_save
                    && name.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            {
                app.save_selection();
            }
            if let Some(index) = app.selected_sample
                && selection.is_some_and(|range| range != app.samples[index].range())
                && action(ui, "Update saved cut", true).clicked()
            {
                app.update_sample(index);
            }
            if let Some(index) = app.selected_sample {
                ComboBox::from_id_salt("similarity_priority").selected_text(app.match_priority.label())
                    .show_ui(ui, |ui| {
                        for (mode, name) in crate::analysis::MatchBy::OPTIONS {
                            ui.selectable_value(&mut app.match_priority, mode, name);
                        }
                    }).response.on_hover_text("Choose what Find similar should prioritize");
                if action(ui, "Find similar locally", true).clicked() {
                    app.search_similar_cut(index);
                }
                if action(ui, "Dig for similar", app.busy.is_none()).clicked() {
                    app.build_queue(Some(index));
                }
            }
        });
        if app.samples.is_empty() {
            small(ui, "Select a range on the waveform to save it here.");
            return;
        }
        let marks_id = ui.id().with("marked_cuts");
        let query_id = ui.id().with("cut_query");
        let prefix_id = ui.id().with("cut_prefix");
        let mut marked = ui.data(|data| data.get_temp::<Vec<(usize, crate::model::NamedSample)>>(marks_id)).unwrap_or_default();
        marked.retain(|(index, cut)| app.samples.get(*index) == Some(cut));
        let mut query = ui.data(|data| data.get_temp::<String>(query_id)).unwrap_or_default();
        let mut prefix = ui.data(|data| data.get_temp::<String>(prefix_id)).unwrap_or_default();
        let mut batch_rename = false;
        let mut batch_export = false;
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search cuts or source titles").desired_width(190.0));
            if !marked.is_empty() {
                small(ui, format!("{} marked", marked.len()));
                if action(ui, "Clear marks", true).clicked() {
                    marked.clear();
                }
                ui.add(egui::TextEdit::singleline(&mut prefix).hint_text("Name prefix").desired_width(115.0).char_limit(MAX_SAMPLE_NAME));
                batch_rename = action(ui, "Number marked cuts", !prefix.trim().is_empty()).clicked();
                batch_export = action(ui, "Export marked cuts…", app.busy.is_none()).clicked();
            }
        });
        let search = query.trim().to_lowercase();
        let mut pending = None;
        ScrollArea::vertical()
            .id_salt("named_samples")
            .max_height(175.0)
            .show(ui, |ui| {
                for (index, sample) in app.samples.iter().enumerate() {
                    let source_title = (!app.studio.sources.is_empty()).then(|| {
                        app.source_record(sample.source).map(|record| record.title.clone())
                    }).flatten();
                    if !search.is_empty()
                        && !sample.name.to_lowercase().contains(&search)
                        && !source_title.as_ref().is_some_and(|title| title.to_lowercase().contains(&search))
                    {
                        continue;
                    }
                    let pad_uses = (0..STEPS)
                        .filter(|&pad| app.studio.pad_sources[pad] == sample.source && app.pad_range(pad) == Some(sample.range()))
                        .count();
                    let key_uses = app.studio.key_chops.iter()
                        .filter(|&(note, chop)| *chop == KeyChop::Cut(sample.start, sample.end)
                            && app.studio.key_sources.get(note).copied().unwrap_or(0) == sample.source)
                        .count();
                    ui.push_id(index, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if let Some((row, value)) =
                                app.sample_rename.as_mut().filter(|(row, _)| *row == index)
                            {
                                let edit = ui.add(
                                    egui::TextEdit::singleline(value)
                                        .hint_text("Sample name")
                                        .desired_width(220.0)
                                        .char_limit(MAX_SAMPLE_NAME),
                                );
                                let name = value.trim();
                                if action(ui, "Apply", !name.is_empty()).clicked()
                                    || (!name.is_empty()
                                        && edit.lost_focus()
                                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                                {
                                    pending = Some(SampleAction::CommitName(*row, name.to_owned()));
                                }
                                if action(ui, "Cancel", true).clicked() {
                                    pending = Some(SampleAction::CancelRename);
                                }
                                return;
                            }
                            let mut checked = marked.iter().any(|(row, _)| *row == index);
                            let mark = ui.checkbox(&mut checked, "");
                            mark.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, checked, format!("Mark {}", sample.name)));
                            if mark.changed() {
                                if checked {
                                    marked.push((index, sample.clone()));
                                } else {
                                    marked.retain(|(row, _)| *row != index);
                                }
                            }
                            let column = (ui.available_width() - 345.0).max(120.0);
                            ui.allocate_ui_with_layout(
                                vec2(column, 38.0),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    let color = if app.selected_sample == Some(index) {
                                        MINT
                                    } else {
                                        TEXT
                                    };
                                    ui.add(
                                        Label::new(label(&sample.name, 13.0, color).strong())
                                            .truncate().sense(Sense::drag()),
                                    ).on_hover_text("Drag this cut onto a pad or key")
                                        .dnd_set_drag_payload(CutDrag { name: sample.name.clone(), source: sample.source, range: sample.range() });
                                    if let Some(title) = source_title.as_deref() {
                                        small(ui, title);
                                    }
                                    small(
                                        ui,
                                        format!(
                                            "{} – {}  ·  {:.3} s",
                                            time(sample.start, true),
                                            time(sample.end, true),
                                            sample.end - sample.start
                                        ),
                                    );
                                },
                            );
                            if action(ui, "Select", true)
                                .on_hover_text("Recall the range on the waveform")
                                .clicked()
                            {
                                pending = Some(SampleAction::Select(index));
                            }
                            if action(ui, "Play", app.output.is_some() && !app.playing)
                                .on_hover_text("Audition just this cut")
                                .clicked()
                            {
                                pending = Some(SampleAction::Play(index));
                            }
                            if action(ui, "WAV", app.busy.is_none())
                                .on_hover_text("Export this cut as WAV")
                                .clicked()
                            {
                                pending = Some(SampleAction::Export(index));
                            }
                            ui.menu_button("Cut actions", |ui| {
                                if action(ui, "Audition with beat", app.output.is_some()).clicked() {
                                    pending = Some(SampleAction::BeatAudition(index));
                                    ui.close_menu();
                                }
                                small(ui, format!("Used by {pad_uses} pad assignments and {key_uses} key overrides"));
                                if action(ui, &format!("Assign to pad {}", app.selected_pad + 1), true).clicked() {
                                    pending = Some(SampleAction::AssignPad(index));
                                    ui.close_menu();
                                }
                                if action(ui, &format!("Assign to key {}", piano_note(app.piano_key)), !app.studio.chromatic_enabled)
                                    .on_disabled_hover_text("Switch to Chops per key in Perform")
                                    .clicked()
                                {
                                    pending = Some(SampleAction::AssignKey(index));
                                    ui.close_menu();
                                }
                                if action(ui, "Rename", true).clicked() {
                                    pending = Some(SampleAction::Rename(index));
                                    ui.close_menu();
                                }
                                if action(ui, "Remove cut", true).clicked() {
                                    pending = Some(SampleAction::Remove(index));
                                    ui.close_menu();
                                }
                            });
                        });
                    });
                    ui.separator();
                }
            });
        let mut indices: Vec<_> = marked.iter().map(|(index, _)| *index).collect();
        indices.sort_unstable();
        if batch_rename {
            app.rename_samples(&indices, prefix.trim());
            for (index, cut) in &mut marked {
                cut.clone_from(&app.samples[*index]);
            }
        }
        if batch_export {
            app.export_samples(&indices);
        }
        ui.data_mut(|data| {
            data.insert_temp(marks_id, marked);
            data.insert_temp(query_id, query);
            data.insert_temp(prefix_id, prefix);
        });
        match pending {
            Some(SampleAction::Select(index)) => app.recall_sample(index),
            Some(SampleAction::Play(index)) => {
                let range = app.samples[index].range();
                app.select_source(app.samples[index].source);
                app.audition_range(range);
            }
            Some(SampleAction::AssignPad(index)) => {
                let cut = &app.samples[index];
                app.assign_pad_range(app.selected_pad, cut.source, cut.range());
            }
            Some(SampleAction::AssignKey(index)) => {
                let cut = &app.samples[index];
                app.assign_piano_source(cut.source, cut.range());
            }
            Some(SampleAction::BeatAudition(index)) => app.audition_saved_cut_with_beat(index),
            Some(SampleAction::Export(index)) => app.export_sample(index),
            Some(SampleAction::Rename(index)) => {
                app.sample_rename = Some((index, app.samples[index].name.clone()))
            }
            Some(SampleAction::CommitName(index, name)) => {
                app.rename_sample(index, name);
                app.sample_rename = None;
            }
            Some(SampleAction::CancelRename) => app.sample_rename = None,
            Some(SampleAction::Remove(index)) => app.remove_sample(index),
            None => {}
        }
    });
}

fn time_control(ui: &mut Ui, name: &str, time: &mut Option<f64>, duration: f64) {
    ui.label(label(name, 11.0, DIM).monospace());
    if let Some(time) = time {
        if action(ui, "«", true)
            .on_hover_text("Move back 100 ms")
            .clicked()
        {
            *time -= 0.1;
        }
        if action(ui, "‹", true)
            .on_hover_text("Move back 10 ms")
            .clicked()
        {
            *time -= 0.01;
        }
        *time = time.clamp(0.0, duration);
        ui.add(
            DragValue::new(time)
                .speed(0.01)
                .fixed_decimals(3)
                .range(0.0..=duration)
                .suffix(" s"),
        )
        .on_hover_text("Click to enter an exact time in seconds.");
        if action(ui, "›", true)
            .on_hover_text("Move forward 10 ms")
            .clicked()
        {
            *time += 0.01;
        }
        if action(ui, "»", true)
            .on_hover_text("Move forward 100 ms")
            .clicked()
        {
            *time += 0.1;
        }
    } else {
        ui.label(label("--:--.---", 12.0, FAINT).monospace());
    }
}

#[derive(Clone, Copy)]
enum WaveDrag {
    Select { anchor: f64 },
    TrimIn { end: f64, offset: f64 },
    TrimOut { start: f64, offset: f64 },
}

impl WaveDrag {
    fn range(self, time: f64, duration: f64) -> (f64, f64) {
        match self {
            Self::Select { anchor } => (anchor.min(time), anchor.max(time)),
            Self::TrimIn { end, offset } => (
                (time + offset).clamp(0.0, (end - MIN_LOOP_SECONDS).max(0.0)),
                end,
            ),
            Self::TrimOut { start, offset } => (
                start,
                (time + offset).clamp((start + MIN_LOOP_SECONDS).min(duration), duration),
            ),
        }
    }
}

fn timeline_at(app: &mut App, ui: &mut Ui, rect: Rect, id: &str, select_loop: bool) {
    let Some(clip) = app.active_clip() else {
        return;
    };
    let duration = clip.duration();
    let (visible_start, visible_end) = if select_loop {
        app.waveform.range(duration)
    } else {
        (0.0, duration)
    };
    let span = visible_end - visible_start;
    if span <= 0.0 || rect.width() <= 0.0 {
        return;
    }
    let visible = visible_start..=visible_end;
    let at = |time: f64| rect.left() + ((time - visible_start) / span) as f32 * rect.width();
    let time_at = |pointer: Pos2| {
        visible_start + f64::from(((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0)) * span
    };
    let response = ui
        .interact(rect, ui.make_persistent_id(id), Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true,
        "Source waveform. Use the Seek, In and Out controls to edit without dragging."));
    let selection_id = response.id.with("loop_drag");
    let loop_range = app.selection();
    let snap = if ui.input(|input| input.modifiers.alt) { Snap::Off } else { app.wave_snap };
    let snap_time = |time| snap.time(clip, time, app.studio.bpm);
    let start_drag = |origin: Pos2| {
        let time = time_at(origin);
        if let Some((start, end)) = loop_range {
            let distance = |edge| {
                if visible.contains(&edge) {
                    (at(edge) - origin.x).abs()
                } else {
                    f32::INFINITY
                }
            };
            let (left, right) = (distance(start), distance(end));
            if left <= 8.0 && left <= right {
                return WaveDrag::TrimIn {
                    end,
                    offset: start - time,
                };
            }
            if right <= 8.0 {
                return WaveDrag::TrimOut {
                    start,
                    offset: end - time,
                };
            }
        }
        WaveDrag::Select { anchor: snap_time(time) }
    };
    if select_loop
        && response.hovered()
        && ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pointer| !matches!(start_drag(pointer), WaveDrag::Select { .. }))
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if select_loop
        && response.drag_started()
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        ui.data_mut(|data| data.insert_temp(selection_id, start_drag(origin)));
    }
    let pointer_time = response.interact_pointer_pos().map(time_at);
    let drag = if select_loop && (response.dragged() || response.drag_stopped()) {
        ui.data(|data| data.get_temp::<WaveDrag>(selection_id))
            .zip(pointer_time)
            .map(|(target, time)| {
                let (start, end) = target.range(time, duration);
                let range = match target {
                    WaveDrag::Select { anchor } => {
                        let end = snap_time(time);
                        (anchor.min(end), anchor.max(end))
                    }
                    WaveDrag::TrimIn { .. } => (snap_time(start).min((end - MIN_LOOP_SECONDS).max(0.0)), end),
                    WaveDrag::TrimOut { .. } => (start, snap_time(end).max((start + MIN_LOOP_SECONDS).min(duration))),
                };
                (target, range)
            })
    } else {
        None
    };
    let drag_range = drag.map(|(_, range)| range);
    let interface = app.library.interface;
    let colors = interface.control_colors();
    let painter = ui.painter_at(rect);
    if select_loop {
        beat_map::paint(app, &painter, rect, visible.clone());
    }
    if select_loop && let Some(markers) = app.studio.warp_markers.get(&app.active_source) {
        for (index, marker) in markers.iter().enumerate() {
            if visible.contains(&marker.source_seconds) {
                let x = at(marker.source_seconds);
                painter.vline(x, rect.y_range(), Stroke::new(1.4_f32, BLUE));
                painter.text(pos2(x + 3.0, rect.top() + 2.0), Align2::LEFT_TOP,
                    format!("W{}", index + 1), FontId::monospace(10.0), BLUE);
            }
        }
    }
    let cy = rect.center().y;
    if select_loop {
        for cut in &app.suggestions {
            if cut.start < visible_end && cut.end > visible_start {
                painter.line_segment(
                    [
                        pos2(at(cut.start.max(visible_start)), rect.bottom() - 2.0),
                        pos2(at(cut.end.min(visible_end)), rect.bottom() - 2.0),
                    ],
                    Stroke::new(3.0_f32, GOLD),
                );
            }
        }
    }
    if let Some((start, end)) = drag_range.or(loop_range)
        && start < visible_end
        && end > visible_start
    {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(at(start.max(visible_start)), rect.top()),
                pos2(at(end.min(visible_end)), rect.bottom()),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(0, 195, 133, 25),
        );
    }
    let stereo = select_loop && interface.stereo_waveform;
    let centers = if stereo {
        [cy - rect.height() * 0.25, cy + rect.height() * 0.25]
    } else {
        [cy; 2]
    };
    for &center in centers.iter().take(if stereo { 2 } else { 1 }) {
        painter.hline(rect.x_range(), center, Stroke::new(1.0_f32, colors.edge));
    }
    let zoomed = select_loop && app.waveform.zoom > 1.0;
    let bins = if zoomed {
        rect.width().ceil().clamp(1.0, 1200.0) as usize
    } else {
        clip.peaks.len()
    };
    let unplayed = if interface.high_contrast {
        colors.secondary_text
    } else {
        Color32::from_rgb(85, 105, 96)
    };
    for index in 0..bins {
        let x = rect.left() + (index as f32 + 0.5) / bins as f32 * rect.width();
        let color = if x < at(app.position) { MINT } else { unplayed };
        let start = visible_start + span * index as f64 / bins as f64;
        let end = visible_start + span * (index + 1) as f64 / bins as f64;
        let peaks = if stereo {
            clip.stereo_peak_between(start, end)
        } else {
            let peak = if zoomed { clip.peak_between(start, end) } else { clip.peaks[index] };
            [peak; 2]
        };
        let scale = rect.height() * if stereo { 0.21 } else { 0.43 };
        for (&center, peak) in centers.iter().zip(peaks).take(if stereo { 2 } else { 1 }) {
            let amplitude = (peak.sqrt() * scale).max(0.8);
            painter.vline(x, center - amplitude..=center + amplitude, Stroke::new(1.0_f32, color));
        }
    }
    if stereo {
        for (name, y) in [("L", rect.top() + 16.0), ("R", cy + 4.0)] {
            painter.text(pos2(rect.left() + 4.0, y), Align2::LEFT_TOP, name, FontId::monospace(11.0), TEXT);
        }
    }
    let markers = if app.studio.loop_source == app.active_source {
        [app.studio.loop_in, app.studio.loop_out]
    } else {
        [None, None]
    };
    let points = drag_range.map_or(markers, |(start, end)| {
        [Some(start), Some(end)]
    });
    for point in points
        .into_iter()
        .flatten()
        .filter(|point| visible.contains(point))
    {
        let x = at(point);
        painter.vline(x, rect.y_range(), Stroke::new(colors.emphasis, colors.accent));
        if select_loop && (loop_range.is_some() || drag_range.is_some()) {
            painter.rect_filled(
                Rect::from_center_size(pos2(x, rect.top() + 6.0), vec2(10.0, 12.0)),
                2.0,
                colors.accent,
            );
        }
    }
    for (pad, point) in app.studio.pads.iter().enumerate() {
        let Some(point) = point.filter(|point| {
            app.studio.pad_sources[pad] == app.active_source && visible.contains(point)
        }) else { continue };
        let x = at(point);
        painter.vline(x, rect.top()..=rect.top() + 9.0, Stroke::new(1.0_f32, BLUE));
        if select_loop && pad == app.selected_pad {
            painter.text(pos2(x, rect.top() + 12.0), Align2::CENTER_TOP, format!("P{}", pad + 1), FontId::monospace(11.0), BLUE);
        }
    }
    if visible.contains(&app.position) {
        painter.vline(at(app.position), rect.y_range(), Stroke::new(colors.emphasis.max(1.5), colors.focus));
    }
    if select_loop && response.drag_stopped() {
        ui.data_mut(|data| data.remove::<WaveDrag>(selection_id));
        if let Some((target, range)) =
            drag.filter(|(_, (start, end))| end - start >= MIN_LOOP_SECONDS - 1e-9)
        {
            app.set_loop_range(range);
            match target {
                WaveDrag::Select { .. } => app.seek(range.0),
                WaveDrag::TrimIn { .. } if !app.playing => {
                    if app.output.is_none() || (app.studio.looping && app.preview) {
                        app.seek(range.0);
                    } else {
                        app.audition_range(range);
                    }
                }
                _ => {}
            }
        }
    } else if (response.clicked() || (!select_loop && response.dragged()))
        && let Some(time) = pointer_time
    {
        app.seek(time);
    }
}

fn pads(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(label("PADS", 13.0, DIM));
            if pill(ui, "Play with keyboard   `", app.keyboard).clicked() { app.keyboard = !app.keyboard; }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for (mode, title, tooltip) in [
                    (PadMode::Jump, "Jump", "Start at the chop and keep playing"),
                    (PadMode::Gate, "Gate", "Play while held"),
                    (PadMode::Slice, "Slice", "Stop at the next chop"),
                ] {
                    if pill(ui, title, app.pad_mode == mode).on_hover_text(tooltip).clicked() { app.pad_mode = mode; }
                }
            });
        });
        let max_size = if app.studio_view == StudioView::Sample { 70.0 } else { 101.0 };
        let pad_size = ((ui.available_width() - 24.0) / 4.0).clamp(48.0, max_size);
        let grid_width = 4.0 * pad_size + 3.0 * 8.0;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - grid_width) * 0.5 - 8.0).max(0.0));
            ui.vertical(|ui| {
                for row in (0..4).rev() {
                    ui.horizontal(|ui| {
                        for column in 0..4 {
                            let index = row * 4 + column;
                            pad_cell(app, ui, index, pad_size);
                        }
                    });
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            slice_menu(app, ui);
            if action(ui, &format!("Clear pad {}", app.selected_pad + 1), app.studio.pads[app.selected_pad].is_some()).clicked() {
                app.clear_pad(app.selected_pad);
            }
            let count = app.studio.pads.iter().flatten().count();
            if action(ui, "Clear all", count > 0).clicked() {
                for index in 0..STEPS { app.clear_pad(index); }
            }
            small(ui, format!("{count} / 16 chops"));
            let selected = app.pad_range(app.selected_pad);
            if action(ui, "Export pad WAV", selected.is_some() && app.busy.is_none()).clicked()
                && let Some(range) = selected
            {
                app.export_source_region(app.studio.pad_sources[app.selected_pad], range, &format!("pad-{:02}.wav", app.selected_pad + 1));
            }
        });
        if app.studio.pads[app.selected_pad].is_some() {
            ui.menu_button("Move or copy pad…", |ui| {
                let from = app.selected_pad;
                let range = app.pad_range(from);
                for to in (0..STEPS).filter(|&pad| pad != from) {
                    ui.horizontal(|ui| {
                        let assigned = app.studio.pads[to].is_some();
                        let verb = if assigned { "Swap with" } else { "Move to" };
                        if action(ui, &format!("{verb} pad {}", to + 1), true).clicked() {
                            app.swap_pads(from, to);
                            ui.close_menu();
                        }
                        if action(ui, "Copy", !assigned && range.is_some()).clicked()
                            && let Some(range) = range
                        {
                            app.assign_pad_range(to, app.studio.pad_sources[from], range);
                            app.studio.pad_names[to] = app.studio.pad_names[from].clone();
                            app.studio.pad_colors[to] = app.studio.pad_colors[from];
                            app.studio.pad_controls[to] = app.studio.pad_controls[from];
                            app.studio.choke_groups.pads[to] = app.studio.choke_groups.pads[from];
                            if let Some(layers) = app.studio.instruments.pads.get(&from).copied() {
                                app.studio.instruments.pads.insert(to, layers);
                            }
                            app.selected_pad = to;
                            ui.close_menu();
                        }
                    });
                }
            }).response.on_hover_text("Move sample assignments and sound settings. Sequencer steps keep their pad numbers.");
        }
        if action(ui, "Resample selected pad", app.can_add_source() && !app.recording
            && app.pad_range(app.selected_pad).is_some() && app.studio.pads.iter().any(Option::is_none))
            .on_hover_text("Render this pad's sound settings into a new source and assign it to an empty pad")
            .clicked()
        {
            app.resample_selected_pad();
        }
        small(ui, "Empty pad sets a marker. Shift-click reassigns. Drop a saved cut onto a pad.");
        ui.separator();
        let pad = app.selected_pad;
        let duration = app.source_duration(app.studio.pad_sources[pad]);
        ui.label(label(format!("Pad {:02} · {}", pad + 1, if app.studio.pads[pad].is_some() { "Assigned" } else { "Empty" }), 15.0, MINT).strong());
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Assign selection", app.selection().is_some())
                .on_hover_text("Assign the waveform selection to the selected pad").clicked()
                && let Some((start, end)) = app.selection()
            {
                app.assign_pad_range(pad, app.active_source, (start, end));
            }
            if let Some((start, end)) = app.pad_range(pad) {
                let mut linked = app.studio.pad_ends[pad].is_none();
                if ui.checkbox(&mut linked, "Link end to next marker").changed() {
                    app.studio.pad_ends[pad] = (!linked).then_some(end);
                }
                if action(ui, "Show range", true).clicked() {
                    app.select_source(app.studio.pad_sources[pad]);
                    app.waveform.reveal((start, end), app.duration());
                    app.set_loop_range((start, end));
                    app.studio_view = StudioView::Sample;
                }
            }
        });
        if app.studio.pads[pad].is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut app.studio.pad_names[pad])
                    .hint_text("Pad name")
                    .desired_width(128.0)
                    .char_limit(MAX_SAMPLE_NAME));
                let color = usize::from(app.studio.pad_colors[pad]).min(PAD_COLORS.len() - 1);
                ComboBox::from_id_salt("pad_color").selected_text(PAD_COLORS[color].0).show_ui(ui, |ui| {
                    for (index, (name, tint)) in PAD_COLORS.into_iter().enumerate() {
                        ui.selectable_value(&mut app.studio.pad_colors[pad], index as u8, label(name, 13.0, tint));
                    }
                });
                ui.menu_button("Copy to pad", |ui| {
                    if let Some((start, end)) = app.pad_range(pad) {
                        for target in 0..STEPS {
                            if target != pad && app.studio.pads[target].is_none()
                                && ui.button(format!("Pad {:02}", target + 1)).clicked()
                            {
                                app.assign_pad_range(target, app.studio.pad_sources[pad], (start, end));
                                app.studio.pad_names[target] = app.studio.pad_names[pad].clone();
                                app.studio.pad_colors[target] = app.studio.pad_colors[pad];
                                app.studio.pad_controls[target] = app.studio.pad_controls[pad];
                                app.studio.choke_groups.pads[target] = app.studio.choke_groups.pads[pad];
                                if let Some(layers) = app.studio.instruments.pads.get(&pad).copied() {
                                    app.studio.instruments.pads.insert(target, layers);
                                }
                                ui.close_menu();
                            }
                        }
                    }
                }).response.on_hover_text("Copy the current range and controls to an empty pad");
            });
            ui.horizontal_wrapped(|ui| {
                let last_start = app.studio.pad_ends[pad].map_or(duration, |end| (end - 0.001).max(0.0));
                time_control(ui, "Start", &mut app.studio.pads[pad], last_start);
            });
            if app.studio.pad_ends[pad].is_some() {
                ui.horizontal_wrapped(|ui| time_control(ui, "End", &mut app.studio.pad_ends[pad], duration));
            }
        }
        ui.add_enabled_ui(app.studio.pads[pad].is_some(), |ui| {
            sample_controls(ui, &mut app.studio.pad_controls[pad]);
            choke_group_control(ui, &mut app.studio.choke_groups.pads[pad]);
            let before = app.studio.pad_layers(pad);
            let mut layers = before;
            layers::edit(ui, app, &mut layers);
            if layers != before {
                app.studio.instruments.pads.insert(pad, layers);
            }
        });
    });
}

fn slice_menu(app: &mut App, ui: &mut Ui) {
    ui.add_enabled_ui(app.selection().is_some(), |ui| {
        ui.menu_button("Slice selection…", |ui| {
            if sample::slice_controls(app, ui) {
                ui.close_menu();
            }
        });
    }).response.on_disabled_hover_text("Select a waveform range first");
}

fn sample_controls(ui: &mut Ui, controls: &mut SampleControls) -> bool {
    let mut reset = false;
    ui.horizontal_wrapped(|ui| {
        small(ui, "SAMPLE");
        ui.add(
            DragValue::new(&mut controls.gain)
                .range(0.0..=2.0)
                .speed(0.01)
                .prefix("Gain ×"),
        );
        ui.add(
            DragValue::new(&mut controls.pan)
                .range(-1.0..=1.0)
                .speed(0.01)
                .prefix("Pan "),
        );
        ui.add(
            DragValue::new(&mut controls.tune)
                .range(-24.0..=24.0)
                .speed(0.1)
                .prefix("Tune ")
                .suffix(" st"),
        );
        ui.checkbox(&mut controls.reverse, "Reverse");
        ui.add(
            DragValue::new(&mut controls.attack_ms)
                .range(0..=1000)
                .prefix("Attack ")
                .suffix(" ms"),
        );
        ui.add(
            DragValue::new(&mut controls.release_ms)
                .range(0..=1000)
                .prefix("Release ")
                .suffix(" ms"),
        )
        .on_hover_text("Fade time after you release a key or pad in Gate mode");
        if action(ui, "Reset controls", true).clicked() {
            *controls = SampleControls::default();
            reset = true;
        }
    });
    reset
}

fn piano(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(label("CHOP PIANO  ·  A0–C8", 13.0, DIM));
            if pill(ui, "Play with keyboard   `", app.keyboard).clicked() {
                app.keyboard = !app.keyboard;
            }
            if app.keyboard {
                ui.add(DragValue::new(&mut app.piano_octave).range(0..=7).prefix("Octave "));
            }
            if pill(ui, "One shot", !app.piano_gate).clicked() {
                app.piano_gate = false;
            }
            if pill(ui, "Gate", app.piano_gate).clicked() {
                app.piano_gate = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut chromatic = app.studio.chromatic_enabled;
            ui.selectable_value(&mut chromatic, false, "Chops per key");
            ui.selectable_value(&mut chromatic, true, "Chromatic sample");
            if chromatic != app.studio.chromatic_enabled {
                app.midi_panic();
                app.piano_arm = false;
                app.studio.chromatic_enabled = chromatic;
            }
        });
        let zoom_id = ui.id().with("key_zoom");
        let mut zoom = ui.data(|data| data.get_temp::<f32>(zoom_id)).unwrap_or(2.0);
        ui.add(DragValue::new(&mut zoom).range(1.0..=4.0).fixed_decimals(1).prefix("Key zoom ").suffix("×"));
        ui.data_mut(|data| data.insert_temp(zoom_id, zoom));
        ui.horizontal_wrapped(|ui| {
            let key = ui.add(DragValue::new(&mut app.piano_key).range(PIANO_LOW..=PIANO_HIGH).prefix("Selected MIDI key "));
            named_number(key, "Selected MIDI key", f64::from(app.piano_key));
            small(ui, piano_note(app.piano_key));
        });
        let key_width = (ui.available_width() * zoom).max(360.0);
        ScrollArea::horizontal().id_salt("piano_keys").show(ui, |ui| {
            ui.set_min_width(key_width);
            draw_piano_keys(app, ui);
        });
        if app.studio.chromatic_enabled {
            chromatic_controls(app, ui);
            return;
        }
        let note = app.piano_key;
        let selection = app.selection();
        let current = app.key_range(note);
        ui.horizontal_wrapped(|ui| {
            ui.label(label(format!("{} · MIDI {note}", piano_note(note)), 14.0, MINT).strong());
            if let Some((start, end)) = current {
                small(ui, format!("{} – {}", time(start, true), time(end, true)));
                if action(ui, "Show on waveform", true).clicked() {
                    app.select_source(app.studio.key_source(note));
                    app.waveform.reveal((start, end), app.duration());
                    app.set_loop_range((start, end));
                    app.seek(start);
                    app.studio_view = StudioView::Sample;
                }
            } else {
                small(ui, "No chop assigned");
            }
            if action(ui, "Assign selection", selection.is_some()).clicked()
                && let Some(range) = selection
            {
                app.assign_piano(range);
            }
            if ui.add_enabled_ui(selection.is_some(), |ui| pill(ui, "Assign next key", app.piano_arm)).inner.clicked() {
                app.piano_arm = !app.piano_arm;
            }
            let mut from_pad = None;
            ComboBox::from_id_salt("assign_key_pad").selected_text("Copy pad chop…").show_ui(ui, |ui| {
                for pad in 0..STEPS {
                    let range = app.pad_range(pad);
                    if action(ui, &format!("Pad {}", pad + 1), range.is_some()).clicked()
                        && let Some(range) = range
                    {
                        from_pad = Some((pad, range));
                        ui.close_menu();
                    }
                }
            });
            if let Some((pad, range)) = from_pad {
                app.assign_piano_source(app.studio.pad_sources[pad], range);
                app.studio.key_controls.insert(note, app.studio.pad_controls[pad]);
            }
            let mut from_sample = None;
            ComboBox::from_id_salt("assign_key_sample").selected_text("Copy named cut…").show_ui(ui, |ui| {
                for sample in &app.samples {
                    if ui.selectable_label(false, &sample.name).clicked() {
                        from_sample = Some((sample.source, sample.range()));
                        ui.close_menu();
                    }
                }
                if app.samples.is_empty() {
                    small(ui, "Save a cut in Samples first.");
                }
            });
            if let Some((source, range)) = from_sample {
                app.assign_piano_source(source, range);
            }
            if action(ui, "Clear key", current.is_some()).clicked() {
                app.studio.key_chops.remove(&note);
                app.studio.key_sources.remove(&note);
                if Studio::pad_for_key(note).is_some() {
                    app.studio.key_chops.insert(note, KeyChop::Muted);
                }
                app.studio.key_controls.remove(&note);
                app.studio.instruments.keys.remove(&note);
            }
            if Studio::pad_for_key(note).is_some()
                && app.studio.key_chops.contains_key(&note)
                && action(ui, "Use pad default", true).clicked()
            {
                app.studio.key_chops.remove(&note);
                app.studio.key_sources.remove(&note);
                app.studio.key_controls.remove(&note);
                app.studio.instruments.keys.remove(&note);
            }
        });
        if app.piano_arm {
            small(ui, "Play any key on screen or on your controller to assign the waveform selection.");
        } else {
            small(ui, "C4–D♯5 mirror pads 1–16 until you assign your own chops. Every other key is assignable too.");
        }
        if app.keyboard {
            small(ui, "Computer keys  A W S E D F T G Y H U J K O L P ;  play consecutive semitones.");
        }
        ui.separator();
        let previous = app.studio.controls_for_key(note);
        let mut controls = previous;
        let reset = ui.add_enabled_ui(current.is_some(), |ui| sample_controls(ui, &mut controls)).inner;
        if reset {
            app.studio.key_controls.remove(&note);
        } else if controls != previous {
            app.studio.key_controls.insert(note, controls);
        }
        if current.is_some() {
            let before = app.studio.key_layers(note);
            let mut layers = before;
            layers::edit(ui, app, &mut layers);
            if layers != before {
                app.studio.instruments.keys.insert(note, layers);
            }
            if app.studio.instruments.keys.contains_key(&note)
                && action(ui, "Reset key layers", true).clicked()
            {
                app.studio.instruments.keys.remove(&note);
            }
        }
    });
}

fn chromatic_controls(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        let harmony = &mut app.studio.harmony;
        ComboBox::from_id_salt("harmony_root").selected_text(crate::harmony::ROOTS[usize::from(harmony.root) % 12])
            .show_ui(ui, |ui| {
                for (index, name) in crate::harmony::ROOTS.into_iter().enumerate() {
                    ui.selectable_value(&mut harmony.root, index as u8, name);
                }
            });
        ComboBox::from_id_salt("harmony_scale").selected_text(harmony.scale.label()).show_ui(ui, |ui| {
            for scale in crate::harmony::Scale::ALL {
                ui.selectable_value(&mut harmony.scale, scale, scale.label());
            }
        });
        ui.checkbox(&mut harmony.constrain, "Snap keys to scale");
        ComboBox::from_id_salt("harmony_chord").selected_text(harmony.chord.label()).show_ui(ui, |ui| {
            for chord in crate::harmony::Chord::ALL {
                ui.selectable_value(&mut harmony.chord, chord, chord.label());
            }
        });
        ui.add_enabled_ui(app.studio.chromatic.is_some() && !app.recording, |ui| {
            ui.menu_button("Write arpeggio…", |ui| {
                for (title, up, grid) in [
                    ("Up · 1/8", true, Quantize::Eighth),
                    ("Down · 1/16", false, Quantize::Sixteenth),
                    ("Up · 1/8 triplet", true, Quantize::EighthTriplet),
                ] {
                    if ui.button(title).clicked() {
                        app.write_arpeggio(up, grid);
                        ui.close_menu();
                    }
                }
                small(ui, format!("Writes notes into bar {}", app.bar + 1));
            });
        });
    });
    let selection = app.selection();
    if action(ui, "Use waveform selection", selection.is_some()).clicked()
        && let Some((start, end)) = selection
    {
        let previous = app.studio.chromatic;
        app.studio.chromatic = Some(crate::model::ChromaticSample {
            source: app.active_source, start, end,
            root: previous.map_or(60, |sample| sample.root),
            controls: previous.map_or_else(SampleControls::default, |sample| sample.controls),
        });
    }
    if let Some(sample) = &mut app.studio.chromatic {
        ui.horizontal_wrapped(|ui| {
            ui.label("Root note");
            ComboBox::from_id_salt("chromatic_root").selected_text(piano_note(sample.root)).show_ui(ui, |ui| {
                for note in PIANO_LOW..=PIANO_HIGH {
                    ui.selectable_value(&mut sample.root, note, piano_note(note));
                }
            });
            small(ui, format!("{} to {}", time(sample.start, true), time(sample.end, true)));
        });
        sample_controls(ui, &mut sample.controls);
        let mut layers = app.studio.instruments.chromatic;
        layers::edit(ui, app, &mut layers);
        app.studio.instruments.chromatic = layers;
        let glide = &mut app.studio.instruments.glide;
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut glide.mono, "Mono / legato")
                .on_hover_text("The most recent active key plays. Releasing it returns to the previous key.");
            ui.add_enabled_ui(glide.mono, |ui| {
                ui.add(DragValue::new(&mut glide.milliseconds).range(0..=2000)
                    .prefix("Glide ").suffix(" ms"));
                ui.checkbox(&mut glide.retrigger, "Retrigger overlapping notes");
            });
        });
    } else {
        small(ui, "Assign a waveform selection. The root key plays it at its original pitch.");
    }
}

fn draw_piano_keys(app: &mut App, ui: &mut Ui) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 109.0), Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "88-key chop piano"));
    let width = rect.width() / 52.0;
    let mut whites = Vec::with_capacity(52);
    let mut blacks = Vec::with_capacity(36);
    for note in PIANO_LOW..=PIANO_HIGH {
        let x = rect.left() + whites.len() as f32 * width;
        if matches!(note % 12, 1 | 3 | 6 | 8 | 10) {
            let black =
                Rect::from_min_size(pos2(x - width * 0.31, rect.top()), vec2(width * 0.62, 66.0));
            blacks.push((note, black));
        } else {
            whites.push((
                note,
                Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height())),
            ));
        }
    }
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let hovered = pointer.and_then(|pos| {
        blacks
            .iter()
            .chain(&whites)
            .find(|(_, key)| key.contains(pos))
            .map(|&(note, _)| note)
    });
    let down =
        response.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_down());
    let active = if down { hovered } else { None };
    let previous = app.piano_pointer;
    if active != app.piano_pointer {
        if let Some(note) = app.piano_pointer {
            app.piano_up(0, note);
        }
        app.piano_pointer = active;
        if let Some(note) = active {
            app.piano_down(0, note, 127);
        }
    }
    if active.is_none()
        && previous.is_none()
        && response.clicked()
        && let Some(note) = hovered
    {
        app.piano_down(0, note, 127);
        app.piano_up(0, note);
    }
    let painter = ui.painter_at(rect);
    for (black, keys) in [(false, &whites), (true, &blacks)] {
        for &(note, key) in keys {
            let assigned = app.key_range(note).is_some();
            let fill = match (app.piano_pressed(note), assigned, black) {
                (true, _, _) => GREEN,
                (_, true, true) => Color32::from_rgb(25, 82, 66),
                (_, true, false) => Color32::from_rgb(184, 226, 211),
                (_, false, true) => Color32::from_rgb(32, 29, 30),
                (_, false, false) => Color32::from_rgb(231, 223, 213),
            };
            let in_scale = app.studio.chromatic_enabled
                && app.studio.harmony.scale != crate::harmony::Scale::Chromatic
                && app.studio.harmony.contains(note);
            let outline = if app.piano_key == note { GOLD } else if in_scale { BLUE } else { EDGE };
            painter.rect(
                key,
                1.5,
                fill,
                Stroke::new(1.0_f32, outline),
                StrokeKind::Inside,
            );
            if !black && (note % 12 == 0 || note == PIANO_LOW) {
                painter.text(
                    key.center_bottom() - vec2(0.0, 4.0),
                    Align2::CENTER_BOTTOM,
                    piano_note(note),
                    FontId::monospace(8.0),
                    BG,
                );
            }
        }
    }
    if let Some(note) = hovered {
        if let Some(cut) = response.dnd_release_payload::<CutDrag>() {
            app.piano_key = note;
            if app.studio.chromatic_enabled {
                app.studio.chromatic = Some(crate::model::ChromaticSample {
                    source: cut.source, start: cut.range.0, end: cut.range.1,
                    root: note, controls: SampleControls::default(),
                });
            } else {
                app.assign_piano_source(cut.source, cut.range);
            }
        }
        response.on_hover_text(format!("{} · MIDI {note}", piano_note(note)));
    }
}

fn pad_cell(app: &mut App, ui: &mut Ui, index: usize, size: f32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let interface = app.library.interface;
    let colors = interface.control_colors();
    let assigned = app.studio.pads[index];
    response.widget_info(|| {
        WidgetInfo::labeled(
            WidgetType::Button,
            true,
            format!(
                "Pad {} {}",
                index + 1,
                assigned.map_or_else(|| "empty".into(), |value| time(value, true))
            ),
        )
    });
    let selected = app.selected_pad == index;
    let name = &app.studio.pad_names[index];
    let tint = PAD_COLORS[usize::from(app.studio.pad_colors[index]).min(PAD_COLORS.len() - 1)].1;
    let drop_target = response.dnd_hover_payload::<CutDrag>().is_some();
    let held = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    let hold_id = response.id.with("held");
    let was_held = ui.data_mut(|data| {
        let previous = data.get_temp::<bool>(hold_id).unwrap_or(false);
        data.insert_temp(hold_id, held);
        previous
    });
    let fill = if held {
        GREEN_BG
    } else if assigned.is_some() {
        Color32::from_rgb(14, 36, 32)
    } else if response.hovered() {
        Color32::from_rgb(37, 32, 29)
    } else {
        INSET
    };
    let border = if response.has_focus() {
        Stroke::new(colors.emphasis, colors.focus)
    } else if drop_target {
        Stroke::new(colors.emphasis, GOLD)
    } else if selected {
        Stroke::new(colors.emphasis, colors.accent)
    } else {
        Stroke::new(1.0_f32, colors.edge)
    };
    let p = ui.painter_at(rect);
    p.rect(
        rect,
        6.0,
        fill,
        border,
        StrokeKind::Inside,
    );
    p.text(
        rect.left_top() + vec2(8.0, 7.0),
        Align2::LEFT_TOP,
        format!("{:02}", index + 1),
        FontId::monospace(11.0),
        if assigned.is_some() { tint } else { colors.secondary_text },
    );
    let cap = Rect::from_min_size(
        pos2(rect.right() - 25.0, rect.top() + 7.0),
        vec2(17.0, 17.0),
    );
    p.rect(
        cap,
        4.0,
        Color32::from_black_alpha(34),
        Stroke::new(1.0_f32, colors.edge),
        StrokeKind::Inside,
    );
    p.text(
        cap.center(),
        Align2::CENTER_CENTER,
        PAD_LABELS[index],
        FontId::monospace(11.0),
        colors.secondary_text,
    );
    if let Some(clip) = app.clip.as_deref().and_then(|root| root.source(app.studio.pad_sources[index]))
        && let Some((start, end)) = app.pad_range(index)
    {
        let waveform = rect.shrink2(vec2(8.0, 25.0));
        let bins = 18;
        let trace = if interface.high_contrast { tint } else { tint.gamma_multiply(0.75) };
        for bin in 0..bins {
            let low = start + (end - start) * f64::from(bin) / f64::from(bins);
            let high = start + (end - start) * f64::from(bin + 1) / f64::from(bins);
            let x = waveform.left() + waveform.width() * (bin as f32 + 0.5) / bins as f32;
            let half = clip.peak_between(low, high) * waveform.height().max(0.0) * 0.42;
            p.vline(x, waveform.center().y - half..=waveform.center().y + half, Stroke::new(1.0_f32, trace));
        }
    }
    p.text(
        rect.left_bottom() + vec2(8.0, -8.0),
        Align2::LEFT_BOTTOM,
        if name.is_empty() {
            assigned.map_or_else(|| "—".into(), |position| time(position, true))
        } else {
            name.chars().take(if size < 75.0 { 8 } else { 13 }).collect()
        },
        FontId::monospace(11.0),
        if assigned.is_some() { MINT } else if interface.high_contrast { colors.secondary_text } else { FAINT },
    );
    if let Some(cut) = response.dnd_release_payload::<CutDrag>() {
        app.selected_pad = index;
        app.assign_pad_range(index, cut.source, cut.range);
        app.studio.pad_names[index] = cut.name.clone();
    }
    response.context_menu(|ui| {
        if action(ui, "Select pad", true).clicked() {
            app.selected_pad = index;
            ui.close_menu();
        }
        if action(ui, "Clear pad", assigned.is_some()).clicked() {
            app.clear_pad(index);
            ui.close_menu();
        }
    });
    let shift = ui.input(|i| i.modifiers.shift);
    if !was_held && (held || response.clicked()) {
        app.pad(index, shift);
    }
    if app.pad_mode == PadMode::Gate && !held && (was_held || response.clicked()) {
        app.send(Command::Release(index));
    }
}

fn project_history(app: &mut App, ui: &mut Ui) {
    for (command, title) in [(Action::Snapshots, "Snapshots…"), (Action::Undo, "Undo"), (Action::Redo, "Redo")] {
        if command_button(ui, app, command, title).clicked() {
            command.run(app);
        }
    }
}

fn mixer_panel(app: &mut App, ui: &mut Ui) {
    panel().show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let width = (ui.available_width() - 20.0) / 2.0;
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(width);
                ui.label(label("DRUM FX", 12.0, DIM).monospace());
                ui.checkbox(&mut app.studio.drum_fx_bypass, "Bypass drum effects");
                ui.add_enabled_ui(!app.studio.drum_fx_bypass, |ui| {
                    fx(ui, "Reverb", &mut app.studio.reverb);
                    fx(ui, "Delay", &mut app.studio.delay);
                    fx(ui, "Filter", &mut app.studio.filter);
                });
                let division = &mut app.studio.delay_division;
                let divisions = crate::model::DelayDivision::ALL;
                let label = divisions.iter().find(|(value, _)| value == division).unwrap().1;
                ComboBox::from_id_salt("delay_division").selected_text(label)
                    .show_ui(ui, |ui| {
                        for (value, name) in divisions {
                            ui.selectable_value(division, value, name);
                        }
                    }).response.on_hover_text("Tempo-synced delay for drum and arrangement effects");
                if action(ui, "Reset drum effects", true).clicked() {
                    app.studio.reverb = 0.0;
                    app.studio.delay = 0.0;
                    app.studio.filter = 0.0;
                    app.studio.drum_fx_bypass = false;
                }
                ui.checkbox(&mut app.studio.mix_fx_bypass, "Bypass arrangement effects")
                    .on_hover_text("Bypass automated delay, reverb, and filtering on the sample and drum buses. Volume automation stays active.");
            });
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(width);
                ui.label(label("MIX & TIMING", 12.0, DIM).monospace());
                fx(ui, "Drums", &mut app.studio.drum_level);
                fx(ui, "Sample", &mut app.studio.sample_level);
                fx(ui, "Swing", &mut app.studio.swing);
            });
        });
        ui.separator();
        let reveal = std::mem::take(&mut app.reveal_audio_output);
        let output = egui::CollapsingHeader::new("Audio output")
            .open(reveal.then_some(true))
            .show(ui, |ui| audio_output_controls(app, ui));
        if reveal {
            output.header_response.scroll_to_me(Some(Align::Center));
        }
        ui.collapsing("Drum channels", |ui| {
                ui.horizontal_wrapped(|ui| {
                    let ready = app.busy.is_none() && app.input_recording.is_none();
                    if action(ui, "Save drum kit…", ready).clicked() {
                        app.save_drum_kit();
                    }
                    if action(ui, "Load drum kit…", ready && !app.recording).clicked() {
                        app.load_drum_kit();
                    }
                });
                small(ui, "Add drum audio in Sample. Unassigned channels use the selected kit.");
                for (index, name) in DRUMS.into_iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if action(ui, name, app.output.is_some()).on_hover_text("Audition this drum").clicked() {
                                app.sync();
                                app.send(Command::Drum(index));
                            }
                            let mut source = app.studio.drum_sources[index];
                            let title = source.and_then(|source| app.source_record(source))
                                .map_or("Built-in sound", |record| record.title.as_str());
                            ComboBox::from_id_salt("drum_source").selected_text(title).show_ui(ui, |ui| {
                                ui.selectable_value(&mut source, None, "Built-in sound");
                                for source_index in 0..=app.studio.sources.len() {
                                    if let Some(record) = app.source_record(source_index) {
                                        ui.selectable_value(&mut source, Some(source_index), &record.title);
                                    }
                                }
                            });
                            app.studio.drum_sources[index] = source;
                            let strip = &mut app.studio.drum_mix[index];
                            ui.add(DragValue::new(&mut strip.gain).range(0.0..=2.0).speed(0.01).prefix("Gain ×"));
                            ui.add(DragValue::new(&mut strip.pan).range(-1.0..=1.0).speed(0.01).prefix("Pan "));
                            ui.add(DragValue::new(&mut strip.tune).range(-24.0..=24.0).speed(0.1).suffix(" st"));
                            ui.checkbox(&mut strip.mute, "Mute");
                            ui.checkbox(&mut strip.solo, "Solo");
                            choke_group_control(ui, &mut app.studio.choke_groups.drums[index]);
                            if action(ui, "Reset", true).clicked() {
                                *strip = Default::default();
                                app.studio.choke_groups.drums[index] = 0;
                            }
                        });
                    });
                }
        });
    });
}

fn choke_group_control(ui: &mut Ui, group: &mut u8) {
    let text = if *group == 0 { "Choke off".into() } else { format!("Choke {group}") };
    ComboBox::from_id_salt("choke_group")
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(group, 0, "Off");
            for number in 1..=crate::model::ChokeGroups::MAX {
                ui.selectable_value(group, number, format!("Group {number}"));
            }
        })
        .response
        .on_hover_text("A hit fades out pads and drums already sounding in the same group.");
}

fn audio_output_controls(app: &mut App, ui: &mut Ui) {
    let draft_id = ui.id().with("output_settings");
    let mut draft = ui.data(|data| data.get_temp::<crate::audio::OutputSettings>(draft_id))
        .unwrap_or_else(|| app.library.audio.clone());
    if let Some(output) = &app.output {
        let latency = output.requested_buffer.map_or_else(
            || "system buffer".into(),
            |frames| format!("requested buffer {:.1} ms", 1_000.0 * f64::from(frames) / f64::from(output.sample_rate)),
        );
        small(ui, format!("{} · {} Hz · {latency}", output.device_name, output.sample_rate));
    }
    ui.horizontal_wrapped(|ui| {
        ComboBox::from_id_salt("output_device").selected_text(draft.device.as_deref().unwrap_or("System default"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.device, None, "System default");
                for name in crate::audio::output_devices() {
                    ui.selectable_value(&mut draft.device, Some(name.clone()), name);
                }
            });
        ComboBox::from_id_salt("output_rate")
            .selected_text(draft.rate.map_or_else(|| "Device rate".into(), |rate| format!("{rate} Hz")))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.rate, None, "Device rate");
                for rate in [44_100, 48_000, 88_200, 96_000] {
                    ui.selectable_value(&mut draft.rate, Some(rate), format!("{rate} Hz"));
                }
            });
        ComboBox::from_id_salt("output_buffer")
            .selected_text(draft.buffer.map_or_else(|| "Device buffer".into(), |frames| format!("{frames} frames")))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.buffer, None, "Device buffer");
                for frames in [128, 256, 512, 1024, 2048] {
                    ui.selectable_value(&mut draft.buffer, Some(frames), format!("{frames} frames"));
                }
            });
        if action(ui, "Apply output settings", !app.recording).clicked() {
            app.set_audio_output(draft.clone());
        }
    });
    ui.data_mut(|data| data.insert_temp(draft_id, draft));
}

fn fx(ui: &mut Ui, name: &str, value: &mut f32) {
    ui.horizontal(|ui| {
        ui.add_sized([60.0, 24.0], Label::new(label(name, 12.0, TEXT)));
        ui.spacing_mut().slider_width = (ui.available_width() - 70.0).clamp(75.0, 255.0);
        let mut percent = (*value * 100.0).round() as u32;
        if ui
            .add(egui::Slider::new(&mut percent, 0..=100).suffix("%"))
            .changed()
        {
            *value = percent as f32 / 100.0;
        }
    });
}

fn views_label(count: u64) -> String {
    let digits = count.to_string();
    let mut formatted = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    format!("{formatted} views")
}

fn time(seconds: f64, precision: bool) -> String {
    let millis = (seconds * 1000.0).round() as u64;
    let (minutes, seconds) = (millis / 60_000, millis / 1000 % 60);
    if precision {
        format!("{minutes}:{seconds:02}.{:03}", millis % 1000)
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Clip;
    use std::sync::Arc;

    #[test]
    fn the_first_and_last_piano_keys_take_distinct_waveform_assignments() {
        let context = Context::default();
        let mut app = App::new(context.clone());
        app.output = None;
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 16_000])));
        let render = |app: &mut App, events| {
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1040.0, 200.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(Frame::NONE)
                        .show(ctx, |ui| {
                            draw_piano_keys(app, ui);
                        });
                },
            );
        };
        render(&mut app, vec![]);
        for (x, note, range) in [(10.0, 21, (0.1, 0.3)), (1030.0, 108, (0.8, 1.1))] {
            app.piano_arm = true;
            app.set_loop_range(range);
            let pos = pos2(x, 90.0);
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            render(&mut app, vec![egui::Event::PointerMoved(pos), button(true)]);
            render(&mut app, vec![button(false)]);
            assert_eq!(app.piano_key, note);
            assert!(!app.piano_arm);
            assert_eq!(app.studio.key_range(note, 2.0), Some(range));
        }
        assert_eq!(app.studio.key_range(21, 2.0), Some((0.1, 0.3)));
    }

    #[test]
    fn dragging_the_studio_waveform_sets_the_loop() {
        let context = Context::default();
        let mut app = App::new(context.clone());
        app.output = None;
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 80_000])));
        drag_waveform(&context, &mut app, 85.0, 35.0);
        assert_eq!(app.studio.loop_range(), Some((2.5, 7.5)));
        assert_eq!(app.position, 2.5);
    }

    #[test]
    fn zoomed_handles_trim_independently_and_cannot_cross() {
        let context = Context::default();
        let mut app = App::new(context.clone());
        app.output = None;
        app.clip = Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 80_000])));
        app.waveform = WaveformView {
            start: 2.5,
            zoom: 2.0,
        };
        app.set_loop_range((3.75, 6.25));
        drag_waveform(&context, &mut app, 35.0, 60.0);
        assert_eq!(app.studio.loop_range(), Some((5.0, 6.25)));
        assert_eq!(app.position, 5.0);
        drag_waveform(&context, &mut app, 85.0, 35.0);
        let (start, end) = app
            .studio
            .loop_range()
            .expect("handles must leave a valid loop");
        assert_eq!(start, 5.0);
        assert!((end - start - MIN_LOOP_SECONDS).abs() < 1e-9);
        assert_eq!(app.position, 5.0);
    }

    fn drag_waveform(context: &Context, app: &mut App, from: f32, to: f32) {
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 60.0));
        let from = pos2(from, 30.0);
        let to = pos2(to, 30.0);
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(from), press(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![press(to, false)],
        ] {
            let _ = context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        timeline_at(app, ui, rect, "waveform_test", true);
                    });
                },
            );
        }
    }
}
