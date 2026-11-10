use super::{
    GOLD, PANEL, action, beat_map, command_button, extraction, groove, label, loop_panel, panel,
    performance_panel, pill, primary, samples_panel, small, time,
};
use crate::{
    app::{App, actions::Action},
    model::{MAX_NAMED_SAMPLES, STEPS},
    waveform::transient_near,
};
use eframe::egui::{self, ComboBox, DragValue, Frame, Margin, ScrollArea, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Tool {
    Pads,
    Slice,
    BeatMap,
    Warp,
    Groove,
    Transcribe,
    Extract,
    Stems,
    Suggestions,
    Input,
}

impl Tool {
    pub(crate) const TOOLS: [Self; 9] = [
        Self::Slice,
        Self::BeatMap,
        Self::Warp,
        Self::Groove,
        Self::Transcribe,
        Self::Extract,
        Self::Stems,
        Self::Suggestions,
        Self::Input,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Pads => "Pads & keys",
            Self::Slice => "Slice selection",
            Self::BeatMap => "Beat map",
            Self::Warp => "Warp & fit to bars",
            Self::Groove => "Groove transfer",
            Self::Transcribe => "Transcribe",
            Self::Extract => "Extract sound · Experimental",
            Self::Stems => "Stem separation",
            Self::Suggestions => "Suggested cuts",
            Self::Input => "Record audio input",
        }
    }
}

pub(super) fn show(app: &mut App, ui: &mut Ui) {
    let beside_waveform = app.library.interface.show_pads && ui.available_width() >= 980.0;
    if beside_waveform {
        let max = ui.available_width() * 0.56;
        let width = app.library.interface.pads_width.clamp(330.0, max);
        let inspector = egui::SidePanel::right("sample_performance")
            .default_width(width)
            .width_range(330.0..=max)
            .resizable(true)
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::same(8)))
            .show_inside(ui, |ui| {
                let selected = inspector_header(app, ui, Some(Tool::Pads));
                ui.separator();
                if let Some(tool) = selected {
                    ScrollArea::vertical()
                        .id_salt(("sample_inspector", tool))
                        .show(ui, |ui| inspector_body(app, ui, tool));
                }
            });
        if ui.input(|input| input.pointer.any_released())
            && (inspector.response.rect.width() - width).abs() > 2.0
        {
            app.library.interface.pads_width = inspector.response.rect.width();
            app.persist_library();
        }
    }
    ScrollArea::vertical()
        .id_salt("sample")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            source_picker(app, ui);
            let selected = if beside_waveform {
                None
            } else {
                inspector_header(app, ui, None)
            };
            loop_panel(app, ui);
            if app.warp_preview().is_some() {
                panel().show(ui, |ui| beat_map::warp_preview(app, ui));
            }
            if let Some(tool) = selected {
                panel().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    inspector_body(app, ui, tool);
                });
            }
            samples_panel(app, ui);
        });
}

fn inspector_header(app: &mut App, ui: &mut Ui, fallback: Option<Tool>) -> Option<Tool> {
    let mut selected = app.view.sample_tool.or(fallback);
    ui.horizontal_wrapped(|ui| {
        if pill(ui, Tool::Pads.title(), selected == Some(Tool::Pads)).clicked() {
            selected = Some(Tool::Pads);
        }
        ComboBox::from_id_salt("sample_tool")
            .selected_text("Sample tools")
            .show_ui(ui, |ui| {
                for tool in Tool::TOOLS {
                    let reason = Action::Sample(tool).unavailable(app);
                    let response = ui.add_enabled(
                        reason.is_none(),
                        egui::Button::new(tool.title()).selected(selected == Some(tool)),
                    );
                    if response.clicked() {
                        selected = Some(tool);
                    }
                    if let Some(reason) = reason {
                        response.on_disabled_hover_text(reason);
                    }
                }
            });
        if fallback.is_none() && selected.is_some() && action(ui, "Close inspector", true).clicked()
        {
            selected = None;
        }
    });
    if selected != app.view.sample_tool.or(fallback) {
        if let Some(tool) = selected {
            Action::Sample(tool).run(app);
        } else {
            app.view.sample_tool = None;
        }
    }
    selected
}

fn inspector_body(app: &mut App, ui: &mut Ui, tool: Tool) {
    if tool == Tool::Pads {
        performance_panel(app, ui);
        return;
    }
    ui.heading(tool.title());
    if let Some((start, end)) = app.selection() {
        small(
            ui,
            format!(
                "Selection {} – {} · {:.3} s",
                time(start, true),
                time(end, true),
                end - start
            ),
        );
    } else if let Some(record) = app.source_record(app.media.active_source()) {
        small(ui, format!("Source · {}", record.title));
    }
    match tool {
        Tool::Pads => unreachable!(),
        Tool::Slice => {
            slice_controls(app, ui);
        }
        Tool::BeatMap => beat_map::panel(app, ui),
        Tool::Warp => {
            if app.active_clip().is_some() {
                fit_to_bars(app, ui);
                warp_editor(app, ui);
            } else {
                small(ui, "Load a source to edit warp markers.");
            }
        }
        Tool::Groove => groove::panel(app, ui, true),
        Tool::Transcribe => transcription(app, ui),
        Tool::Extract => extraction::panel(app, ui),
        Tool::Stems => stem_separation(app, ui),
        Tool::Suggestions => suggestions(app, ui),
        Tool::Input => record_input(app, ui),
    }
}

fn transcription(app: &mut App, ui: &mut Ui) {
    small(
        ui,
        "Choose an isolated bass or melody passage. Basic Pitch runs in a local Python environment.",
    );
    small(ui, format!("Python · {}", app.pitch_python().display()));
    if action(ui, "Choose Basic Pitch Python…", true).clicked() {
        app.choose_pitch_python();
    }
    if command_button(ui, app, Action::Transcribe, Action::Transcribe.title()).clicked() {
        Action::Transcribe.run(app);
    }
}

fn source_picker(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        let selected = app
            .source_record(app.media.active_source())
            .map_or("No audio loaded", |record| record.title.as_str());
        let mut source = app.media.active_source();
        ComboBox::from_id_salt("project_source")
            .selected_text(selected)
            .width(ui.available_width().min(250.0))
            .show_ui(ui, |ui| {
                for index in 0..=app.project.studio.sources.len() {
                    if let Some(record) = app.source_record(index) {
                        ui.selectable_value(&mut source, index, &record.title);
                    }
                }
            });
        app.select_source(source);
        if command_button(ui, app, Action::AddSource, Action::AddSource.title())
            .on_hover_text("Add audio to this project and keep your existing assignments")
            .clicked()
        {
            Action::AddSource.run(app);
        }
    });
}

fn record_input(app: &mut App, ui: &mut Ui) {
    let active = app.input_recording.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!active, |ui| {
            let name = app.input_device.clone();
            ComboBox::from_id_salt("audio_input_device")
                .selected_text(name.as_deref().unwrap_or("System default input"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.input_device, None, "System default input");
                    for name in crate::input_audio::devices() {
                        ui.selectable_value(&mut app.input_device, Some(name.clone()), name);
                    }
                });
        });
        if ui
            .add_enabled(
                app.audio.output().is_some(),
                egui::Checkbox::new(&mut app.input_monitor, "Monitor input"),
            )
            .on_hover_text("Use headphones when monitoring a microphone.")
            .changed()
            && let Some(take) = &app.input_recording
        {
            take.monitor(app.input_monitor);
        }
        if active {
            if action(ui, "Finish input", true).clicked() {
                app.finish_input();
            }
        } else if primary(ui, "Record input", app.can_add_source() && !app.recording).clicked() {
            app.record_input();
        }
    });
    small(
        ui,
        "Record up to five minutes. Finishing adds the take as a project source.",
    );
}

fn stem_separation(app: &mut App, ui: &mut Ui) {
    small(
        ui,
        "Demucs processes the selection locally and adds vocals, drums, bass, and other as separate sources. Its first run may download model weights.",
    );
    ui.horizontal_wrapped(|ui| {
        let executable = app
            .demucs_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(
                || "demucs on PATH".into(),
                |name| name.to_string_lossy().into_owned(),
            );
        small(ui, executable);
        if action(ui, "Choose Demucs executable…", app.busy.is_none()).clicked() {
            app.demucs_path = rfd::FileDialog::new()
                .set_title("Choose the Demucs executable")
                .pick_file()
                .or(app.demucs_path.take());
        }
        let space = app.project.studio.sources.len() + 1 + crate::separation::STEMS.len()
            <= crate::model::MAX_SOURCES;
        if action(
            ui,
            if app.selection().is_some() {
                "Separate selection"
            } else {
                "Separate source"
            },
            app.active_clip().is_some() && app.can_add_source() && space && !app.recording,
        )
        .clicked()
        {
            app.separate_stems();
        }
    });
}

fn suggestions(app: &mut App, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        if command_button(ui, app, Action::SuggestCuts, Action::SuggestCuts.title()).clicked() {
            Action::SuggestCuts.run(app);
        }
        ui.add(
            DragValue::new(&mut app.suggestion_bars)
                .range(1..=8)
                .suffix(" bars"),
        )
        .on_hover_text("Passage length at the current project tempo");
        ComboBox::from_id_salt("suggestion_emphasis")
            .selected_text(app.suggestion_emphasis.label())
            .show_ui(ui, |ui| {
                for (mode, name) in crate::analysis::Emphasis::OPTIONS {
                    ui.selectable_value(&mut app.suggestion_emphasis, mode, name);
                }
            });
    });
    for index in 0..app.suggestions.len() {
        let mut select = false;
        let mut save = false;
        let mut play = false;
        let suggestion = &app.suggestions[index];
        let range = suggestion.range();
        ui.horizontal_wrapped(|ui| {
            ui.label(label(&suggestion.name, 13.0, GOLD));
            small(
                ui,
                format!("{} – {}", time(range.0, true), time(range.1, true)),
            );
            select = action(ui, "Select range", true).clicked();
            play = action(ui, "Audition", app.audio.output().is_some() && !app.playing).clicked();
            save = action(ui, "Save cut", app.project.samples.len() < MAX_NAMED_SAMPLES).clicked();
        });
        if select || save {
            app.select_suggestion(index, save);
        }
        if play {
            app.audition_range(range);
        }
    }
}

fn warp_editor(app: &mut App, ui: &mut Ui) {
    small(
        ui,
        "Select a range. Mark source positions and set the beat each position should land on, measured from the start of the new clip.",
    );
    let source = app.media.active_source();
    let duration = app.duration();
    let selection = app.selection();
    let mut markers = app
        .project.studio
        .warp_markers
        .get(&source)
        .cloned()
        .unwrap_or_default();
    let before = markers.clone();
    let can_add = markers.len() < crate::stretch::Marker::MAX
        && selection.is_some_and(|(start, end)| {
            (start..end).contains(&app.position) && app.position > start
        });
    if action(ui, "Add at playhead", can_add)
        .on_disabled_hover_text("Seek inside the selected range first.")
        .clicked()
    {
        let start = selection.unwrap().0;
        let beat = ((app.position - start) * f64::from(app.project.studio.bpm) / 60.0 * 4.0).round() / 4.0;
        markers.push(crate::stretch::Marker {
            source_seconds: app.position,
            target_beat: beat.max(0.25),
        });
        markers.sort_by(|a, b| a.source_seconds.total_cmp(&b.source_seconds));
    }
    let mut remove = None;
    for (index, marker) in markers.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Marker {}", index + 1));
                ui.add(
                    DragValue::new(&mut marker.source_seconds)
                        .range(0.0..=duration)
                        .speed(0.01)
                        .suffix(" s"),
                );
                ui.add(
                    DragValue::new(&mut marker.target_beat)
                        .range(0.25..=127.75)
                        .speed(0.25)
                        .prefix("Beat "),
                );
                if action(ui, "Seek", true).clicked() {
                    app.seek(marker.source_seconds);
                }
                if action(ui, "Remove", true).clicked() {
                    remove = Some(index);
                }
            })
        });
    }
    if let Some(index) = remove {
        markers.remove(index);
    }
    if before != markers {
        if markers.is_empty() {
            app.project.studio.warp_markers.remove(&source);
        } else {
            app.project.studio.warp_markers.insert(source, markers);
        }
    }
    ui.add_enabled_ui(selection.is_some() && app.can_add_source(), |ui| {
        ui.menu_button("Preview warped copy…", |ui| {
            for bars in [1, 2, 4, 8, 16] {
                if ui.button(format!("Fit to {bars} bar(s)")).clicked() {
                    app.warp_selection(bars);
                    ui.close_menu();
                }
            }
            let id = ui.id().with("custom_target_beats");
            let mut beats = ui.data(|data| data.get_temp::<usize>(id)).unwrap_or(16);
            ui.add(
                DragValue::new(&mut beats)
                    .range(1..=128)
                    .prefix("Target beats "),
            );
            ui.data_mut(|data| data.insert_temp(id, beats));
            if action(ui, "Preview custom length", true).clicked() {
                app.warp_selection_beats(beats);
                ui.close_menu();
            }
        });
    });
}

pub(super) fn slice_controls(app: &mut App, ui: &mut Ui) -> bool {
    let Some((start, end)) = app.selection() else {
        small(ui, "Select a waveform range first.");
        return false;
    };
    let count_id = ui.id().with("count");
    let mode_id = ui.id().with("mode");
    let division_id = ui.id().with("division");
    let mut count = ui
        .data(|data| data.get_temp::<usize>(count_id))
        .unwrap_or(STEPS);
    let mut division = ui
        .data(|data| data.get_temp::<usize>(division_id))
        .unwrap_or(1);
    let mut mode = ui.data(|data| data.get_temp::<u8>(mode_id)).unwrap_or(0);
    let sensitivity_id = ui.id().with("transient_sensitivity");
    let mut sensitivity = ui
        .data(|data| data.get_temp::<u8>(sensitivity_id))
        .unwrap_or(35);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut mode, 0, "Even");
        ui.selectable_value(&mut mode, 1, "Beats");
        ui.selectable_value(&mut mode, 4, "Bars");
        ui.selectable_value(&mut mode, 2, "Transients");
        ui.selectable_value(&mut mode, 3, "Warp markers");
    });
    let beat = 60.0 / f64::from(app.project.studio.bpm);
    if mode == 1 {
        ui.add(
            DragValue::new(&mut division)
                .range(1..=4)
                .prefix("Slices per beat "),
        );
    } else if mode != 3 && mode != 4 {
        ui.add(
            DragValue::new(&mut count)
                .range(2..=STEPS)
                .prefix("Slices "),
        );
    }
    if mode == 2 {
        ui.add(egui::Slider::new(&mut sensitivity, 5..=45).text("Sensitivity"))
            .on_hover_text("Higher sensitivity searches farther from each evenly spaced boundary");
    }
    ui.data_mut(|data| {
        data.insert_temp(count_id, count);
        data.insert_temp(division_id, division);
        data.insert_temp(mode_id, mode);
        data.insert_temp(sensitivity_id, sensitivity);
    });
    let mut boundaries = vec![start];
    if mode == 1 || mode == 4 {
        if let Some(map) = app.project.studio.beat_maps.get(&app.media.active_source()) {
            boundaries.extend(map.slice_points((start, end), mode == 4, division));
            if boundaries.len() == 1 {
                small(ui, "Select a range containing beat or bar markers.");
            }
        } else if mode == 1 {
            for index in 1..STEPS {
                let at = start + beat / division as f64 * index as f64;
                if at >= end - 0.001 {
                    break;
                }
                boundaries.push(at);
            }
            small(ui, "Using a steady grid at the project tempo.");
        } else {
            small(ui, "Add bar starts in Beat map first.");
        }
    } else if mode == 3 {
        if let Some(markers) = app.project.studio.warp_markers.get(&app.media.active_source()) {
            boundaries.extend(
                markers
                    .iter()
                    .map(|marker| marker.source_seconds)
                    .filter(|&at| start < at && at < end)
                    .take(STEPS - 1),
            );
        }
        boundaries.sort_by(f64::total_cmp);
        if boundaries.len() == 1 {
            small(
                ui,
                "Add a warp marker inside the selection to split it here.",
            );
        }
    } else {
        let step = (end - start) / count as f64;
        for index in 1..count {
            let at = start + step * index as f64;
            let adjusted = if mode == 2 {
                app.active_clip().map_or(at, |clip| {
                    transient_near(clip, at, (step * f64::from(sensitivity) / 100.0).min(0.2))
                })
            } else {
                at
            };
            boundaries.push(adjusted);
        }
    }
    if (mode == 4 || (mode == 1 && app.project.studio.beat_maps.contains_key(&app.media.active_source())))
        && boundaries.len() < 2
    {
        return false;
    }
    boundaries.push(end);
    if boundaries.windows(2).any(|pair| pair[1] - pair[0] < 0.001) {
        small(ui, "Select a longer range or use fewer slices.");
        return false;
    }
    small(
        ui,
        format!(
            "{} slices. Applying replaces pad ranges.",
            boundaries.len() - 1
        ),
    );
    ui.horizontal_wrapped(|ui| {
        for (index, pair) in boundaries.windows(2).enumerate() {
            if action(
                ui,
                &format!("Play {}", index + 1),
                app.audio.output().is_some() && !app.playing,
            )
            .on_hover_text(format!("{:.3}–{:.3} s", pair[0], pair[1]))
            .clicked()
            {
                app.audition_range((pair[0], pair[1]));
            }
        }
    });
    if primary(ui, "Apply slices", true).clicked() {
        app.project.studio.apply_slices(&boundaries, app.media.active_source());
        return true;
    }
    false
}

fn fit_to_bars(app: &mut App, ui: &mut Ui) {
    ui.add_enabled_ui(
        app.selection().is_some() && app.can_add_source() && !app.recording,
        |ui| {
            ui.menu_button("Fit to bars…", |ui| {
                small(ui, "Prepares a pitch-preserved preview at the current BPM.");
                for bars in [1, 2, 4, 8] {
                    let seconds = 240.0 * bars as f64 / f64::from(app.project.studio.bpm);
                    let enabled = app.selection().is_some_and(|(start, end)| {
                        (0.5..=2.0).contains(&(seconds / (end - start)))
                    });
                    if action(
                        ui,
                        &format!(
                            "{bars} {} · {seconds:.2} s",
                            if bars == 1 { "bar" } else { "bars" }
                        ),
                        enabled,
                    )
                    .on_disabled_hover_text(
                        "Choose a duration between half and twice the selection",
                    )
                    .clicked()
                    {
                        app.stretch_selection(bars);
                        ui.close_menu();
                    }
                }
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audio::Clip, files};
    use eframe::egui::{Context, FullOutput, Pos2, Rect, Shape, vec2};
    use std::{path::PathBuf, sync::Arc};

    fn render(app: &mut App, ctx: &Context, width: f32) -> FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 750.0))),
                ..Default::default()
            },
            |ctx| super::super::draw(app, ctx),
        )
    }

    fn visible(output: &FullOutput, label: &str) -> bool {
        output.shapes.iter().any(|paint| {
            matches!(&paint.shape, Shape::Text(text) if text.galley.text() == label
                && paint.clip_rect.contains_rect(text.visual_bounding_rect()))
        })
    }

    #[test]
    fn sample_keeps_the_waveform_visible_with_pads_or_a_contextual_tool() {
        let ctx = Context::default();
        let mut app = App::new(ctx.clone());
        app.audio = Default::default();
        app.library.interface = Default::default();
        crate::ui::preferences::apply(app.library.interface, &ctx);
        app.project.record = Some(files::local_record(PathBuf::from("fixture.wav")));
        app.media.replace(Some(Arc::new(Clip::new(8_000, vec![[0.25; 2]; 32_000]))));
        app.set_loop_range((1.0, 2.0));
        Action::Studio(crate::app::StudioView::Sample).run(&mut app);

        render(&mut app, &ctx, 1150.0);
        let output = render(&mut app, &ctx, 1150.0);
        assert!(visible(&output, "PADS"));
        assert!(visible(&output, "Selection 1.000 s"));
        assert!(!visible(&output, "Choose Basic Pitch Python…"));

        Action::Sample(Tool::Transcribe).run(&mut app);
        let output = render(&mut app, &ctx, 1150.0);
        assert!(visible(&output, "Choose Basic Pitch Python…"));
        assert!(visible(&output, "Selection 1.000 s"));
        assert!(!visible(&output, "PADS"));
        assert_eq!(app.selection(), Some((1.0, 2.0)));

        Action::Sample(Tool::Extract).run(&mut app);
        render(&mut app, &ctx, 800.0);
        let output = render(&mut app, &ctx, 800.0);
        assert!(visible(&output, "Sample tools"));
        assert!(visible(&output, "Selection 1.000 s"));
        assert!(visible(&output, "Extract sound · Experimental"));
        assert!(!visible(&output, "Choose Basic Pitch Python…"));
    }
}
