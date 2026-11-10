use super::{action, responsive_modal, small};
use crate::{
    app::App,
    model::{
        MAX_SONG_BARS,
        snapshot::{MAX_SNAPSHOT_BYTES, MAX_SNAPSHOTS},
    },
};
use eframe::egui::{ComboBox, Context, DragValue};

pub(crate) struct Dialog {
    selected: usize,
    name: String,
    song: bool,
    bar: usize,
    match_level: bool,
    error: Option<String>,
}

impl Default for Dialog {
    fn default() -> Self {
        Self {
            selected: 0,
            name: String::new(),
            song: false,
            bar: 1,
            match_level: false,
            error: None,
        }
    }
}

impl Dialog {
    pub fn show(&mut self, app: &mut App, ctx: &Context) -> bool {
        let mut close = false;
        self.selected = self.selected.min(app.project.snapshots.len().saturating_sub(1));
        let modal = responsive_modal(ctx, "project_snapshots", 580.0, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Project snapshots");
                close = action(ui, "Close", true).clicked();
            });
            small(
                ui,
                format!(
                    "{} of {MAX_SNAPSHOTS} snapshots · {} MiB metadata limit · Shared audio",
                    app.project.snapshots.len(),
                    MAX_SNAPSHOT_BYTES / (1024 * 1024)
                ),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut self.name);
                let ready = !app.recording
                    && !self.name.trim().is_empty()
                    && app.project.snapshots.len() < MAX_SNAPSHOTS;
                if action(ui, "Save snapshot", ready).clicked() {
                    match app.save_named_snapshot(&self.name) {
                        Ok(()) => {
                            self.name.clear();
                            self.error = None;
                            self.selected = app.project.snapshots.len() - 1;
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
            });
            if !app.project.snapshots.is_empty() {
                let settings = (self.selected, self.song, self.bar, self.match_level);
                ui.horizontal_wrapped(|ui| {
                    ComboBox::from_id_salt("snapshot_selection")
                        .selected_text(&app.project.snapshots[self.selected].name)
                        .show_ui(ui, |ui| {
                            for (index, snapshot) in app.project.snapshots.iter().enumerate() {
                                ui.selectable_value(&mut self.selected, index, &snapshot.name);
                            }
                        });
                    if action(ui, "Restore", !app.playing && !app.recording).clicked() {
                        close = app.restore_named_snapshot(self.selected);
                        if !close {
                            self.error = Some("The snapshot's sources are unavailable.".into());
                        }
                    }
                    if action(ui, "Delete", !app.recording)
                        .on_hover_text("Delete this snapshot. Undo restores it.")
                        .clicked()
                    {
                        app.delete_named_snapshot(self.selected);
                        self.selected = self.selected.min(app.project.snapshots.len().saturating_sub(1));
                    }
                });
                if !app.project.snapshots.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.song, false, "Pattern");
                        ui.selectable_value(&mut self.song, true, "Song");
                        ui.add(
                            DragValue::new(&mut self.bar)
                                .range(1..=MAX_SONG_BARS)
                                .prefix("Start bar "),
                        );
                        ui.checkbox(&mut self.match_level, "Match levels");
                    });
                    if settings != (self.selected, self.song, self.bar, self.match_level) {
                        app.clear_snapshot_preview();
                    }
                    small(
                        ui,
                        "Both previews restart from the chosen bar and play up to four bars at their saved tempos.",
                    );
                    ui.horizontal_wrapped(|ui| {
                        let ready = app.audio.output().is_some() && app.busy.is_none() && !app.recording;
                        if action(ui, "Prepare comparison", ready).clicked() {
                            self.error = app
                                .prepare_snapshot_preview(
                                    self.selected,
                                    self.song,
                                    self.bar - 1,
                                    self.match_level,
                                )
                                .err()
                                .map(|error| error.to_string());
                        }
                        let prepared =
                            app.snapshot_preview_ready() && app.audio.output().is_some() && !app.recording;
                        if action(ui, "A · Working", prepared).clicked() {
                            app.play_snapshot_preview(0);
                        }
                        if action(ui, "B · Snapshot", prepared).clicked() {
                            app.play_snapshot_preview(1);
                        }
                        if action(ui, "Stop", prepared).clicked() {
                            app.send(crate::audio::Command::StopListening);
                        }
                    });
                }
            }
            if let Some(error) = &self.error {
                small(ui, error);
            }
        });
        if close || modal.should_close() {
            app.clear_snapshot_preview();
            return false;
        }
        true
    }
}
