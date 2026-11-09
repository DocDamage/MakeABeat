use super::{
    MINT, action, command_button, groove, label, named_number, panel, pill, responsive_modal,
    small, step_grid, variation,
};
use crate::{
    app::{
        App,
        actions::Action,
        sequence_workflow::{BarEdit, ClearLane, Edit},
    },
    audio::Command,
    model::{KITS, MAX_BARS, PIANO_HIGH, PIANO_LOW, Quantize, STEPS, piano_note},
};
use eframe::egui::{ComboBox, Context, DragValue, Id, Label, ScrollArea, Ui};

#[derive(Clone, Copy)]
pub(crate) enum Tool {
    Recording,
    Selection,
    Timing,
    Conditions,
    Velocity,
    Groove,
    Tempo,
}

const GRID_STATE: &str = "sequencer_grid_state";

pub(super) fn show(app: &mut App, ui: &mut Ui) {
    let id = Id::new(GRID_STATE);
    let mut grid = ui
        .data_mut(|data| data.remove_temp::<step_grid::State>(id))
        .unwrap_or_default();
    grid.selection.set_bar(app.bar);
    panel().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            let name = app.studio.editing_pattern.map_or("Working pattern", |index| {
                app.studio.patterns[index].name.as_str()
            });
            ui.add_sized([148.0, 20.0], Label::new(label(name, 14.0, MINT).strong()).truncate());
            recording_controls(app, ui);
            small(ui, "BPM");
            let tempo = ui.add(DragValue::new(&mut app.studio.bpm).range(40..=240).speed(0.25));
            named_number(tempo, "Tempo in beats per minute", f64::from(app.studio.bpm));
            if action(ui, "Tap", true).clicked() {
                app.tap();
            }
            small(ui, "Kit");
            ComboBox::from_id_salt("kit")
                .selected_text(KITS[app.studio.kit])
                .width(72.0)
                .show_ui(ui, |ui| {
                    for (index, name) in KITS.into_iter().enumerate() {
                        ui.selectable_value(&mut app.studio.kit, index, name);
                    }
                })
                .response.on_hover_text("Choose the drum kit");
            tools(app, ui, &grid);
            if let Some(estimate) = app.tempo_estimate
                && action(ui, &format!("Estimated {} BPM…", estimate.bpm), true).clicked()
            {
                app.sequence_tool = Some(Tool::Tempo);
            }
        });
        ui.horizontal_wrapped(|ui| {
            bar_navigation(app, ui);
            grid.selection.set_bar(app.bar);
            instruments(app, ui);
            grid.selection.toggle(ui, can_select(app));
        });
        ui.separator();
        ScrollArea::vertical()
            .id_salt("sequence_lanes")
            .drag_to_scroll(false)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                step_grid::show(app, ui, &mut grid);
                small(ui, "Drag to paint or erase drums. Shift adds accents. Ctrl-click edits velocity. Alt-click opens conditions. Gold dots mark recorded hits.");
            });
    });
    ui.data_mut(|data| data.insert_temp(id, grid));
}

fn recording_controls(app: &mut App, ui: &mut Ui) {
    let can_record = app.recording || (app.output.is_some() && app.clip.is_some());
    if action(
        ui,
        if app.recording {
            "● Finish recording"
        } else {
            "● Record"
        },
        can_record,
    )
    .on_hover_text("Overdub pads or keys with the mouse, computer keyboard, or MIDI")
    .clicked()
    {
        app.toggle_recording();
    }
    ui.add_enabled_ui(!app.recording, |ui| {
        ComboBox::from_id_salt("record_quantize")
            .selected_text(format!("Quantize {}", app.quantize.label()))
            .show_ui(ui, |ui| {
                for quantize in Quantize::ALL {
                    ui.selectable_value(&mut app.quantize, quantize, quantize.label());
                }
            });
    })
    .response
    .on_hover_text("Off keeps your timing. Grid settings snap new hits to the nearest division.");
    if ui
        .checkbox(&mut app.click_track, "Click")
        .on_hover_text("Play a metronome during playback and recording")
        .changed()
    {
        app.send(Command::ClickTrack(app.click_track));
    }
}

fn bar_navigation(app: &mut App, ui: &mut Ui) {
    let bars = app.studio.bars;
    app.bar = app.bar.min(bars - 1);
    let playing = app
        .song_playing
        .is_none()
        .then_some(app.step)
        .flatten()
        .map(|step| step / STEPS);
    if bars > 1 && ui.available_width() >= 280.0 + 32.0 * bars as f32 {
        small(ui, "Bar");
        for bar in 0..bars {
            let title = if playing == Some(bar) {
                format!("▶ {}", bar + 1)
            } else {
                (bar + 1).to_string()
            };
            if pill(ui, &title, app.bar == bar).clicked() {
                app.bar = bar;
            }
        }
    } else {
        if action(ui, "‹", app.bar > 0)
            .on_hover_text("Previous bar")
            .clicked()
        {
            app.bar -= 1;
        }
        ComboBox::from_id_salt("sequence_bar")
            .selected_text(format!("Bar {} / {bars}", app.bar + 1))
            .show_ui(ui, |ui| {
                for bar in 0..bars {
                    let mark = if playing == Some(bar) { "▶ " } else { "" };
                    ui.selectable_value(&mut app.bar, bar, format!("{mark}Bar {}", bar + 1));
                }
            });
        if action(ui, "›", app.bar + 1 < bars)
            .on_hover_text("Next bar")
            .clicked()
        {
            app.bar += 1;
        }
    }
}

fn instruments(app: &mut App, ui: &mut Ui) {
    ComboBox::from_id_salt("sequence_pad")
        .selected_text(format!("Pad {:02}", app.selected_pad + 1))
        .show_ui(ui, |ui| {
            for pad in 0..STEPS {
                if app.studio.pads[pad].is_some() {
                    ui.selectable_value(&mut app.selected_pad, pad, format!("Pad {:02}", pad + 1));
                }
            }
        })
        .response
        .on_hover_text("Choose the pad to place on the Chops row");
    ComboBox::from_id_salt("sequence_key")
        .selected_text(piano_note(app.piano_key))
        .show_ui(ui, |ui| {
            for note in PIANO_LOW..=PIANO_HIGH {
                if app.key_range(note).is_some() {
                    ui.selectable_value(&mut app.piano_key, note, piano_note(note));
                }
            }
        })
        .response
        .on_hover_text("Choose the key to place on the Keys row");
}

fn tools(app: &mut App, ui: &mut Ui, grid: &step_grid::State) {
    ui.menu_button("Sequence tools", |ui| {
        ui.menu_button("Edit bars", |ui| bar_tools(app, ui));
        ui.menu_button("Clear programmed steps", |ui| {
            let bar = app.bar;
            let lanes = [
                (
                    "Clear pad steps",
                    ClearLane::Pads,
                    app.studio.chops[bar].iter().any(Option::is_some),
                ),
                (
                    "Clear key steps",
                    ClearLane::Keys,
                    app.studio.key_steps[bar].iter().any(Option::is_some),
                ),
                (
                    "Clear drums",
                    ClearLane::Drums,
                    app.studio.hits[bar].iter().flatten().any(|&hit| hit > 0),
                ),
            ];
            for (label, lane, enabled) in lanes {
                if menu_action(ui, label, enabled) {
                    app.edit_sequence([Edit::ClearProgrammed(lane)], false);
                }
            }
        });
        ui.separator();
        for (tool, enabled) in [
            (Tool::Selection, can_select(app)),
            (Tool::Velocity, grid.velocity_cell.is_some()),
            (Tool::Conditions, true),
            (Tool::Timing, true),
            (Tool::Recording, true),
            (Tool::Groove, true),
        ] {
            if menu_action(ui, &format!("{}…", tool.title()), enabled) {
                app.sequence_tool = Some(tool);
            }
        }
        ui.separator();
        for command in [Action::PianoRoll, Action::Templates] {
            if command_button(ui, app, command, command.title()).clicked() {
                ui.close_menu();
                command.run(app);
            }
        }
        if menu_action(ui, "Generate variations…", can_select(app)) {
            let selected = grid
                .selection
                .enabled()
                .then(|| grid.selection.region())
                .flatten()
                .map(|region| (app.bar, region));
            app.variations = Some(variation::Dialog::new(
                app.studio.active_sequence(),
                selected,
            ));
        }
        if menu_action(
            ui,
            "Estimate BPM",
            app.duration() >= 6.0 && app.busy.is_none(),
        ) {
            app.estimate_tempo();
        }
        let free_pad = app.studio.pads.iter().any(Option::is_none);
        let has_sound = app.studio.has_pattern() || app.studio.active_loop().is_some();
        if menu_action(
            ui,
            "Resample pattern to pad",
            app.can_add_source() && !app.recording && free_pad && has_sound,
        ) {
            app.resample_mix(false);
        }
        if command_button(ui, app, Action::ExportMidi, Action::ExportMidi.title()).clicked() {
            ui.close_menu();
            Action::ExportMidi.run(app);
        }
    });
}

fn bar_tools(app: &mut App, ui: &mut Ui) {
    ui.add_enabled_ui(!app.recording, |ui| {
        ui.menu_button(format!("Pattern length · {} bars", app.studio.bars), |ui| {
            for count in 1..=MAX_BARS {
                if menu_action(ui, &count.to_string(), count != app.studio.bars) {
                    app.edit_sequence([Edit::Bar(BarEdit::SetCount(count))], false);
                }
            }
        });
    });
    let editable = can_select(app);
    if menu_action(ui, "Copy bar", editable) {
        app.bar_clipboard = Some(app.studio.copy_bar(app.bar));
    }
    let space = app.studio.bars < MAX_BARS;
    for (label, edit, enabled) in [
        ("Paste bar", BarEdit::Paste, app.bar_clipboard.is_some()),
        ("Duplicate bar", BarEdit::Duplicate, space),
        ("Insert blank bar after", BarEdit::Insert, space),
        ("Remove bar", BarEdit::Remove, app.studio.bars > 1),
    ] {
        if menu_action(ui, label, editable && enabled) {
            app.edit_sequence([Edit::Bar(edit)], false);
        }
    }
}

fn can_select(app: &App) -> bool {
    !app.recording && app.song_playing.is_none()
}

fn menu_action(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    let clicked = action(ui, text, enabled).clicked();
    if clicked {
        ui.close_menu();
    }
    clicked
}

impl Tool {
    fn title(self) -> &'static str {
        match self {
            Self::Recording => "Recording options",
            Self::Selection => "Edit cell selection",
            Self::Timing => "Lane timing",
            Self::Conditions => "Step conditions",
            Self::Velocity => "Drum velocity",
            Self::Groove => "Groove transfer",
            Self::Tempo => "Tempo estimate",
        }
    }
}

pub(super) fn dialog(app: &mut App, ctx: &Context) {
    let Some(tool) = app.sequence_tool else {
        return;
    };
    let id = Id::new(GRID_STATE);
    let mut grid = ctx
        .data_mut(|data| data.remove_temp::<step_grid::State>(id))
        .unwrap_or_default();
    let modal = responsive_modal(ctx, "sequence_tool", 560.0, |ui| {
        ui.heading(tool.title());
        match tool {
            Tool::Recording => recording_options(app, ui),
            Tool::Selection => grid.selection.controls(app, ui),
            Tool::Timing => step_grid::lane_timing(app, ui),
            Tool::Conditions => grid.conditions(app, ui),
            Tool::Velocity => {
                if let Some(cell) = grid.velocity_cell {
                    step_grid::velocity(app, ui, cell);
                }
            }
            Tool::Groove => groove::panel(app, ui, false),
            Tool::Tempo => tempo_estimate(app, ui),
        }
        ui.separator();
        ui.button("Done").clicked()
    });
    if modal.inner || modal.should_close() {
        app.sequence_tool = None;
    }
    ctx.data_mut(|data| data.insert_temp(id, grid));
}

fn recording_options(app: &mut App, ui: &mut Ui) {
    ui.add_enabled_ui(!app.recording, |ui| {
        ui.checkbox(&mut app.replace_take, "Replace recorded notes");
        ui.add(
            DragValue::new(&mut app.count_in_bars)
                .range(0..=2)
                .prefix("Count-in ")
                .suffix(" bars"),
        )
        .on_hover_text("Applies when starting stopped playback on the internal clock");
        ui.checkbox(&mut app.punch_in, "Punch in");
        if app.punch_in {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    DragValue::new(&mut app.punch_first)
                        .range(1..=app.studio.bars)
                        .prefix("From bar "),
                );
                ui.add(
                    DragValue::new(&mut app.punch_last)
                        .range(app.punch_first.min(app.studio.bars)..=app.studio.bars)
                        .prefix("Through bar "),
                );
            });
        }
    });
    let mut gain = (app.click_gain * 100.0).round() as u8;
    if ui
        .add(
            DragValue::new(&mut gain)
                .range(0..=100)
                .prefix("Click ")
                .suffix("%"),
        )
        .changed()
    {
        app.click_gain = f32::from(gain) * 0.01;
        app.send(Command::ClickGain(app.click_gain));
    }
}

fn tempo_estimate(app: &mut App, ui: &mut Ui) {
    let Some(estimate) = app.tempo_estimate else {
        return;
    };
    small(
        ui,
        format!(
            "Estimated {} BPM · onset match {}%",
            estimate.bpm, estimate.strength
        ),
    );
    ui.horizontal_wrapped(|ui| {
        for candidate in [estimate.bpm / 2, estimate.bpm, estimate.bpm * 2] {
            if !(40..=240).contains(&candidate) {
                continue;
            }
            ui.menu_button(format!("{candidate} BPM"), |ui| {
                if menu_action(
                    ui,
                    "Audition beat grid",
                    app.output.is_some() && !app.playing,
                ) {
                    app.audition_tempo(candidate);
                }
                if menu_action(ui, "Apply tempo", true) {
                    app.studio.bpm = candidate;
                    app.tempo_estimate = None;
                    app.sequence_tool = None;
                    app.send(Command::StopListening);
                }
            });
        }
        if action(ui, "Dismiss estimate", true).clicked() {
            app.tempo_estimate = None;
            app.sequence_tool = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{Page, StudioView},
        model::drum_velocity,
    };
    use eframe::egui::{
        self, Event, FullOutput, Modifiers, PointerButton, Pos2, Rect, Shape, pos2, vec2,
    };

    fn render(app: &mut App, ctx: &Context, events: Vec<Event>) -> FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1150.0, 750.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                super::super::draw(app, ctx);
                app.observe_edits(ctx.input(|input| input.pointer.primary_down()));
            },
        )
    }

    fn widget(output: &FullOutput, label: &str) -> Option<Pos2> {
        let update = output.platform_output.accesskit_update.as_ref()?;
        update.nodes.iter().find_map(|(_, node)| {
            if node.label()?.starts_with(label) {
                let bounds = node.bounds()?;
                Some(pos2(
                    ((bounds.x0 + bounds.x1) * 0.5) as f32,
                    ((bounds.y0 + bounds.y1) * 0.5) as f32,
                ))
            } else {
                None
            }
        })
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn click(app: &mut App, ctx: &Context, pos: Pos2) -> FullOutput {
        render(app, ctx, vec![Event::PointerMoved(pos), button(pos, true)]);
        render(app, ctx, vec![button(pos, false)])
    }

    #[test]
    fn default_layout_keeps_bottom_drum_lane_visible_and_paints_one_undo_group() {
        let ctx = Context::default();
        super::super::style(&ctx);
        let mut app = App::new(ctx.clone());
        app.output = None;
        app.library.interface = Default::default();
        app.library.interface.apply(&ctx);
        app.page = Page::Studio;
        app.studio_view = StudioView::Sequence;
        app.studio.bars = MAX_BARS;
        ctx.enable_accesskit();
        render(&mut app, &ctx, vec![]);
        let output = render(&mut app, &ctx, vec![]);
        assert!(output.shapes.iter().any(|paint| {
            matches!(&paint.shape, Shape::Text(text) if text.galley.text() == "Ride"
                && paint.clip_rect.contains_rect(text.visual_bounding_rect()))
        }));
        assert!(widget(&output, "Play pattern").is_some());
        assert!(widget(&output, "Sequence tools").is_some());
        assert!(widget(&output, "Copy bar").is_none());
        let first = widget(&output, "Kick · bar 1 · step 1 ·").expect("first kick step");
        let second = widget(&output, "Kick · bar 1 · step 2 ·").expect("second kick step");
        render(
            &mut app,
            &ctx,
            vec![Event::PointerMoved(first), button(first, true)],
        );
        render(&mut app, &ctx, vec![Event::PointerMoved(second)]);
        render(&mut app, &ctx, vec![button(second, false)]);
        assert_eq!(
            [0, 1].map(|step| drum_velocity(app.studio.hits[0][0][step])),
            [89, 89]
        );
        app.undo_project(false);
        assert_eq!(app.studio.hits[0][0][..2], [0, 0]);

        let output = render(&mut app, &ctx, vec![]);
        let tools = widget(&output, "Sequence tools").unwrap();
        click(&mut app, &ctx, tools);
        let output = render(&mut app, &ctx, vec![]);
        let edit_bars = widget(&output, "Edit bars").expect("bar submenu");
        render(&mut app, &ctx, vec![Event::PointerMoved(edit_bars)]);
        let output = render(&mut app, &ctx, vec![]);
        let remove = widget(&output, "Remove bar").expect("remove action");
        click(&mut app, &ctx, remove);
        assert_eq!(app.studio.bars, MAX_BARS - 1);
        app.undo_project(false);
        assert_eq!(app.studio.bars, MAX_BARS);
    }
}
