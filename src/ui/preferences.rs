use super::{action, named_number, small};
use crate::app::{App, Page, StudioView};
use eframe::egui::{self, Context, DragValue, ScrollArea};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub zoom: f32,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    pub show_pads: bool,
    pub stereo_waveform: bool,
    pub crate_sort: usize,
    pub pads_width: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            high_contrast: false,
            reduce_motion: false,
            show_pads: true,
            stereo_waveform: false,
            crate_sort: 0,
            pads_width: 410.0,
        }
    }
}

impl Preferences {
    pub fn apply(self, ctx: &Context) {
        ctx.set_zoom_factor(if self.zoom.is_finite() {
            self.zoom.clamp(0.8, 1.45)
        } else {
            1.0
        });
        super::style(ctx);
        ctx.style_mut(|style| {
            if self.high_contrast {
                style.visuals.widgets.inactive.bg_stroke.color = egui::Color32::from_gray(133);
                style.visuals.widgets.hovered.bg_stroke.color = egui::Color32::WHITE;
                style.visuals.selection.stroke = egui::Stroke::new(2.0_f32, super::MINT);
            }
            if self.reduce_motion {
                style.animation_time = 0.0;
            }
        });
    }
}

enum Command {
    Open,
    Save,
    Import,
    Undo,
    Redo,
    Preview,
    Play,
    Stop,
    Dig,
    Queue,
    Crate,
    Studio(StudioView),
}

pub fn show(app: &mut App, ctx: &Context) {
    let mut close = false;
    let mut run = None;
    let initial = app.library.interface;
    let modal = super::responsive_modal(ctx, "help_and_commands", 530.0, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Commands and settings");
            close = action(ui, "Close", true).clicked();
        });
        ui.horizontal_wrapped(|ui| {
            let scale = ui.add(DragValue::new(&mut app.library.interface.zoom)
                .range(0.8..=1.45).speed(0.02).fixed_decimals(2).prefix("UI scale ").suffix("×"));
            let scale_value = (f64::from(app.library.interface.zoom) * 100.0).round() / 100.0;
            named_number(scale, "UI scale", scale_value);
            ui.checkbox(&mut app.library.interface.high_contrast, "High contrast controls");
            ui.checkbox(&mut app.library.interface.reduce_motion, "Reduce motion");
            ui.checkbox(&mut app.library.interface.show_pads, "Show pads beside waveform");
        });
        let search = ui.add(egui::TextEdit::singleline(&mut app.command_query).hint_text("Find a command or shortcut"));
        if app.focus_commands {
            search.request_focus();
            app.focus_commands = false;
        }
        let submit_search = (search.has_focus() || search.lost_focus())
            && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let query = app.command_query.trim().to_lowercase();
        let busy = app.busy.is_some() || app.input_recording.is_some();
        let items = [
            ("Open project", "Ctrl+O", !busy, Command::Open),
            ("Save project", "Ctrl+S", !busy, Command::Save),
            ("Import audio", "Ctrl+I", !busy, Command::Import),
            ("Undo project edit", "Ctrl+Z", app.can_undo_project(false), Command::Undo),
            ("Redo project edit", "Ctrl+Shift+Z", app.can_undo_project(true), Command::Redo),
            ("Preview source", "Space", app.clip.is_some(), Command::Preview),
            ("Play or stop pattern", "Enter", app.output.is_some(), Command::Play),
            ("Stop all playback", "", app.output.is_some(), Command::Stop),
            ("Open Dig", "", true, Command::Dig),
            ("Open Queue", "", true, Command::Queue),
            ("Open Crate", "", true, Command::Crate),
            ("Open waveform editor", "", true, Command::Studio(StudioView::Sample)),
            ("Open performance controls", "", true, Command::Studio(StudioView::Perform)),
            ("Open step sequencer", "", true, Command::Studio(StudioView::Sequence)),
            ("Open song arrangement", "", true, Command::Studio(StudioView::Arrange)),
            ("Open mixer", "", true, Command::Studio(StudioView::Mix)),
        ];
        ScrollArea::vertical().max_height((ctx.screen_rect().height() * 0.46).min(230.0)).show(ui, |ui| {
            let mut first = true;
            for (name, shortcut, enabled, command) in items {
                if !format!("{name} {shortcut}").to_lowercase().contains(&query) {
                    continue;
                }
                let pressed = ui.horizontal(|ui| {
                    let clicked = action(ui, name, enabled).clicked();
                    if !shortcut.is_empty() { small(ui, shortcut); }
                    clicked
                }).inner;
                if pressed || (first && enabled && submit_search) {
                    run = Some(command);
                }
                if enabled { first = false; }
            }
        });
        small(ui, "Tab moves focus, Enter or Space activates a control, and Escape clears focus. Ctrl+K opens these commands.");
        small(ui, "Backtick switches performance keys. In Queue, use arrows to select a track, Space to preview, and K, J or L to decide.");
        small(ui, "Waveform: Z and C set boundaries, X toggles looping. Pad mode uses Z, X and C to play pads.");
    });
    if app.library.interface != initial {
        app.library.interface.apply(ctx);
        app.persist_library();
    }
    if let Some(command) = run {
        close = true;
        match command {
            Command::Open => app.open_dialog(),
            Command::Save => app.save_dialog(),
            Command::Import => app.import(),
            Command::Undo => app.undo_project(false),
            Command::Redo => app.undo_project(true),
            Command::Preview => app.toggle_preview(),
            Command::Play => app.toggle_pattern(),
            Command::Stop => app.stop_all(),
            Command::Dig => app.page = Page::Dig,
            Command::Queue => app.page = Page::Queue,
            Command::Crate => app.page = Page::Crate,
            Command::Studio(view) => {
                app.page = Page::Studio;
                if view == StudioView::Arrange {
                    app.open_arrangement();
                } else {
                    app.studio_view = view;
                }
            }
        }
    }
    if close || modal.should_close() {
        app.show_commands = false;
    }
}
