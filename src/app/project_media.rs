use crate::audio::Clip;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct ProjectMedia {
    root: Option<Arc<Clip>>,
    detached: Option<Arc<Clip>>,
    active_source: usize,
}

impl ProjectMedia {
    pub(crate) fn root(&self) -> Option<&Arc<Clip>> {
        self.root.as_ref()
    }

    pub(crate) fn root_clip(&self) -> Option<&Clip> {
        self.root.as_deref()
    }

    pub(crate) fn active_source(&self) -> usize {
        self.active_source
    }

    pub(super) fn select(&mut self, index: usize) {
        self.active_source = index;
    }

    pub(crate) fn replace(&mut self, root: Option<Arc<Clip>>) {
        *self = Self {
            root,
            ..Self::default()
        };
    }

    pub(super) fn append(&mut self, mut clip: Clip, visible_extras: usize) -> usize {
        self.detached = None;
        let Some(root) = &mut self.root else {
            self.root = Some(Arc::new(clip));
            return 0;
        };
        let children = std::mem::take(&mut clip.sources);
        let root = Arc::make_mut(root);
        // Undo can leave a suffix available for redo. A new import replaces that branch.
        root.sources.truncate(visible_extras);
        root.sources.push(Arc::new(clip));
        root.sources.extend(children);
        visible_extras + 1
    }

    pub(super) fn detach_for_undo(&mut self) {
        self.detached = self.root.take();
    }

    pub(super) fn restore_for_redo(&mut self) {
        self.root = self.detached.take();
    }
}
