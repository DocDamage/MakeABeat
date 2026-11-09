use super::{Command, Mixer, Studio};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
};

const COMMANDS_PER_CALLBACK: usize = 32;
const RETIRED_SETTINGS: usize = 128;

#[derive(Clone)]
pub(crate) struct CommandSender(Arc<Mutex<Option<VecDeque<Command>>>>);

impl CommandSender {
    pub(crate) fn send(&self, command: Command) {
        let Ok(mut shared) = self.0.lock() else {
            return;
        };
        let Some(queue) = shared.as_mut() else { return };
        // A tempo or instrument transition can reset audition or capture state.
        // Keep those transitions and all intervening commands in order.
        if let (Some(Command::Settings(previous)), Command::Settings(next)) =
            (queue.back_mut(), &command)
            && previous.bpm == next.bpm
            && previous.same_capture_instruments(next)
        {
            let replaced = queue.pop_back();
            queue.push_back(command);
            drop(shared);
            drop(replaced);
        } else {
            queue.push_back(command);
        }
    }
}

pub(crate) struct CommandReceiver {
    shared: CommandSender,
    batch: VecDeque<Command>,
    retire: SyncSender<Box<Studio>>,
    pending_retirement: Option<Box<Studio>>,
}

pub(crate) fn channel() -> (CommandSender, CommandReceiver, Receiver<Box<Studio>>) {
    let send = CommandSender(Arc::new(Mutex::new(Some(VecDeque::new()))));
    let (retire, retired) = sync_channel(RETIRED_SETTINGS);
    let receive = CommandReceiver {
        shared: send.clone(),
        batch: VecDeque::with_capacity(COMMANDS_PER_CALLBACK),
        retire,
        pending_retirement: None,
    };
    (send, receive, retired)
}

impl CommandReceiver {
    fn fill_batch(&mut self) {
        // Leave the commands queued if a producer is active. The render thread
        // never waits on the queue, and the lock is released before mixing.
        if let Ok(mut shared) = self.shared.0.try_lock()
            && let Some(queue) = shared.as_mut()
        {
            let count = (COMMANDS_PER_CALLBACK - self.batch.len()).min(queue.len());
            self.batch.extend(queue.drain(..count));
        }
    }

    pub(super) fn apply(&mut self, mixer: &mut Mixer) {
        if let Some(settings) = self.pending_retirement.take()
            && !self.retire(settings)
        {
            return;
        }
        self.fill_batch();
        while let Some(command) = self.batch.pop_front() {
            if let Command::Settings(mut settings) = command {
                // Reuse the incoming box to move the previous settings back to the UI.
                mixer.replace_settings(&mut settings);
                if !self.retire(settings) {
                    break;
                }
            } else {
                mixer.command(command);
            }
        }
    }

    fn retire(&mut self, settings: Box<Studio>) -> bool {
        match self.retire.try_send(settings) {
            Ok(()) => true,
            Err(TrySendError::Full(settings) | TrySendError::Disconnected(settings)) => {
                self.pending_retirement = Some(settings);
                false
            }
        }
    }

    #[cfg(all(test, windows))]
    pub(crate) fn take_commands(&mut self) -> Vec<Command> {
        self.fill_batch();
        self.batch.drain(..).collect()
    }
}

impl Drop for CommandReceiver {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.0.lock() {
            let queued = shared.take();
            drop(shared);
            drop(queued);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(bpm: u16, level: f32) -> Command {
        Command::Settings(Box::new(Studio {
            bpm,
            sample_level: level,
            ..Studio::default()
        }))
    }

    #[test]
    fn consecutive_settings_coalesce_without_crossing_transport_or_tempo_changes() {
        let (send, mut receive, retired) = channel();
        let mut mixer = Mixer::new(48_000);
        let bpm = mixer.settings.bpm;
        let initial_level = mixer.settings.sample_level;
        send.send(settings(bpm, 0.1));
        send.send(settings(bpm, 0.2));
        send.send(Command::ClockStart(false));
        send.send(settings(bpm, 0.3));
        send.send(settings(bpm + 1, 0.4));
        send.send(settings(bpm + 1, 0.5));

        receive.apply(&mut mixer);
        assert_eq!(mixer.settings.sample_level, 0.5);
        assert_eq!(mixer.settings.bpm, bpm + 1);
        let levels: Vec<_> = retired
            .try_iter()
            .map(|settings| settings.sample_level)
            .collect();
        assert_eq!(levels, [initial_level, 0.2, 0.3]);
    }

    #[test]
    fn busy_queue_and_command_bursts_leave_work_for_the_next_callback() {
        let (send, mut receive, _retired) = channel();
        let mut mixer = Mixer::new(48_000);
        let bpm = mixer.settings.bpm;
        for _ in 0..COMMANDS_PER_CALLBACK {
            send.send(Command::Fill(true));
        }
        send.send(Command::Fill(false));
        send.send(settings(bpm, 0.25));

        let producer = send.0.lock().unwrap();
        receive.apply(&mut mixer);
        assert!(!mixer.fill);
        drop(producer);
        receive.apply(&mut mixer);
        assert!(mixer.fill);
        receive.apply(&mut mixer);
        assert!(!mixer.fill);
        assert_eq!(mixer.settings.sample_level, 0.25);
    }

    #[test]
    fn retirement_backpressure_keeps_the_old_settings_and_command_order() {
        let (send, mut receive, retired) = channel();
        let mut mixer = Mixer::new(48_000);
        let bpm = mixer.settings.bpm;
        for index in 0..=RETIRED_SETTINGS {
            // Keep each settings transition distinct.
            send.send(settings(bpm + index as u16 % 2, 0.25));
        }
        send.send(Command::Fill(true));
        for _ in 0..=(RETIRED_SETTINGS / COMMANDS_PER_CALLBACK) {
            receive.apply(&mut mixer);
        }
        assert!(receive.pending_retirement.is_some());
        assert!(!mixer.fill);
        receive.apply(&mut mixer);
        assert!(!mixer.fill);

        assert_eq!(retired.try_iter().count(), RETIRED_SETTINGS);
        receive.apply(&mut mixer);
        assert!(receive.pending_retirement.is_none());
        assert_eq!(retired.try_iter().count(), 1);
        assert!(mixer.fill);
    }
}
