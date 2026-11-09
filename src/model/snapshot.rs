use super::{NamedSample, Record, Studio};
use serde::{Deserialize, Serialize};

pub const MAX_SNAPSHOTS: usize = 8;
pub const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedSnapshot {
    pub name: String,
    pub created_utc: u64,
    pub source_ids: Vec<SourceId>,
    studio: Studio,
    samples: Vec<NamedSample>,
}

impl NamedSnapshot {
    pub fn uses_instruments(&self) -> bool {
        self.studio.uses_instruments()
    }

    pub fn uses_transcription(&self) -> bool {
        self.studio.uses_transcription()
    }

    pub fn uses_grooves(&self) -> bool {
        self.studio.uses_grooves()
    }

    pub fn uses_beat_maps(&self) -> bool {
        self.studio.uses_beat_maps()
    }

    pub fn new(
        name: String,
        created_utc: u64,
        source_ids: Vec<SourceId>,
        mut studio: Studio,
        samples: Vec<NamedSample>,
    ) -> Self {
        // The project's source registry owns media and descriptive metadata.
        studio.sources.clear();
        Self {
            name,
            created_utc,
            source_ids,
            studio,
            samples,
        }
    }

    pub fn resolve(
        &self,
        registry: &[Record],
        source_ids: &[SourceId],
    ) -> Option<(Studio, Vec<NamedSample>)> {
        let indices: Vec<_> = self
            .source_ids
            .iter()
            .map(|id| source_ids.iter().position(|candidate| candidate == id))
            .collect::<Option<_>>()?;
        let index = |old: usize| indices.get(old).copied().unwrap_or_default();
        let mut studio = self.studio.clone();
        studio.sources = registry.to_vec();
        for source in &mut studio.pad_sources {
            *source = index(*source);
        }
        for source in studio.drum_sources.iter_mut().flatten() {
            *source = index(*source);
        }
        let key_sources = std::mem::take(&mut studio.key_sources);
        for &key in studio.key_chops.keys() {
            let source = index(key_sources.get(&key).copied().unwrap_or_default());
            if source != 0 {
                studio.key_sources.insert(key, source);
            }
        }
        if let Some(chromatic) = &mut studio.chromatic {
            chromatic.source = index(chromatic.source);
        }
        studio.loop_source = index(studio.loop_source);
        studio.instruments.remap_sources(index);
        for note in studio.performance.iter_mut().chain(studio.patterns.iter_mut().flat_map(|p| &mut p.notes.performance)) {
            if let Some(expression) = &mut note.expression
                && let super::melody::Instrument::Sample { source, .. } = &mut std::sync::Arc::make_mut(expression).instrument
            { *source = index(*source); }
        }
        studio.warp_markers = std::mem::take(&mut studio.warp_markers)
            .into_iter()
            .map(|(source, markers)| (index(source), markers))
            .collect();
        studio.beat_maps = std::mem::take(&mut studio.beat_maps)
            .into_iter().map(|(source, map)| (index(source), map)).collect();
        studio.warp_sources = std::mem::take(&mut studio.warp_sources)
            .into_iter().map(|(source, mut warp)| {
                warp.parent = index(warp.parent);
                (index(source), warp)
            }).collect();
        let mut samples = self.samples.clone();
        for sample in &mut samples {
            sample.source = index(sample.source);
        }
        Some((studio, samples))
    }
}

pub fn valid_source_ids(ids: &[SourceId], count: usize) -> bool {
    ids.len() == count
        && ids
            .iter()
            .enumerate()
            .all(|(at, id)| id.0 > 0 && !ids[..at].contains(id))
}
