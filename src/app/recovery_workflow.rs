use crate::{
    audio::Clip,
    background::Control,
    files::recovery,
    model::project::Project,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

const INTERVAL: Duration = Duration::from_secs(20);

enum Work {
    Save {
        generation: u64,
        slot: usize,
        snapshot: Arc<Project>,
        clip: Option<Arc<Clip>>,
        control: Control,
    },
    Clear(PathBuf),
}

enum Reply {
    Saved(u64, Result<(), String>),
    Cleared(Result<(), String>),
}

pub(super) struct Recovery {
    dir: PathBuf,
    worker: Sender<Work>,
    replies: Receiver<Reply>,
    pending: Option<(Arc<Project>, Control)>,
    cleanups: usize,
    needs_cleanup: bool,
    generation: u64,
    slot: usize,
    saved: Option<Arc<Project>>,
    last_attempt: Instant,
}

impl Recovery {
    pub fn new(dir: PathBuf, ctx: egui::Context) -> Self {
        let (worker, work) = mpsc::channel();
        let (send, replies) = mpsc::channel();
        let worker_dir = dir.clone();
        std::thread::spawn(move || {
            let mut writer = recovery::Writer::default();
            while let Ok(work) = work.recv() {
                let reply = match work {
                    Work::Save {
                        generation,
                        slot,
                        snapshot,
                        clip,
                        control,
                    } => {
                        let result = control.run(|| {
                            crate::background::check()?;
                            writer.save(&worker_dir, slot, &snapshot, clip.as_deref())
                        });
                        Reply::Saved(generation, result.map_err(|error| error.to_string()))
                    }
                    Work::Clear(dir) => {
                        let result = if dir == worker_dir {
                            writer.clear(&dir)
                        } else {
                            recovery::remove(&dir)
                        };
                        Reply::Cleared(result.map_err(|error| error.to_string()))
                    }
                };
                if send.send(reply).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            dir,
            worker,
            replies,
            pending: None,
            cleanups: 0,
            needs_cleanup: false,
            generation: 0,
            slot: 0,
            saved: None,
            last_attempt: Instant::now(),
        }
    }

    pub fn save(
        &mut self,
        snapshot: &Arc<Project>,
        clip: Option<&Arc<Clip>>,
    ) -> Result<(), String> {
        if self.pending.is_some()
            || self.last_attempt.elapsed() < INTERVAL
            || self
                .saved
                .as_ref()
                .is_some_and(|saved| Arc::ptr_eq(saved, snapshot))
        {
            return Ok(());
        }
        self.last_attempt = Instant::now();
        let control = Control::default();
        self.worker
            .send(Work::Save {
                generation: self.generation,
                slot: self.slot,
                snapshot: snapshot.clone(),
                clip: clip.cloned(),
                control: control.clone(),
            })
            .map_err(|_| "The recovery worker stopped unexpectedly.".to_owned())?;
        self.pending = Some((snapshot.clone(), control));
        self.needs_cleanup = true;
        Ok(())
    }

    pub fn poll(&mut self) -> Option<String> {
        let mut error = None;
        loop {
            match self.replies.try_recv() {
                Ok(Reply::Saved(generation, result)) if generation == self.generation => {
                    let pending = self.pending.take();
                    match result {
                        Ok(()) => {
                            self.slot = (self.slot + 1) % recovery::SLOT_COUNT;
                            self.saved = pending.map(|(snapshot, _)| snapshot);
                        }
                        Err(message) => {
                            error = Some(format!("Could not save recovery snapshot. {message}"))
                        }
                    }
                }
                Ok(Reply::Saved(..)) => {}
                Ok(Reply::Cleared(result)) => {
                    self.cleanups -= 1;
                    if let Err(message) = result {
                        error = Some(format!("Could not remove recovery snapshots. {message}"));
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.pending.take().is_some() || self.cleanups > 0 {
                        error = Some("The recovery worker stopped unexpectedly.".into());
                    }
                    self.cleanups = 0;
                    break;
                }
            }
        }
        error
    }

    pub fn clear(&mut self) {
        if let Some((_, control)) = self.pending.take() {
            control.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.slot = 0;
        self.saved = None;
        self.last_attempt = Instant::now();
        if std::mem::take(&mut self.needs_cleanup) {
            self.remove(self.dir.clone());
        }
    }

    pub fn clear_if_needed(&mut self) {
        if self.needs_cleanup {
            self.clear();
        }
    }

    pub fn remove(&mut self, dir: PathBuf) {
        if self.worker.send(Work::Clear(dir)).is_ok() {
            self.cleanups += 1;
        }
    }

    pub fn cleaning(&self) -> bool {
        self.cleanups > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_during_a_write_cannot_resurrect_the_old_session() -> crate::files::Result<()> {
        let folder = tempfile::tempdir()?;
        let dir = folder.path().join("session");
        let mut state = Recovery::new(dir.clone(), egui::Context::default());
        state.last_attempt = Instant::now() - INTERVAL;
        state.save(&Arc::new(Project::default()), None)?;
        state.clear();
        assert!(state.cleaning());
        let mut project = Project::default();
        project.studio.bpm = 111;
        state.last_attempt = Instant::now() - INTERVAL;
        state.save(&Arc::new(project), None)?;
        let until = Instant::now() + Duration::from_secs(5);
        while state.cleaning() || state.pending.is_some() {
            assert!(state.poll().is_none());
            assert!(Instant::now() < until, "recovery worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(state.slot, 1);
        assert_eq!(std::fs::read_dir(&dir)?.count(), 1);
        let path = recovery::find(folder.path()).expect("current recovery snapshot");
        assert_eq!(recovery::open(&path)?.0.studio.bpm, 111);
        Ok(())
    }
}
