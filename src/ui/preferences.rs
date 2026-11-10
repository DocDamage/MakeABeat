use super::{action, named_number, small};
use crate::{app::{App, actions::Action}, model::preferences::Preferences};
use eframe::egui::{self, Context, DragValue, ScrollArea};

#[derive(Clone, Copy)]
pub(super) struct ControlColors {
    pub edge: egui::Color32,
    pub secondary_text: egui::Color32,
    pub accent: egui::Color32,
    pub focus: egui::Color32,
    pub emphasis: f32,
}

pub(super) fn control_colors(preferences: Preferences) -> ControlColors {
    if preferences.high_contrast {
        ControlColors {
            edge: egui::Color32::from_gray(133),
            secondary_text: egui::Color32::from_gray(221),
            accent: super::MINT,
            focus: egui::Color32::WHITE,
            emphasis: 2.0,
        }
    } else {
        ControlColors {
            edge: super::EDGE,
            secondary_text: super::DIM,
            accent: super::GREEN,
            focus: super::TEXT,
            emphasis: 1.0,
        }
    }
}

pub(crate) fn apply(preferences: Preferences, ctx: &Context) {
    ctx.set_zoom_factor(if preferences.zoom.is_finite() {
        preferences.zoom.clamp(0.8, 1.45)
    } else {
        1.0
    });
    super::style(ctx);
    ctx.style_mut(|style| {
        if preferences.high_contrast {
            let colors = control_colors(preferences);
            style.visuals.widgets.inactive.bg_stroke.color = colors.edge;
            style.visuals.widgets.hovered.bg_stroke.color = colors.focus;
            style.visuals.selection.stroke = egui::Stroke::new(colors.emphasis, colors.accent);
        }
        if preferences.reduce_motion {
            style.animation_time = 0.0;
        }
    });
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
        let search = ui.add(egui::TextEdit::singleline(&mut app.view.commands.query).hint_text("Find a command or shortcut"));
        if app.view.commands.focus {
            search.request_focus();
            app.view.commands.focus = false;
        }
        let submit_search = (search.has_focus() || search.lost_focus())
            && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let query = app.view.commands.query.trim().to_lowercase();
        ScrollArea::vertical().max_height((ctx.screen_rect().height() * 0.46).min(230.0)).show(ui, |ui| {
            let mut first = true;
            let mut matched = false;
            for &command in Action::PALETTE.iter().filter(|command| command.matches(&query)) {
                matched = true;
                let reason = command.unavailable(app);
                let enabled = reason.is_none();
                let shortcut = command.shortcut_text();
                let pressed = ui.horizontal_wrapped(|ui| {
                    let clicked = action(ui, command.title(), enabled).clicked();
                    if !shortcut.is_empty() { small(ui, shortcut); }
                    if let Some(reason) = reason { small(ui, reason); }
                    clicked
                }).inner;
                if pressed || (first && enabled && submit_search) {
                    run = Some(command);
                }
                if enabled { first = false; }
            }
            if !matched {
                small(ui, "No matching commands.");
            }
        });
        small(ui, "Tab moves focus, Enter or Space activates a control, and Escape clears focus. Ctrl+K opens these commands.");
        small(ui, "Backtick switches performance keys. In Queue, use arrows to select a track, Space to preview, and K, J or L to decide.");
        small(ui, "Waveform: Z and C set boundaries, X toggles looping. Pad mode uses Z, X and C to play pads.");
    });
    if app.library.interface != initial {
        apply(app.library.interface, ctx);
        app.persist_library();
    }
    if let Some(command) = run {
        app.view.commands.open = false;
        command.run(app);
    } else if close || modal.should_close() {
        app.view.commands.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::{Page, StudioView}, ui::sample::Tool};

    #[test]
    fn palette_enter_runs_matching_tools_and_respects_disabled_actions() {
        let ctx = Context::default();
        let mut app = App::new(ctx.clone());
        app.audio.remove_output();
        let frame = |app: &mut App, events| {
            let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |ctx| show(app, ctx));
        };
        let enter = || egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        Action::Commands.run(&mut app);
        app.view.commands.query = "slice selection".into();
        frame(&mut app, vec![]);
        frame(&mut app, vec![enter()]);
        assert!(app.view.commands.open);
        assert_eq!(app.view.sample_tool, None);

        app.view.commands.query = "sam audio".into();
        app.view.commands.focus = true;
        frame(&mut app, vec![]);
        frame(&mut app, vec![enter()]);
        assert!(!app.view.commands.open);
        assert_eq!(app.view.page, Page::Studio);
        assert_eq!(app.view.studio_view, StudioView::Sample);
        assert_eq!(app.view.sample_tool, Some(Tool::Extract));
    }
}
