use super::{GOLD, MINT, TEXT, action, label, primary, small};
use crate::{
    app::{App, StudioView, arrangement_workflow::Edit},
    model::{MAX_PATTERNS, MAX_REPEATS, MAX_SAMPLE_NAME, MAX_SONG_BARS, SectionKind},
};
use eframe::egui::{self, ComboBox, DragValue, ScrollArea, Ui};

mod automation;
mod timeline;
pub(crate) use timeline::Timeline;

pub(super) fn show(app: &mut App, ui: &mut Ui) {
    let playing = app.song_playing.is_some();
    if app.project.studio.patterns.is_empty() {
        small(ui, "Save the current grid as your first pattern, then add it to the timeline.");
        if primary(ui, "Create first pattern", !playing && !app.recording).clicked() {
            if app.pattern_name.trim().is_empty() {
                app.pattern_name = app.project.studio.next_pattern_name();
            }
            app.create_pattern(false);
        }
    }
    scene_launcher(app, ui);
    let pattern_list_height = if app.view.arrangement_timeline.automation.is_some() { 58.0 } else { 103.0 };
    ui.add_enabled_ui(!playing && !app.recording, |ui| {
        ui.label(label("Patterns", 15.0, MINT).strong());
        small(ui, "Save the current grid or start blank. Edits to a named pattern update every section that uses it.");
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut app.pattern_name).hint_text("Pattern name")
                .char_limit(MAX_SAMPLE_NAME).desired_width(145.0));
            let name = app.pattern_name.trim();
            let unique = !name.is_empty() && !app.project.studio.patterns.iter().any(|pattern| pattern.name.eq_ignore_ascii_case(name));
            let space = app.project.studio.patterns.len() < MAX_PATTERNS;
            let blank_space = app.project.studio.patterns.len() + usize::from(app.project.studio.has_draft()) < MAX_PATTERNS;
            if action(ui, "Save current as new", unique && space).clicked() {
                app.create_pattern(false);
            }
            if action(ui, "New blank", unique && blank_space).clicked() {
                app.create_pattern(true);
                app.view.studio_view = StudioView::Sequence;
            }
            if action(ui, "Rename current", unique && app.project.studio.editing_pattern.is_some()).clicked()
                && let Some(index) = app.project.studio.editing_pattern
            {
                app.project.studio.patterns[index].name = app.pattern_name.trim().to_owned();
                app.pattern_name = app.project.studio.next_pattern_name();
            }
        });
        let mut edit = None;
        let mut remove = None;
        ScrollArea::vertical().id_salt("saved_patterns").max_height(pattern_list_height).show(ui, |ui| {
            for (index, pattern) in app.project.studio.patterns.iter().enumerate() {
                ui.push_id(index, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let current = app.project.studio.editing_pattern == Some(index);
                        if action(ui, if current { "Editing" } else { "Edit" }, !current).clicked() {
                            edit = Some(index);
                        }
                        ui.label(label(&pattern.name, 13.0, if current { MINT } else { TEXT }));
                        small(ui, format!("{} bars", pattern.length));
                        let uses = app.project.studio.sections.iter().filter(|section| section.pattern == index).count();
                        if uses > 0 {
                            small(ui, format!("Used by {uses} {}", if uses == 1 { "section" } else { "sections" }));
                        }
                        if action(ui, "Remove", uses == 0 && !current && app.scene_active.is_none()
                            && app.scene_pending.is_none() && !app.scene_capture && app.scene_take.is_empty())
                            .on_hover_text("Switch away from this pattern and remove its sections before deleting it.")
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
        if let Some(index) = edit {
            app.edit_pattern(index);
            if app.project.studio.editing_pattern == Some(index) {
                app.view.studio_view = StudioView::Sequence;
            }
        }
        if let Some(index) = remove {
            app.project.studio.remove_pattern(index);
        }
    });

    ui.separator();
    let editable = !playing && !app.recording;
    let mut automation_updated = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(label("Section timeline", 15.0, MINT).strong());
        if let Some(pattern) = app.project.studio.editing_pattern.or_else(|| (!app.project.studio.patterns.is_empty()).then_some(0))
            && action(ui, "+ Add section", app.can_add_section(pattern)).clicked()
        {
            app.edit_arrangement(Edit::Add(pattern));
        }
        let selected = app.view.arrangement_timeline.selected;
        if action(ui, "Duplicate", selected.is_some_and(|index| app.can_duplicate_section(index))).clicked()
            && let Some(index) = selected
        {
            app.edit_arrangement(Edit::Duplicate(index));
        }
        if !app.project.studio.sections.is_empty() {
            let timeline = &mut app.view.arrangement_timeline;
            ui.add(egui::Slider::new(&mut timeline.bar_width, 18.0..=144.0).text("Zoom").show_value(false));
            if super::pill(ui, "Automation", timeline.automation.is_some()).clicked() {
                timeline.automation = if timeline.automation.is_some() { None } else { Some(automation::Editor::default()) };
            }
        }
    });
    if app.project.studio.sections.is_empty() {
        if !app.project.studio.patterns.is_empty() {
            small(ui, "Use Add section to place a pattern on the timeline.");
        }
    } else {
        let timeline = &mut app.view.arrangement_timeline;
        let studio = &mut app.project.studio;
        if let Some(editor) = &mut timeline.automation {
            editor.controls(ui, studio, &mut timeline.selected, &mut timeline.automation_clipboard, editable);
            editor.point_controls(ui, studio, timeline.selected, editable);
            automation_updated = editor.live_controls(ui, studio, timeline.selected, app.step.filter(|_| playing));
        }
        if let Some(edit) = timeline.show(ui, studio, app.step.filter(|_| playing), editable) {
            app.edit_arrangement(edit);
        }
        let timeline = &mut app.view.arrangement_timeline;
        let studio = &mut app.project.studio;
        if timeline.automation.is_none() {
            small(ui, "Drag a block to move it. Drag either edge to trim or extend by whole bars.");
        }
        if let Some(index) = timeline.selected.filter(|&index| index < studio.sections.len()) {
            let count = studio.sections.len();
            let mut move_to = None;
            ui.horizontal_wrapped(|ui| {
                let section = &mut studio.sections[index];
                let patterns = &studio.patterns;
                ui.add_enabled_ui(editable, |ui| {
                    ComboBox::from_id_salt("kind").selected_text(section.kind.label()).width(70.0).show_ui(ui, |ui| {
                        for kind in SectionKind::ALL {
                            ui.selectable_value(&mut section.kind, kind, kind.label());
                        }
                    });
                    ComboBox::from_id_salt("pattern").selected_text(&patterns[section.pattern].name).width(120.0).show_ui(ui, |ui| {
                        for (index, pattern) in patterns.iter().enumerate() {
                            ui.selectable_value(&mut section.pattern, index, &pattern.name);
                        }
                    });
                    if ui.add(DragValue::new(&mut section.repeats).range(1..=MAX_REPEATS).prefix("× "))
                        .on_hover_text("Set this section to a whole number of pattern repeats.").changed()
                    {
                        section.span = None;
                    }
                    if section.span.is_some() && action(ui, "Reset length", true).clicked() {
                        section.span = None;
                    }
                });
                let length = patterns[section.pattern].length;
                small(ui, if section.span.is_some() {
                    format!("{} bars · pattern bar {}", section.bars(length), section.offset(length) + 1)
                } else {
                    format!("{} bars", section.bars(length))
                });
                let pattern = section.pattern;
                let shared = studio.sections.iter().filter(|section| section.pattern == pattern).count() > 1;
                if action(ui, "Make unique", editable && shared && studio.patterns.len() < MAX_PATTERNS)
                    .on_hover_text("Copy this pattern and use the copy only in this section")
                    .clicked()
                {
                    let mut copy = studio.patterns[pattern].clone();
                    copy.name = studio.next_pattern_name();
                    copy.notes.clear_event_ids();
                    studio.sections[index].pattern = studio.patterns.len();
                    studio.patterns.push(copy);
                    studio.identify_saved_events();
                }
                if action(ui, "←", editable && index > 0).on_hover_text("Move section left").clicked() {
                    move_to = Some(index - 1);
                }
                if action(ui, "→", editable && index + 1 < count).on_hover_text("Move section right").clicked() {
                    move_to = Some(index + 1);
                }
                if action(ui, "Remove", editable).clicked() {
                    studio.sections.remove(index);
                    timeline.sections_changed();
                    timeline.selected = (!studio.sections.is_empty()).then(|| index.min(studio.sections.len() - 1));
                }
            });
            if let Some(to) = move_to {
                app.edit_arrangement(Edit::Move { from: index, to });
            }
        }
    }
    if automation_updated { app.update_playing_automation(); }
    let bars = app.project.studio.song_bars();
    let ready = (1..=MAX_SONG_BARS).contains(&bars);
    ui.horizontal_wrapped(|ui| {
        if primary(ui, if playing { "Stop song" } else { "Play song" }, playing || (ready && app.audio.output().is_some() && !app.recording)).clicked() {
            if playing { app.stop_song(); } else { app.play_song(); }
        }
        if let Some(index) = app.view.arrangement_timeline.selected {
            let can_play = ready && app.can_play_section();
            if action(ui, "Play from section", can_play).clicked() {
                app.play_section(index, false);
            }
            if action(ui, "Loop section", can_play).clicked() {
                app.play_section(index, true);
            }
        }
        if action(ui, "Export…", ready && app.busy.is_none()).clicked() {
            app.export_dialog = Some(super::export::Dialog::new(app));
        }
        small(ui, format!("{bars} bars total"));
    });
    if bars > MAX_SONG_BARS {
        ui.label(label(format!("Reduce the arrangement to {MAX_SONG_BARS} bars or fewer to play or export."), 12.0, GOLD));
    }
    if let Some((message, true)) = &app.status {
        ui.label(label(message, 12.0, GOLD));
    }
}

fn scene_launcher(app: &mut App, ui: &mut Ui) {
    if action(ui, "Resample song to pad", !app.project.studio.sections.is_empty() && app.can_add_source()
        && !app.recording && app.project.studio.pads.iter().any(Option::is_none))
        .on_hover_text("Render the arrangement and assign it to an empty pad")
        .clicked()
    {
        app.resample_mix(true);
    }
    ui.collapsing("Scene launcher", |ui| {
        if app.project.studio.patterns.is_empty() {
            small(ui, "Save a named pattern to launch it as a scene.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ComboBox::from_id_salt("scene_quantize")
                .selected_text(format!("Launch every {} bar(s)", app.scene_quantize))
                .show_ui(ui, |ui| {
                    for bars in [1, 2, 4] {
                        ui.selectable_value(&mut app.scene_quantize, bars, format!("{bars} bar(s)"));
                    }
                });
            if super::pill(ui, "Capture played bars", app.scene_capture).clicked() {
                app.scene_capture = !app.scene_capture;
            }
            let bars: usize = app.scene_take.iter().map(|run| run.bars).sum();
            if action(ui, &format!("Append take · {bars} bars"), bars > 0).clicked() {
                app.append_scene_take();
            }
            if action(ui, "Clear take", bars > 0).clicked() { app.scene_take.clear(); }
        });
        let mut launch = None;
        ui.horizontal_wrapped(|ui| {
            for (index, pattern) in app.project.studio.patterns.iter().enumerate() {
                let state = if app.scene_pending == Some(index) { " · queued" }
                    else if app.scene_active == Some(index) { " · playing" } else { "" };
                if action(ui, &format!("{}{state}", pattern.name), app.can_launch_scene()).clicked() {
                    launch = Some(index);
                }
            }
        });
        if let Some(index) = launch { app.launch_scene(index); }
        small(ui, "Scenes loop. Capture counts completed bars. Use Stop pattern to stop.");
    });
}
