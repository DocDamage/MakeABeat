use super::{App, action, primary, small};
use crate::{
    app::transcription_workflow::Editor,
    model::{
        ChopInput, MAX_SAMPLE_NAME, MAX_SONG_BARS, PIANO_HIGH, PIANO_LOW, PlayedChop,
        TICKS_PER_BAR,
        melody::{Expression, Instrument},
        piano_note,
    },
    transcription::{Alignment, MAX_NOTES},
};
use eframe::egui::{self, ComboBox, Context, DragValue, Ui};
use std::sync::Arc;

pub fn show(app: &mut App, ctx: &Context, editor: &mut Editor) -> bool {
    let mut close = false;
    let modal = super::responsive_modal(ctx, "transcribe_notes", 880.0, |ui| {
        ui.heading("Transcribe to notes");
        small(
            ui,
            format!(
                "{} · {:.3}–{:.3} s",
                editor.title, editor.range.0, editor.range.1
            ),
        );
        small(
            ui,
            "Use an isolated bass or melodic stem. Basic Pitch detects pitches from A0 through C8.",
        );
        let job = app.transcription_job();
        ui.horizontal_wrapped(|ui| {
            ui.add(
                DragValue::new(&mut editor.detection.low)
                    .range(0..=127)
                    .prefix("Low MIDI "),
            );
            ui.add(
                DragValue::new(&mut editor.detection.high)
                    .range(0..=127)
                    .prefix("High MIDI "),
            );
            ui.add(
                DragValue::new(&mut editor.detection.min_ms)
                    .range(30..=2_000)
                    .prefix("Minimum ")
                    .suffix(" ms"),
            );
            ui.add(
                DragValue::new(&mut editor.detection.onset)
                    .range(0.05..=0.95)
                    .speed(0.01)
                    .prefix("Onset "),
            );
            ui.add(
                DragValue::new(&mut editor.detection.frame)
                    .range(0.05..=0.95)
                    .speed(0.01)
                    .prefix("Frame "),
            );
            ui.checkbox(&mut editor.detection.recover_sustains, "Recover weak notes");
        });
        ui.horizontal_wrapped(|ui| {
            if editor.mapped() {
                small(ui, "Timing follows the corrected beat map");
            } else {
                ui.add(
                    DragValue::new(&mut editor.bpm)
                        .range(30..=300)
                        .prefix("Source tempo ")
                        .suffix(" BPM"),
                );
                small(
                    ui,
                    format!("Preview plays at project tempo · {} BPM", app.project.studio.bpm),
                );
            }
            let has_downbeat = editor.has_downbeat();
            ComboBox::from_id_salt("transcription_alignment")
                .selected_text(match editor.alignment {
                    Alignment::Selection => "Align selection start",
                    Alignment::FirstNote => "Align first detected note",
                    Alignment::Downbeat => "Align mapped bar grid",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut editor.alignment,
                        Alignment::Selection,
                        "Align selection start",
                    );
                    ui.selectable_value(
                        &mut editor.alignment,
                        Alignment::FirstNote,
                        "Align first detected note",
                    );
                    if has_downbeat {
                        ui.selectable_value(
                            &mut editor.alignment,
                            Alignment::Downbeat,
                            "Align mapped bar grid",
                        );
                    }
                });
            if action(ui, "Rebuild timing", job.is_none() && editor.has_proposal()).on_hover_text(
                "Rebuild from the original detections using the current alignment and note range."
            ).clicked() {
                app.stop_transcription_preview(editor);
                editor.align();
            }
        });
        ui.horizontal_wrapped(|ui| {
            if primary(
                ui,
                if editor.has_proposal() {
                    "Detect again"
                } else {
                    "Detect notes"
                },
                job.is_none(),
            )
            .clicked()
            {
                app.detect_transcription(editor);
            }
            if let Some((label, state, _)) = job {
                ui.spinner();
                small(ui, state.stage.unwrap_or(label));
                if let Some(progress) = state.progress {
                    ui.add(egui::ProgressBar::new(progress).desired_width(110.0));
                }
                if action(ui, "Cancel analysis", !state.cancelled).clicked() {
                    app.cancel_transcription_job();
                }
            }
            ui.menu_button("Basic Pitch setup", |ui| {
                small(ui, app.pitch_python().display().to_string());
                if action(ui, "Choose Python…", job.is_none()).clicked() {
                    app.choose_pitch_python();
                }
                small(
                    ui,
                    "Install the pinned CPU packages from the audio worker README.",
                );
            });
        });
        if let Some(message) = &editor.status {
            small(ui, message);
        }
        if !editor.unsupported.is_empty() {
            small(
                ui,
                format!(
                    "Outside the editable pitch range · {:?}",
                    editor.unsupported
                ),
            );
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if action(ui, "Hear source", app.audio.output().is_some() && !app.recording).clicked() {
                app.listen_transcription_source(editor);
            }
            sound_picker(app, ui, &mut editor.sound);
            if action(
                ui,
                "Hear notes",
                app.can_play_section() && !editor.notes.is_empty(),
            )
            .clicked()
            {
                app.listen_transcription_notes(editor);
            }
            if action(ui, "Stop", true).clicked() {
                app.stop_transcription_preview(editor);
            }
        });
        if editor.has_proposal() {
            edit_notes(ui, editor);
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut editor.name)
                    .hint_text("New pattern name")
                    .char_limit(MAX_SAMPLE_NAME)
                    .desired_width(165.0),
            );
            if primary(
                ui,
                "Create pattern",
                job_is_idle(app) && !editor.notes.is_empty(),
            )
            .clicked()
            {
                match app.commit_transcription(editor) {
                    Ok(()) => close = true,
                    Err(error) => editor.status = Some(error.to_string()),
                }
            }
            if action(ui, "Close", true).clicked() {
                close = true;
            }
        });
    });
    close || modal.should_close()
}

fn job_is_idle(app: &App) -> bool {
    app.transcription_job().is_none() && !app.recording
}

pub fn sound_picker(app: &App, ui: &mut Ui, selected: &mut Instrument) {
    let options = app.transcription_sounds();
    let without_root = |sound| match sound {
        Instrument::Sample {
            source, start, end, ..
        } => Instrument::Sample {
            source,
            start,
            end,
            root: 60,
        },
        Instrument::Sine => Instrument::Sine,
    };
    let name = options
        .iter()
        .find(|(_, sound)| without_root(*sound) == without_root(*selected))
        .map_or("Project sample", |(label, _)| label.as_str());
    ComboBox::from_id_salt("transcription_sound")
        .selected_text(name)
        .show_ui(ui, |ui| {
            for (name, sound) in options.clone() {
                if ui
                    .selectable_label(without_root(sound) == without_root(*selected), name)
                    .clicked()
                {
                    *selected = sound;
                }
            }
        });
    if let Instrument::Sample { root, .. } = selected {
        let label = piano_note(*root);
        ui.add(
            DragValue::new(root)
                .range(PIANO_LOW..=PIANO_HIGH)
                .prefix("Sample root "),
        )
        .on_hover_text(label);
    }
}

fn edit_notes(ui: &mut Ui, editor: &mut Editor) {
    let max_tick = MAX_SONG_BARS as u32 * TICKS_PER_BAR;
    let end = editor
        .notes
        .iter()
        .map(|note| note.tick + note.length.unwrap_or(1))
        .max()
        .unwrap_or(1);
    let bars = end.div_ceil(TICKS_PER_BAR);
    let mut bar = editor.bar.min(bars - 1) + 1;
    ui.horizontal_wrapped(|ui| {
        small(ui, format!("{} notes · {bars} bars", editor.notes.len()));
        ui.add(
            DragValue::new(&mut bar)
                .range(1..=bars)
                .prefix("Show from bar "),
        );
        if action(ui, "Add note", editor.notes.len() < MAX_NOTES).clicked() {
            editor.notes.push(PlayedChop {
                id: Default::default(),
                tick: (bar - 1) * TICKS_PER_BAR,
                input: ChopInput::Key(60),
                velocity: 90,
                mode: crate::audio::PadMode::Gate,
                length: Some(480),
                sound: Default::default(),
                expression: Some(Arc::new(Expression::default())),
            });
            editor.selected = Some(editor.notes.len() - 1);
        }
    });
    editor.bar = bar - 1;
    super::piano_roll::transcription_canvas(
        ui,
        &editor.notes,
        editor.bar,
        bars,
        &mut editor.selected,
    );
    let Some(index) = editor.selected else { return };
    let Some(note) = editor.notes.get_mut(index) else {
        return;
    };
    let mut pitch = match note.input {
        ChopInput::Key(pitch) => pitch,
        _ => return,
    };
    let mut length = note.length.unwrap_or(1);
    let mut delete = false;
    ui.horizontal_wrapped(|ui| {
        let name = piano_note(pitch);
        ui.add(
            DragValue::new(&mut pitch)
                .range(PIANO_LOW..=PIANO_HIGH)
                .prefix("Pitch "),
        )
        .on_hover_text(name);
        ui.add(
            DragValue::new(&mut note.tick)
                .range(0..=max_tick - 1)
                .prefix("Start ")
                .suffix(" ticks"),
        );
        ui.add(
            DragValue::new(&mut length)
                .range(1..=max_tick - note.tick)
                .prefix("Length ")
                .suffix(" ticks"),
        );
        ui.add(
            DragValue::new(&mut note.velocity)
                .range(1..=127)
                .prefix("Velocity "),
        );
        let bends = note
            .expression
            .as_ref()
            .map_or(0, |expression| expression.bends.len());
        if action(ui, "Clear bend", bends > 0).clicked()
            && let Some(expression) = &mut note.expression
        {
            Arc::make_mut(expression).bends.clear();
        }
        delete = action(ui, "Delete note", true).clicked();
    });
    note.input = ChopInput::Key(pitch);
    note.length = Some(length.min(max_tick - note.tick));
    if delete {
        editor.notes.remove(index);
        editor.selected = None;
    }
}
