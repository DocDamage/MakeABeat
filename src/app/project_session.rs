use super::recovery_workflow::Recovery;
use crate::{
    audio::Clip,
    files::{self, Project},
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const UNDO_LIMIT: usize = 64;

pub(super) struct Session {
    pub path: Option<PathBuf>,
    pub current: Arc<Project>,
    saved: Arc<Project>,
    restored: Option<PathBuf>,
    undo: Vec<Arc<Project>>,
    redo: Vec<Arc<Project>>,
    grouped: bool,
    recovery: Option<Recovery>,
    pub available_recovery: Option<PathBuf>,
    pub show_recovery: bool,
}

impl Session {
    pub fn new(project: Project, nonce: u64, ctx: egui::Context) -> Self {
        let root = files::app_data_dir().map(|path| path.join("recovery"));
        let available_recovery = root.as_deref().and_then(files::recovery::find);
        let current = Arc::new(project);
        Self {
            path: None,
            saved: current.clone(),
            current,
            restored: None,
            undo: Vec::new(),
            redo: Vec::new(),
            grouped: false,
            recovery: root.map(|path| {
                Recovery::new(
                    path.join(format!("{:x}-{nonce:x}", std::process::id())),
                    ctx,
                )
            }),
            show_recovery: available_recovery.is_some(),
            available_recovery,
        }
    }

    pub fn dirty(&self) -> bool {
        self.restored.is_some()
            || (!Arc::ptr_eq(&self.current, &self.saved) && self.current != self.saved)
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
        self.remove_restored_recovery();
        if !self.dirty() {
            self.clear_recovery();
        }
    }

    pub fn autosave(&mut self, clip: Option<&Arc<Clip>>, enabled: bool) -> Option<String> {
        let dirty = self.dirty();
        let recovery = self.recovery.as_mut()?;
        let error = recovery.poll();
        if !dirty {
            recovery.clear_if_needed();
        } else if enabled && let Err(message) = recovery.save(&self.current, clip) {
            return Some(message);
        }
        error
    }

    pub fn cleaning_recovery(&self) -> bool {
        self.recovery.as_ref().is_some_and(Recovery::cleaning)
    }

    pub fn clear_recovery(&mut self) {
        if let Some(recovery) = &mut self.recovery {
            recovery.clear();
        }
    }

    pub fn discard(&mut self) {
        self.remove_restored_recovery();
        self.clear_recovery();
    }

    fn remove_restored_recovery(&mut self) {
        if let Some(dir) = self.restored.take().as_deref().and_then(Path::parent)
            && let Some(recovery) = &mut self.recovery
        {
            recovery.remove(dir.to_owned());
        }
    }
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
        let mut session = Session::new(snapshot(90), 1, egui::Context::default());
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
