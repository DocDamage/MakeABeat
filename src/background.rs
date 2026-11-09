use std::{
    cell::RefCell,
    io,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Default)]
pub struct State {
    pub stage: Option<&'static str>,
    pub progress: Option<f32>,
    pub cancelled: bool,
    pub publishing: bool,
}

#[derive(Clone, Default)]
pub struct Control(Arc<Mutex<State>>);

impl Control {
    pub fn state(&self) -> State {
        *self.0.lock().unwrap()
    }

    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap();
        if !state.publishing {
            state.cancelled = true;
        }
    }

    pub fn run<T>(&self, work: impl FnOnce() -> T) -> T {
        struct Restore(Option<Control>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|slot| slot.replace(self.0.take()));
            }
        }
        let previous = CURRENT.with(|slot| slot.replace(Some(self.clone())));
        let _restore = Restore(previous);
        work()
    }
}

thread_local! {
    // Work stays on its dedicated thread. The UI and synchronous callers have no control.
    static CURRENT: RefCell<Option<Control>> = const { RefCell::new(None) };
}

fn update(work: impl FnOnce(&mut State)) -> io::Result<()> {
    CURRENT.with_borrow(|control| {
        if let Some(control) = control {
            let mut state = control.0.lock().unwrap();
            if state.cancelled {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Operation cancelled",
                ));
            }
            work(&mut state);
        }
        Ok(())
    })
}

pub fn check() -> io::Result<()> {
    update(|_| {})
}

pub fn stage(name: &'static str) -> io::Result<()> {
    update(|state| {
        state.stage = Some(name);
        state.progress = None;
    })
}

pub fn progress(done: usize, total: usize) -> io::Result<()> {
    update(|state| state.progress = (total > 0).then(|| done as f32 / total as f32))
}

// Once publication starts, report the real result even if the window is closing.
pub fn publish() -> io::Result<()> {
    update(|state| {
        state.publishing = true;
        state.stage = Some("Publishing output");
        state.progress = None;
    })
}
