use crate::{background, files::Result};
use eframe::egui::Context;
use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
};

const MAX_QUEUED: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Interactive,
    Foreground,
    Indexing,
}

type Work<T> = Arc<dyn Fn() -> Result<T> + Send + Sync>;

struct Task<T> {
    label: &'static str,
    priority: Priority,
    work: Work<T>,
}

struct Running<T> {
    task: Task<T>,
    control: background::Control,
    done: mpsc::Receiver<std::result::Result<T, String>>,
    resume: bool,
}

pub struct Queue<T> {
    pending: VecDeque<Task<T>>,
    running: Option<Running<T>>,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            running: None,
        }
    }
}

impl<T: Send + 'static> Queue<T> {
    pub fn submit(
        &mut self,
        ctx: &Context,
        label: &'static str,
        priority: Priority,
        work: impl Fn() -> Result<T> + Send + Sync + 'static,
    ) -> Result<()> {
        if self.pending.len() >= MAX_QUEUED {
            return Err("The analysis queue is full.".into());
        }
        self.pending.push_back(Task {
            label,
            priority,
            work: Arc::new(work),
        });
        if let Some(running) = &mut self.running
            && running.task.priority == Priority::Indexing
            && priority < Priority::Indexing
        {
            running.resume = true;
            running.control.cancel();
        }
        self.start(ctx);
        Ok(())
    }

    fn start(&mut self, ctx: &Context) {
        if self.running.is_some() {
            return;
        }
        let Some(index) = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, task)| task.priority)
            .map(|(index, _)| index)
        else {
            return;
        };
        let task = self.pending.remove(index).unwrap();
        let control = background::Control::default();
        let worker = control.clone();
        let work = task.work.clone();
        let ctx = ctx.clone();
        let (tx, done) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = worker.run(|| {
                background::check()?;
                work()
            });
            let _ = tx.send(result.map_err(|error| error.to_string()));
            ctx.request_repaint();
        });
        self.running = Some(Running {
            task,
            control,
            done,
            resume: false,
        });
    }

    pub fn poll(&mut self, ctx: &Context) -> Option<std::result::Result<T, String>> {
        let running = self.running.as_ref()?;
        let result = match running.done.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The analysis task ended unexpectedly.".into())
            }
        };
        let running = self.running.take().unwrap();
        let cancelled = running.control.state().cancelled;
        if cancelled && running.resume {
            self.pending.push_back(running.task);
        }
        self.start(ctx);
        (!cancelled).then_some(result)
    }

    pub fn current(&self) -> Option<(&'static str, background::State, usize)> {
        let running = self.running.as_ref()?;
        Some((
            running.task.label,
            running.control.state(),
            self.pending.len(),
        ))
    }

    pub fn cancel(&mut self) {
        if let Some(running) = &mut self.running {
            running.resume = false;
            running.control.cancel();
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.cancel();
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        if let Some(running) = &self.running {
            running.control.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    #[test]
    fn previews_run_before_pending_analysis_and_indexing_resumes() {
        let ctx = Context::default();
        let mut queue = Queue::default();
        let attempts = Arc::new(AtomicUsize::new(0));
        let indexing = attempts.clone();
        let (started, ready) = mpsc::sync_channel(1);
        queue
            .submit(&ctx, "index", Priority::Indexing, move || {
                if indexing.fetch_add(1, Ordering::SeqCst) == 0 {
                    started.send(()).unwrap();
                    loop {
                        background::check()?;
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                Ok("index")
            })
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        queue
            .submit(&ctx, "analysis", Priority::Foreground, || Ok("analysis"))
            .unwrap();
        queue
            .submit(&ctx, "preview", Priority::Interactive, || Ok("preview"))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut order = Vec::new();
        while queue.current().is_some() && Instant::now() < deadline {
            if let Some(result) = queue.poll(&ctx) {
                order.push(result.unwrap());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(order, ["preview", "analysis", "index"]);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
