use super::App;
use crate::{
    audio::{Command, Output, OutputSettings},
    model::{Studio, playback::PlaybackSettings},
};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct AudioRuntime {
    output: Option<Output>,
    error: Option<String>,
    settings: PlaybackSettings,
    seeked_at: Option<Instant>,
}

impl AudioRuntime {
    pub(super) fn open(settings: &OutputSettings) -> Self {
        match Output::open(settings) {
            Ok(output) => Self {
                output: Some(output),
                ..Self::default()
            },
            Err(error) => Self {
                error: Some(error),
                ..Self::default()
            },
        }
    }

    pub(crate) fn output(&self) -> Option<&Output> {
        self.output.as_ref()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(super) fn send(&self, command: Command) {
        if let Some(output) = &self.output {
            output.send(command);
        }
    }

    pub(super) fn capture_matches(&self, studio: &Studio) -> bool {
        self.settings.capture_matches(studio)
    }

    pub(super) fn sync(&mut self, studio: &Studio) {
        if self.settings.matches(studio) {
            return;
        }
        let settings = PlaybackSettings::from(studio);
        if let Some(output) = &self.output {
            if !self.capture_matches(studio) {
                output.performance.invalidate_preview();
            }
            output.send(Command::Settings(Box::new(settings.clone())));
        }
        self.settings = settings;
    }

    pub(super) fn seek(&mut self, seconds: f64) {
        self.seeked_at = Some(Instant::now());
        self.send(Command::Seek(seconds));
    }

    pub(super) fn poll(&mut self) {
        if let Some(output) = &mut self.output {
            // Retire the settings on the app thread before the bounded queue fills.
            output.retire_settings();
            output.performance.poll();
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_output(&mut self) {
        self.output = None;
    }

    #[cfg(test)]
    pub(super) fn settings(&self) -> &PlaybackSettings {
        &self.settings
    }
}

impl App {
    pub fn retry_audio(&mut self) {
        self.set_audio_output(self.library.audio.clone());
    }

    pub fn set_audio_output(&mut self, settings: OutputSettings) {
        if self.recording || self.input_recording.is_some() {
            return;
        }
        let output = match Output::open(&settings) {
            Ok(output) => output,
            Err(error) => {
                if self.audio.output.is_some() {
                    self.status = Some((error, true));
                } else {
                    self.audio.error = Some(error);
                }
                return;
            }
        };
        self.stop_all();
        self.audio.output = Some(output);
        self.audio.error = None;
        self.audio.settings = PlaybackSettings::from(&self.project.studio);
        self.send(Command::Settings(Box::new(self.audio.settings.clone())));
        self.send(Command::Clip(self.media.root().cloned()));
        self.send(Command::PreviewSource(self.media.active_source()));
        self.send(Command::ClickTrack(self.click_track));
        self.send(Command::ClickGain(self.click_gain));
        self.send(Command::Fill(self.fill));
        if self.midi_clock_connection.is_some() {
            self.midi.route_clock(
                self.midi_clock_connection,
                self.audio.output().map(Output::sender),
                self.project.studio.bpm,
            );
        }
        self.seek(self.position);
        if self.library.audio != settings {
            self.library.audio = settings;
            self.persist_library();
        }
    }

    pub(super) fn poll_audio(&mut self) {
        self.audio.poll();
        let Some(output) = self.audio.output() else {
            return;
        };
        if self
            .audio
            .seeked_at
            .is_none_or(|time| time.elapsed() > Duration::from_millis(60))
        {
            self.position = output.meters.position().min(self.duration());
        }
        self.preview = output.meters.preview();
        self.step = output.meters.step();
        self.scene_active = output.meters.scene();
        if self.scene_active.is_some() && self.scene_active == self.scene_pending {
            self.scene_pending = None;
        }
        if self.song_playing == Some(output.meters.finished_song()) {
            self.playing = false;
            self.song_playing = None;
        }
        if output.meters.failed() {
            self.audio.error = Some("Audio device disconnected.".into());
            self.audio.output = None;
            self.recording = false;
            self.recorded_gates.clear();
            self.playing = false;
            self.song_playing = None;
            self.preview = false;
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::{audio::Clip, files};
    use std::{sync::Arc, thread};

    #[test]
    #[ignore = "requires a working Windows audio output device"]
    fn windows_audio_reconnect_and_settings_retirement() -> files::Result<()> {
        let mut app = App::new(eframe::egui::Context::default());
        app.library.audio = OutputSettings::default();
        app.retry_audio();
        let initial = app.audio.output().ok_or("No audio output")?.meters.clone();
        let unavailable = OutputSettings {
            device: Some("Lost Samples test device that does not exist".into()),
            ..Default::default()
        };
        app.set_audio_output(unavailable);
        assert!(Arc::ptr_eq(&initial, &app.audio.output().unwrap().meters));
        assert!(app.status.as_ref().is_some_and(|(_, error)| *error));
        assert!(app.library.audio == OutputSettings::default());

        app.media.replace(Some(Arc::new(Clip::new(8_000, vec![[0.0; 2]; 8_000]))));
        app.project.studio.bpm = 122;
        app.retry_audio();
        let reopened = app
            .audio
            .output()
            .ok_or("Audio output failed to reopen")?
            .meters
            .clone();
        assert!(!Arc::ptr_eq(&initial, &reopened));
        assert!(app.audio.error().is_none());
        assert!(app.audio.settings().matches(&app.project.studio));

        // Alternating tempos preserve every command and exceed the retire queue capacity.
        for index in 0..192 {
            app.project.studio.bpm = 100 + index % 2;
            app.sync();
        }
        app.seek(0.25);
        let start = Instant::now();
        while (reopened.position() - 0.25).abs() > 0.001 && start.elapsed() < Duration::from_secs(3)
        {
            app.poll_audio();
            thread::sleep(Duration::from_millis(5));
        }
        assert!((reopened.position() - 0.25).abs() <= 0.001);
        assert!(!reopened.failed());
        Ok(())
    }
}
