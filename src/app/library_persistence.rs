use crate::{files, model::Library};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(350);
const STOPPED: &str = "The crate persistence worker stopped unexpectedly.";
pub(super) type RestoreResult = Result<Option<PathBuf>, String>;

enum Work {
    Save(Box<Library>),
    Restore(Arc<files::LibraryRecovery>),
}

enum Reply {
    Saved(Result<(), String>),
    Restored(RestoreResult),
}

#[derive(Clone, Copy)]
enum Pending {
    Save,
    Restore,
}

pub(super) struct Persistence {
    worker: Sender<Work>,
    replies: Receiver<Reply>,
    active: Option<Pending>,
    save_at: Option<Instant>,
    pub error: Option<String>,
}

impl Persistence {
    pub fn new(ctx: egui::Context) -> Self {
        Self::with_writer(ctx, |library| {
            files::write_library(library).map_err(|error| error.to_string())
        })
    }

    fn with_writer(
        ctx: egui::Context,
        write: impl Fn(&Library) -> Result<(), String> + Send + 'static,
    ) -> Self {
        let (worker, work) = mpsc::channel();
        let (send, replies) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(work) = work.recv() {
                let reply = match work {
                    Work::Save(library) => Reply::Saved(write(&library)),
                    Work::Restore(recovery) => {
                        Reply::Restored(recovery.restore().map_err(|error| error.to_string()))
                    }
                };
                if send.send(reply).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            worker,
            replies,
            active: None,
            save_at: None,
            error: None,
        }
    }

    pub fn changed(&mut self) {
        self.save_at = Some(Instant::now() + DEBOUNCE);
        self.error = None;
    }

    pub fn retry(&mut self) {
        self.save_at = Some(Instant::now());
        self.error = None;
    }

    pub fn pending(&self) -> bool {
        self.active.is_some() || self.save_at.is_some()
    }

    pub fn restoring(&self) -> bool {
        matches!(self.active, Some(Pending::Restore))
    }

    pub fn restore(&mut self, recovery: Arc<files::LibraryRecovery>) -> Result<(), String> {
        if self.active.is_none() {
            self.worker
                .send(Work::Restore(recovery))
                .map_err(|_| STOPPED.to_owned())?;
            self.active = Some(Pending::Restore);
        }
        Ok(())
    }

    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        library: &Library,
        flush: bool,
    ) -> Option<RestoreResult> {
        let restored = match self.replies.try_recv() {
            Ok(Reply::Saved(result)) => {
                self.active = None;
                self.error = result.err();
                if self.error.is_some() {
                    ctx.request_repaint();
                }
                None
            }
            Ok(Reply::Restored(result)) => {
                self.active = None;
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => match self.active.take() {
                Some(Pending::Save) => {
                    self.error = Some(STOPPED.into());
                    ctx.request_repaint();
                    None
                }
                Some(Pending::Restore) => Some(Err(STOPPED.into())),
                None => None,
            },
        };
        if let Some(at) = self.save_at
            && self.active.is_none()
        {
            let remaining = at.saturating_duration_since(Instant::now());
            if flush || remaining.is_zero() {
                self.save_at = None;
                if self
                    .worker
                    .send(Work::Save(Box::new(library.clone())))
                    .is_ok()
                {
                    self.active = Some(Pending::Save);
                } else {
                    self.error = Some(STOPPED.into());
                    ctx.request_repaint();
                }
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
        restored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump_until_idle(persistence: &mut Persistence, ctx: &egui::Context, library: &Library) {
        let until = Instant::now() + Duration::from_secs(5);
        while persistence.pending() {
            persistence.poll(ctx, library, true);
            assert!(Instant::now() < until, "crate save did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn flush_writes_the_latest_snapshot_after_an_in_flight_write() {
        let ctx = egui::Context::default();
        let (started, writes) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let mut persistence = Persistence::with_writer(ctx.clone(), move |library| {
            started
                .send((library.heard, std::thread::current().id()))
                .unwrap();
            resume.recv().unwrap();
            Ok(())
        });
        let mut library = Library::default();
        persistence.changed();
        persistence.save_at = Some(Instant::now() + Duration::from_secs(60));
        library.heard = 1;
        persistence.poll(&ctx, &library, false);
        assert!(persistence.active.is_none());
        assert!(writes.try_recv().is_err());

        persistence.poll(&ctx, &library, true);
        let (heard, worker) = writes.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(heard, 1);
        assert_ne!(worker, std::thread::current().id());
        for heard in [2, 3, 4] {
            library.heard = heard;
            persistence.changed();
            persistence.poll(&ctx, &library, true);
        }
        release.send(()).unwrap();

        let until = Instant::now() + Duration::from_secs(5);
        let (heard, next_worker) = loop {
            persistence.poll(&ctx, &library, true);
            if let Ok(saved) = writes.try_recv() {
                break saved;
            }
            assert!(Instant::now() < until, "latest crate write did not start");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(heard, 4);
        assert_eq!(worker, next_worker);
        release.send(()).unwrap();
        pump_until_idle(&mut persistence, &ctx, &library);
        assert!(persistence.error.is_none());
        assert!(writes.try_recv().is_err());
    }

    #[test]
    fn failed_save_can_retry_with_the_current_library() {
        let ctx = egui::Context::default();
        let (saved, writes) = mpsc::channel();
        let mut persistence = Persistence::with_writer(ctx.clone(), move |library| {
            if library.heard == 0 {
                Err("test write failure".into())
            } else {
                saved.send(library.heard).unwrap();
                Ok(())
            }
        });
        let mut library = Library::default();
        persistence.changed();
        pump_until_idle(&mut persistence, &ctx, &library);
        assert_eq!(persistence.error.as_deref(), Some("test write failure"));
        library.heard = 7;
        persistence.retry();
        pump_until_idle(&mut persistence, &ctx, &library);
        assert_eq!(writes.try_recv(), Ok(7));
        assert!(persistence.error.is_none());
    }
}
