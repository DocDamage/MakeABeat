use crate::files::{self, Project};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const UNDO_LIMIT: usize = 64;
const RECOVERY_INTERVAL: Duration = Duration::from_secs(20);
const RECOVERY_SLOTS: [&str; 2] = ["snapshot-0.flip", "snapshot-1.flip"];

pub(super) struct Session {
    pub path: Option<PathBuf>,
    pub current: Arc<Project>,
    saved: Arc<Project>,
    restored: Option<PathBuf>,
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    grouped: bool,
    recovery_dir: Option<PathBuf>,
    pub available_recovery: Option<PathBuf>,
    pub show_recovery: bool,
    recovery_slot: usize,
    recovered_snapshot: Option<Arc<Project>>,
    last_recovery: Instant,
}

impl Session {
    pub fn new(project: Project, nonce: u64) -> Self {
        let root = files::app_data_dir().map(|path| path.join("recovery"));
        let available_recovery = root.as_deref().and_then(find_recovery);
        let current = Arc::new(project);
        Self {
            path: None,
            saved: current.clone(),
            current,
            restored: None,
            undo: Vec::new(),
            redo: Vec::new(),
            grouped: false,
            recovery_dir: root.map(|path| path.join(format!("{:x}-{nonce:x}", std::process::id()))),
            show_recovery: available_recovery.is_some(),
            available_recovery,
            recovery_slot: 0,
            recovered_snapshot: None,
            last_recovery: Instant::now(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.restored.is_some() || self.current != self.saved
    }

    pub fn reset(&mut self, project: Project, path: Option<PathBuf>, restored: Option<PathBuf>) {
        self.clear_recovery();
        self.path = path;
        self.current = Arc::new(project);
        self.saved = self.current.clone();
        self.restored = restored;
        self.undo.clear();
        self.redo.clear();
        self.grouped = false;
        self.last_recovery = Instant::now();
    }

    pub fn observe(&mut self, project: Project, group: bool) {
        if *self.current != project {
            if !self.grouped {
                if self.undo.len() == UNDO_LIMIT {
                    self.undo.remove(0);
                }
                self.undo.push(self.current.clone());
            }
            self.redo.clear();
            self.current = Arc::new(project);
            self.grouped = group;
        } else if !group {
            self.grouped = false;
        }
    }

    pub fn can_travel(&self, redo: bool) -> bool {
        if redo {
            !self.redo.is_empty()
        } else {
            !self.undo.is_empty()
        }
    }

    pub fn end_group(&mut self) {
        self.grouped = false;
    }

    pub fn travel(&mut self, redo: bool) -> Option<Arc<Project>> {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let project = from.pop()?;
        to.push(std::mem::replace(&mut self.current, project.clone()));
        self.grouped = false;
        Some(project)
    }

    pub fn mark_saved(&mut self, path: PathBuf, project: Arc<Project>) {
        self.path = Some(path);
        self.saved = project;
        if let Some(dir) = self.restored.take().as_deref().and_then(Path::parent) {
            remove_snapshots(dir);
        }
        if !self.dirty() {
            self.clear_recovery();
        }
    }

    pub fn recovery_target(&mut self) -> Option<PathBuf> {
        if !self.dirty()
            || self.last_recovery.elapsed() < RECOVERY_INTERVAL
            || self.recovered_snapshot.as_ref() == Some(&self.current)
        {
            return None;
        }
        let path = self
            .recovery_dir
            .as_ref()?
            .join(RECOVERY_SLOTS[self.recovery_slot]);
        self.last_recovery = Instant::now();
        Some(path)
    }

    pub fn recovery_written(&mut self, project: Arc<Project>) {
        self.recovery_slot = (self.recovery_slot + 1) % RECOVERY_SLOTS.len();
        self.recovered_snapshot = Some(project);
        if !self.dirty() {
            self.clear_recovery();
        }
    }

    pub fn clear_recovery(&mut self) {
        if let Some(path) = self.recovery_dir.as_deref() {
            remove_snapshots(path);
        }
        self.recovered_snapshot = None;
    }

    pub fn discard(&mut self) {
        if let Some(path) = self.restored.take().as_deref().and_then(Path::parent) {
            remove_snapshots(path);
        }
        self.clear_recovery();
    }
}

fn find_recovery(root: &Path) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .flat_map(|entry| RECOVERY_SLOTS.map(|name| entry.path().join(name)))
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn remove_snapshots(dir: &Path) {
    for name in RECOVERY_SLOTS {
        let _ = fs::remove_file(dir.join(name));
    }
    let _ = fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Studio;

    fn snapshot(bpm: u16) -> Project {
        Project {
            record: None,
            studio: Studio {
                bpm,
                ..Studio::default()
            },
            samples: Vec::new(),
            ..Project::default()
        }
    }

    #[test]
    fn finishing_an_older_save_preserves_newer_edits_and_undo_reaches_saved_state() {
        let mut session = Session::new(snapshot(90), 1);
        session.recovery_dir = None;
        session.observe(snapshot(100), true);
        let saving = session.current.clone();
        session.observe(snapshot(110), true);
        session.end_group();
        session.mark_saved(PathBuf::from("beat.flip"), saving);
        assert!(session.dirty());
        assert_eq!(session.travel(false).unwrap().studio.bpm, 90);
        assert_eq!(session.travel(true).unwrap().studio.bpm, 110);
        session.observe(snapshot(100), false);
        assert!(!session.dirty());
    }
}
