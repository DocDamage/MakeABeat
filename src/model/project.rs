use super::{
    NamedSample, Record, Source, Studio,
    snapshot::{NamedSnapshot, SourceId},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub record: Option<Record>,
    pub studio: Studio,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<NamedSample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_ids: Vec<SourceId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Arc<Vec<NamedSnapshot>>,
}

impl Project {
    pub(crate) fn uses_extraction(&self) -> bool {
        self.record
            .iter()
            .chain(&self.studio.sources)
            .any(|record| matches!(record.source, Source::Extracted(_)))
    }
}
