use super::{BG, BLUE, INSET, MINT, action, job_controls, primary, responsive_modal, small};
use crate::{
    app::App,
    model::{
        DRUMS, MAX_BARS, STEPS, SequenceBar,
        drum_templates::{FAMILIES, TEMPLATES},
        drum_velocity,
    },
};
use eframe::egui::{
    Align2, ComboBox, Context, FontId, Label, ScrollArea, Sense, TextEdit, Ui, WidgetInfo,
    WidgetType, vec2,
};

#[derive(Default)]
pub(crate) struct Browser {
    query: String,
    family: Option<usize>,
    style: Option<&'static str>,
    selected: usize,
    length: Option<usize>,
    inspected_bar: usize,
    use_groove: bool,
    preview_request: Option<(usize, bool)>,
    preview_started: bool,
}

impl Browser {
    pub fn show(&mut self, app: &mut App, ctx: &Context) -> bool {
        let mut close = false;
        let modal = responsive_modal(ctx, "drum_templates", 630.0, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Drum templates");
                close = action(ui, "Close", true).clicked();
            });
            ui.horizontal_wrapped(|ui| {
                let previous_family = self.family;
                let filter = ui.label("Genre family");
                ComboBox::from_id_salt("genre_family")
                    .selected_text(self.family.map_or("All genres", |family| FAMILIES[family]))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.family, None, "All genres");
                        for (index, family) in FAMILIES.into_iter().enumerate() {
                            ui.selectable_value(&mut self.family, Some(index), family);
                        }
                    })
                    .response
                    .labelled_by(filter.id);
                if self.family != previous_family {
                    self.style = None;
                }
                if let Some(family) = self.family {
                    let label = ui.label("Style");
                    ComboBox::from_id_salt("template_style")
                        .selected_text(self.style.unwrap_or("All styles"))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.style, None, "All styles");
                            let styles: std::collections::BTreeSet<_> = TEMPLATES
                                .iter()
                                .filter(|template| template.family == family)
                                .map(|template| template.genre)
                                .collect();
                            for style in styles {
                                ui.selectable_value(&mut self.style, Some(style), style);
                            }
                        })
                        .response
                        .labelled_by(label.id);
                }
                let search = ui.label("Search");
                ui.add(
                    TextEdit::singleline(&mut self.query)
                        .desired_width(185.0)
                        .hint_text("Genre or beat name"),
                )
                .labelled_by(search.id);
            });

            let previous_selection = self.selected;
            let query = self.query.to_lowercase();
            let terms: Vec<_> = query
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .collect();
            let matches = ui
                .horizontal_wrapped(|ui| {
                    let label = ui.label("Length");
                    ComboBox::from_id_salt("template_length")
                        .selected_text(self.length.map_or_else(|| "Any length".into(), bar_count))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.length, None, "Any length");
                            for length in [1, 2, 4] {
                                ui.selectable_value(
                                    &mut self.length,
                                    Some(length),
                                    bar_count(length),
                                );
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    let matches: Vec<_> = TEMPLATES
                        .iter()
                        .enumerate()
                        .filter(|(_, template)| {
                            if self.family.is_some_and(|family| family != template.family)
                                || self.style.is_some_and(|style| style != template.genre)
                                || self
                                    .length
                                    .is_some_and(|length| length != template.bars.len())
                            {
                                return false;
                            }
                            let searchable = format!(
                                "{} {} {}",
                                template.name, template.genre, FAMILIES[template.family]
                            )
                            .to_lowercase();
                            terms.iter().all(|word| searchable.contains(word))
                        })
                        .collect();
                    small(
                        ui,
                        format!("{} of {} templates", matches.len(), TEMPLATES.len()),
                    );
                    matches
                })
                .inner;
            if matches.is_empty() {
                small(ui, "No templates match these filters.");
                return;
            }
            if !matches.iter().any(|(index, _)| *index == self.selected) {
                self.selected = matches[0].0;
            }
            ScrollArea::vertical()
                .id_salt("template_list")
                .max_height(114.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (index, template) in matches {
                        ui.selectable_value(
                            &mut self.selected,
                            index,
                            format!(
                                "{} · {} · {}",
                                template.genre,
                                template.name,
                                bar_count(template.bars.len())
                            ),
                        );
                    }
                });
            if self.selected != previous_selection {
                self.inspected_bar = 0;
            }
            let template = &TEMPLATES[self.selected];
            let length = template.bars.len();
            let suggested_swing = (100.0 * template.swing).round();
            ui.horizontal_wrapped(|ui| {
                ui.strong(template.name);
                small(
                    ui,
                    format!(
                        "{} · {} · {} BPM · {suggested_swing:.0}% swing",
                        template.genre,
                        bar_count(length),
                        template.bpm
                    ),
                );
            });
            small(ui, template.description);
            let bar = app.bar;
            let end = template.end_bar(bar);
            let target = if length == 1 {
                format!("bar {}", bar + 1)
            } else {
                format!("bars {}–{}", bar + 1, bar + length)
            };
            ui.checkbox(&mut self.use_groove, "Use suggested tempo and swing")
                .on_hover_text("Changes the project tempo and swing when you apply the template.");
            ui.horizontal_wrapped(|ui| {
                let preview = if length == 1 { "Play preview" } else { "Play phrase" };
                if action(ui, preview, app.can_preview_drum_template())
                    .on_disabled_hover_text(
                        "Stop playback or recording and wait for the current job to finish.",
                    )
                    .clicked()
                {
                    self.preview_request = Some((self.selected, self.use_groove));
                    app.preview_drum_template(self.selected, self.use_groove);
                }
                let listening = self.preview_started
                    && app
                        .audio.output()
                        .is_some_and(|output| output.meters.listening());
                if action(ui, "Stop preview", listening || app.drum_preview_pending()).clicked() {
                    self.preview_request = None;
                    self.preview_started = false;
                    app.stop_drum_preview();
                }
                let can_apply = end.is_some() && !app.playing && !app.recording
                    && app.input_recording.is_none();
                if primary(ui, &format!("Apply to {target}"), can_apply)
                    .on_disabled_hover_text("The template must fit within the pattern limit. Stop playback and recording before applying.")
                    .clicked()
                {
                    close = app.project.studio.apply_drum_template(bar, template, self.use_groove);
                }
                if length > 1 {
                    ui.separator();
                    for (index, bar) in template.bars.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.inspected_bar,
                            index,
                            format!("Bar {}", index + 1),
                        )
                        .on_hover_text(bar.label);
                    }
                }
            });
            if app.drum_preview_pending() {
                ui.horizontal_wrapped(|ui| job_controls(app, ui));
            } else if let Some((message, true)) = &app.status {
                ui.label(message);
            }
            if let Some(end) = end {
                let extension = if end > app.project.studio.bars {
                    format!(" Extends the pattern to {end} bars.")
                } else {
                    String::new()
                };
                small(
                    ui,
                    format!("Replaces drum hits and conditions in {target}.{extension}"),
                );
            } else {
                small(
                    ui,
                    format!(
                        "Choose an earlier start bar to fit this phrase within the {MAX_BARS}-bar limit."
                    ),
                );
            }
            if length > 1 {
                small(
                    ui,
                    format!(
                        "Bar {} · {}",
                        self.inspected_bar + 1,
                        template.bars[self.inspected_bar].label
                    ),
                );
            }
            preview_grid(
                ui,
                &template.bars[self.inspected_bar].steps(),
                self.inspected_bar + 1,
            );
        });
        let close = close || modal.should_close();
        if close && (self.preview_started || app.drum_preview_pending()) {
            app.stop_drum_preview();
        }
        close
    }

    pub fn accept_preview(&mut self, index: usize, use_groove: bool) -> bool {
        let accepted = self.preview_request.take() == Some((index, use_groove))
            && self.selected == index
            && self.use_groove == use_groove;
        self.preview_started |= accepted;
        accepted
    }
}

fn bar_count(length: usize) -> String {
    if length == 1 {
        "1 bar".into()
    } else {
        format!("{length} bars")
    }
}

fn preview_grid(ui: &mut Ui, pattern: &SequenceBar, bar: usize) {
    let cell_width =
        ((ui.available_width() - 76.0 - 3.0 * STEPS as f32) / STEPS as f32).clamp(12.0, 28.0);
    ScrollArea::horizontal()
        .id_salt("template_steps")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(|ui| {
                ui.allocate_space(vec2(76.0, 16.0));
                for beat in 1..=4 {
                    let width = 4.0 * cell_width + 9.0;
                    ui.add_sized([width, 16.0], Label::new(beat.to_string()));
                }
            });
            for (lane, hits) in pattern.drums.iter().enumerate() {
                if hits.iter().all(|&hit| hit == 0) {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.add_sized([76.0, 16.0], Label::new(DRUMS[lane]).truncate());
                    for (step, &hit) in hits.iter().enumerate() {
                        let velocity = drum_velocity(hit);
                        let repeats = pattern
                            .rules
                            .get(&(((lane + 2) * STEPS + step) as u8))
                            .map_or(1, |rule| rule.repeats);
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(cell_width, 15.0), Sense::hover());
                        let color = match velocity {
                            0 => INSET,
                            1..=55 => BLUE.gamma_multiply(0.45),
                            56..=100 => BLUE,
                            _ => MINT,
                        };
                        ui.painter().rect_filled(rect, 3.0, color);
                        if repeats > 1 {
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, repeats,
                                FontId::monospace(11.0), BG);
                        }
                        if velocity != 0 {
                            let text = format!(
                                "{} · phrase bar {bar} · step {} · velocity {velocity} · {repeats} hit(s)",
                                DRUMS[lane],
                                step + 1
                            );
                            response.widget_info(|| {
                                WidgetInfo::labeled(WidgetType::Label, true, &text)
                            });
                            response.on_hover_text(text);
                        }
                    }
                });
            }
        });
}
