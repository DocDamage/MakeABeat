use super::super::{DIM, EDGE, GOLD, INSET, MINT, TEXT};
use super::automation::{AutomationClipboard, Editor, LaneView};
use crate::{
    app::arrangement_workflow::{Edge, Edit, trimmed_span},
    model::{STEPS, Section, SectionKind, SectionSpan, Studio},
};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};

const RULER: f32 = 22.0;
const TRACK: f32 = 56.0;

#[derive(Clone, Copy)]
enum DragMode {
    Move,
    Trim(Edge),
}

#[derive(Clone, Copy)]
struct Drag {
    index: usize,
    pointer: f32,
    mode: DragMode,
}

pub(crate) struct Timeline {
    pub selected: Option<usize>,
    pub bar_width: f32,
    reveal: bool,
    drag: Option<Drag>,
    pub(super) automation: Option<Editor>,
    pub(super) automation_clipboard: AutomationClipboard,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            selected: None,
            bar_width: 72.0,
            reveal: false,
            drag: None,
            automation: None,
            automation_clipboard: AutomationClipboard::default(),
        }
    }
}

impl Timeline {
    pub fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.reveal = true;
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
        if let Some(editor) = &mut self.automation {
            editor.cancel_stroke();
        }
    }

    pub fn sections_changed(&mut self) {
        self.drag = None;
        if let Some(editor) = &mut self.automation {
            editor.sections_changed();
        }
    }

    pub fn restore_sections(&mut self, previous: &[Section], restored: &[Section]) {
        self.sections_changed();
        let selected = self
            .selected
            .or_else(|| (previous.is_empty() && !restored.is_empty()).then_some(0));
        self.selected = selected.and_then(|index| {
            let last = restored.len().checked_sub(1)?;
            // A trim changes only this slot, even if another section matches its old span.
            if previous.len() == restored.len()
                && previous
                    .iter()
                    .zip(restored)
                    .enumerate()
                    .all(|(slot, (before, after))| slot == index || before == after)
            {
                return Some(index.min(last));
            }
            let matching = previous.get(index).and_then(|selected| {
                restored
                    .iter()
                    .enumerate()
                    .filter(|(_, section)| *section == selected)
                    .min_by_key(|(candidate, _)| index.abs_diff(*candidate))
                    .map(|(candidate, _)| candidate)
            });
            Some(matching.unwrap_or(index.min(last)))
        });
        self.reveal = self.selected.is_some() && previous != restored;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        studio: &mut Studio,
        playhead: Option<usize>,
        editable: bool,
    ) -> Option<Edit> {
        if !editable
            || self
                .drag
                .is_some_and(|drag| drag.index >= studio.sections.len())
        {
            self.cancel_drag();
        }
        let width = ui.available_width();
        ScrollArea::horizontal()
            .id_salt("section_timeline")
            .max_width(width)
            .auto_shrink([false, true])
            .drag_to_scroll(false)
            .show(ui, |ui| {
                let (width, edit) = self.canvas(ui, studio, playhead, editable, width);
                if let Some(editor) = &mut self.automation {
                    editor.lane(
                        ui,
                        studio,
                        LaneView {
                            width,
                            bar_width: self.bar_width,
                            playhead,
                            editable,
                            selected: self.selected,
                        },
                    );
                }
                edit
            })
            .inner
    }

    fn canvas(
        &mut self,
        ui: &mut Ui,
        studio: &Studio,
        playhead: Option<usize>,
        editable: bool,
        width: f32,
    ) -> (f32, Option<Edit>) {
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let mut bars = studio.song_bars();
        if let Some(Drag {
            index,
            pointer: origin,
            mode: DragMode::Trim(edge),
        }) = self.drag
            && let Some(pointer) = pointer
            && let Some(span) = trimmed_span(
                studio,
                index,
                edge,
                ((pointer.x - ui.cursor().left() - origin) / self.bar_width).round() as i32,
            )
        {
            let section = &studio.sections[index];
            let length = studio.patterns[section.pattern].length;
            bars = bars.max(bars - section.bars(length) + span.bars);
        }
        let (rect, response) = ui.allocate_exact_size(
            vec2(
                width.max((bars as f32 + 0.5) * self.bar_width),
                RULER + TRACK,
            ),
            Sense::click_and_drag(),
        );
        let lane = rect.with_min_y(rect.top() + RULER);
        let mut blocks = section_rects(studio, lane, self.bar_width, None);
        let hit = pointer.and_then(|pos| blocks.iter().position(|rect| rect.contains(pos)));
        if editable
            && let Some(index) = hit
            && let Some(pos) = pointer
            && response.hovered()
        {
            let cursor = match drag_mode(blocks[index], pos.x) {
                DragMode::Move => CursorIcon::Grab,
                DragMode::Trim(_) => CursorIcon::ResizeHorizontal,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        let pressed = (response.is_pointer_button_down_on() || response.clicked())
            && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        if editable
            && pressed
            && let (Some(index), Some(pos)) = (hit, pointer)
        {
            self.selected = Some(index);
            self.drag = Some(Drag {
                index,
                pointer: pos.x - rect.left(),
                mode: drag_mode(blocks[index], pos.x),
            });
        }

        let mut marker = None;
        let mut resized = None;
        let mut edit = None;
        if let (Some(drag), Some(pos)) = (self.drag, pointer) {
            let delta = pos.x - rect.left() - drag.pointer;
            let released = ui.input(|input| !input.pointer.primary_down());
            if delta.abs() >= 3.0 {
                match drag.mode {
                    DragMode::Move => {
                        let from = drag.index;
                        let center = blocks[from].center().x + delta;
                        let to = blocks
                            .iter()
                            .enumerate()
                            .filter(|(index, block)| *index != from && center > block.center().x)
                            .count();
                        if released {
                            if from != to {
                                edit = Some(Edit::Move { from, to });
                            }
                        } else {
                            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                            marker = Some(if to > from {
                                blocks[to].right()
                            } else {
                                blocks[to].left()
                            });
                        }
                    }
                    DragMode::Trim(edge) => {
                        let index = drag.index;
                        let delta = (delta / self.bar_width).round() as i32;
                        if let Some(span) = trimmed_span(studio, index, edge, delta) {
                            resized = Some((index, span));
                            blocks = section_rects(studio, lane, self.bar_width, resized);
                            if released {
                                edit = Some(Edit::Trim { index, edge, delta });
                            }
                        }
                        if !released {
                            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                        }
                    }
                }
                if !released {
                    let visible = ui.clip_rect();
                    let near_right = (pos.x - visible.right() + 20.0).clamp(0.0, 26.0);
                    let near_left = (visible.left() + 20.0 - pos.x).clamp(0.0, 26.0);
                    if near_left != near_right {
                        ui.scroll_with_delta(vec2(near_left - near_right, 0.0));
                        ui.ctx().request_repaint();
                    }
                }
            }
            if released {
                self.drag = None;
            }
        } else if ui.input(|input| !input.pointer.primary_down()) {
            self.drag = None;
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, INSET);
        let clip = ui.clip_rect();
        let first = ((clip.left() - rect.left()) / self.bar_width)
            .floor()
            .max(0.0) as usize;
        let last =
            ((clip.right().min(rect.right()) - rect.left()) / self.bar_width).ceil() as usize;
        for bar in first..=last {
            let x = rect.left() + bar as f32 * self.bar_width;
            painter.vline(x, rect.y_range(), Stroke::new(0.8_f32, EDGE));
            if self.bar_width >= 36.0 || bar.is_multiple_of(2) {
                painter.text(
                    pos2(x + 4.0, rect.top() + 10.0),
                    Align2::LEFT_CENTER,
                    bar + 1,
                    FontId::monospace(10.0),
                    DIM,
                );
            }
        }
        for (index, block) in blocks.iter().enumerate() {
            if !block.intersects(clip) {
                continue;
            }
            let section = &studio.sections[index];
            let pattern = &studio.patterns[section.pattern];
            let bars = resized
                .filter(|&(i, _)| i == index)
                .map_or(section.bars(pattern.length), |(_, span)| span.bars);
            let selected = self.selected == Some(index);
            let fill = section_color(section.kind);
            let block = block.shrink2(vec2(1.0, 3.0));
            painter.rect(
                block,
                4.0,
                fill,
                Stroke::new(
                    1.0_f32,
                    if selected {
                        GOLD
                    } else {
                        fill.gamma_multiply(1.4)
                    },
                ),
                StrokeKind::Inside,
            );
            for x in [block.left() + 3.0, block.right() - 3.0] {
                painter.vline(
                    x,
                    (block.top() + 14.0)..=(block.bottom() - 14.0),
                    Stroke::new(1.0_f32, TEXT.gamma_multiply(0.55)),
                );
            }
            let text = painter.with_clip_rect(block.shrink2(vec2(6.0, 1.0)));
            let kind = section.kind.label();
            text.text(
                block.left_top() + vec2(8.0, 7.0),
                Align2::LEFT_TOP,
                if block.width() < 58.0 {
                    &kind[..1]
                } else {
                    kind
                },
                FontId::proportional(13.0),
                TEXT,
            );
            text.text(
                block.left_bottom() + vec2(8.0, -7.0),
                Align2::LEFT_BOTTOM,
                format!("{} · {bars} bars", pattern.name),
                FontId::proportional(11.0),
                TEXT.gamma_multiply(0.82),
            );
            if !section.automation.is_empty() {
                painter.circle_filled(block.right_top() + vec2(-7.0, 6.0), 2.5, GOLD);
            }
        }
        if let Some(x) = marker {
            painter.vline(x, lane.y_range(), Stroke::new(3.0_f32, GOLD));
        }
        if let Some(step) = playhead {
            let x = rect.left() + step as f32 / STEPS as f32 * self.bar_width;
            painter.vline(x, rect.y_range(), Stroke::new(2.0_f32, MINT));
        }
        if let Some(index) = self.selected.filter(|&index| index < blocks.len())
            && std::mem::take(&mut self.reveal)
        {
            ui.scroll_to_rect(blocks[index], None);
        }
        if let Some(index) = hit.filter(|&index| index < studio.sections.len()) {
            let section = &studio.sections[index];
            let pattern = &studio.patterns[section.pattern];
            response.on_hover_text(format!(
                "{} · {} · {} bars{}",
                section.kind.label(),
                pattern.name,
                section.bars(pattern.length),
                if section.automation.is_empty() {
                    ""
                } else {
                    " · Linked automation"
                }
            ));
        }
        (rect.width(), edit)
    }
}

fn section_rects(
    studio: &Studio,
    lane: Rect,
    width: f32,
    resized: Option<(usize, SectionSpan)>,
) -> Vec<Rect> {
    let mut x = lane.left();
    studio
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| {
            let bars = resized.filter(|&(i, _)| i == index).map_or(
                section.bars(studio.patterns[section.pattern].length),
                |(_, span)| span.bars,
            );
            let end = x + bars as f32 * width;
            let rect = Rect::from_min_max(pos2(x, lane.top()), pos2(end, lane.bottom()));
            x = end;
            rect
        })
        .collect()
}

fn drag_mode(rect: Rect, x: f32) -> DragMode {
    let handle = (rect.width() * 0.24).min(10.0);
    if x <= rect.left() + handle {
        DragMode::Trim(Edge::Start)
    } else if x >= rect.right() - handle {
        DragMode::Trim(Edge::End)
    } else {
        DragMode::Move
    }
}

fn section_color(kind: SectionKind) -> Color32 {
    match kind {
        SectionKind::Intro => Color32::from_rgb(43, 75, 97),
        SectionKind::Verse => Color32::from_rgb(33, 91, 73),
        SectionKind::Hook => Color32::from_rgb(105, 64, 113),
        SectionKind::Bridge => Color32::from_rgb(107, 77, 47),
        SectionKind::Outro => Color32::from_rgb(72, 76, 95),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AutomationPoint, AutomationTarget, Section};

    #[test]
    fn timeline_drags_and_resizes_sections_and_duplicates_the_selected_span() {
        let ctx = egui::Context::default();
        let mut app = crate::app::App::new(ctx.clone());
        app.audio.remove_output();
        app.project.studio = Studio {
            bars: 2,
            ..Studio::default()
        };
        let studio = &mut app.project.studio;
        studio.hits[0][0][0] = 1;
        studio.save_pattern("Intro", false);
        studio.save_pattern("Verse", true);
        studio.save_pattern("Hook", true);
        studio.bars = 2;
        studio.hits[1][0][0] = 2;
        studio.sync_pattern();
        studio.sections = vec![
            Section::new(SectionKind::Intro, 0, 1),
            Section::new(SectionKind::Verse, 1, 1),
            Section::new(SectionKind::Hook, 2, 1),
        ];
        let target = AutomationTarget::MasterVolume;
        let intro = [AutomationPoint {
            tick: 0,
            percent: 23,
            curve: Default::default(),
        }];
        let hook = [AutomationPoint {
            tick: 0,
            percent: 61,
            curve: Default::default(),
        }];
        studio.sections[0]
            .automation
            .set_points(target, intro.to_vec());
        studio.sections[2]
            .automation
            .set_points(target, hook.to_vec());
        app.observe_edits(false);
        let render = |app: &mut crate::app::App, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 200.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ctx, |ui| {
                            if let Some(edit) =
                                app.view.arrangement_timeline
                                    .show(ui, &mut app.project.studio, None, true)
                            {
                                app.edit_arrangement(edit);
                            }
                        });
                },
            );
        };
        let drag = |app: &mut crate::app::App, from, to| {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            let start = pos2(from, 40.0);
            let end = pos2(to, 40.0);
            let before = app.project.studio.sections.clone();
            render(
                app,
                vec![egui::Event::PointerMoved(start), button(start, true)],
            );
            render(app, vec![egui::Event::PointerMoved(end)]);
            assert_eq!(app.project.studio.sections, before);
            render(app, vec![button(end, false)]);
        };
        render(&mut app, vec![]);
        let initial = app.project.studio.sections.clone();
        drag(&mut app, 72.0, 310.0);
        assert_eq!(
            app.project.studio
                .sections
                .iter()
                .map(|section| section.pattern)
                .collect::<Vec<_>>(),
            [1, 2, 0]
        );
        assert_eq!(app.project.studio.sections[2].automation.points(target), intro);
        assert_eq!(app.view.arrangement_timeline.selected, Some(2));
        app.undo_project(false);
        assert_eq!(app.project.studio.sections, initial);
        assert_eq!(app.view.arrangement_timeline.selected, Some(0));
        app.undo_project(true);
        assert_eq!(app.view.arrangement_timeline.selected, Some(2));

        drag(&mut app, 213.0, 285.0);
        assert_eq!(
            app.project.studio.sections[1].span,
            Some(SectionSpan { offset: 0, bars: 3 })
        );
        assert_eq!(app.project.studio.song_bars(), 6);
        drag(&mut app, 75.0, 147.0);
        assert_eq!(
            app.project.studio.sections[1].span,
            Some(SectionSpan { offset: 1, bars: 2 })
        );
        let song = app.project.studio.song_sequence().unwrap();
        assert_eq!(song.bars[1].drums[0][0], 2);
        assert_eq!(song.bars[3].drums[0][0], 1);
        app.undo_project(false);
        assert_eq!(
            app.project.studio.sections[1].span,
            Some(SectionSpan { offset: 0, bars: 3 })
        );
        app.undo_project(true);
        app.edit_arrangement(Edit::Duplicate(1));
        assert_eq!(app.project.studio.sections[2], app.project.studio.sections[1]);
        assert_eq!(app.view.arrangement_timeline.selected, Some(2));
        assert_eq!(app.project.studio.song_bars(), 7);
        app.undo_project(false);
        assert_eq!(app.project.studio.sections.len(), 3);
        assert_eq!(app.project.studio.sections[1].automation.points(target), hook);
        assert_eq!(app.view.arrangement_timeline.selected, Some(1));
    }
}
