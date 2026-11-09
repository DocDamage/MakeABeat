use super::{App, Page};
use crate::{
    analysis_jobs::{Priority, Queue},
    files,
    library_index::Index,
    model::{Source, Verdict},
    recommendations::{self, FEEDBACK_LIMIT, Item, RANK_LIMIT, Ranked},
};
use std::time::{Duration, Instant};

type Ranking = (u64, Vec<(usize, Ranked)>, bool);

#[derive(Default)]
pub(crate) struct Taste {
    jobs: Queue<Ranking>,
    revision: u64,
    requested: Option<Instant>,
    initialized: bool,
    ranked: Vec<(usize, Ranked)>,
    local_index_unavailable: bool,
    listening: Option<(Source, bool, u64)>,
}

impl App {
    pub(crate) fn invalidate_taste(&mut self) {
        self.taste.revision += 1;
        self.taste.jobs.clear();
        self.taste.ranked.clear();
        self.taste.local_index_unavailable = false;
        self.taste.requested = self.library.taste.enabled.then(Instant::now);
        if !self.library.taste.enabled {
            self.taste.listening = None;
        }
    }

    pub fn set_taste_options(&mut self, enabled: bool, adventure: u8, save: bool) {
        let profile = &mut self.library.taste;
        if profile.enabled != enabled || profile.adventure != adventure {
            profile.enabled = enabled;
            profile.adventure = adventure;
            self.invalidate_taste();
        }
        if save {
            self.persist_library();
        }
    }

    pub fn reset_taste(&mut self) {
        self.library.taste.reset();
        self.invalidate_taste();
        self.persist_library();
    }

    fn taste_applied(&self) -> bool {
        self.library.taste.enabled && self.queue_tab == Verdict::Pending && !self.queue_similar
    }

    pub fn queue_indices(&self) -> Vec<usize> {
        let base = self
            .library
            .queue
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.queue_visible(entry))
            .map(|(index, _)| index);
        if self.taste_applied() && !self.taste.ranked.is_empty() {
            self.taste
                .ranked
                .iter()
                .map(|(index, _)| *index)
                .chain(base.skip(self.taste.ranked.len()))
                .collect()
        } else {
            base.collect()
        }
    }

    pub fn queue_ranking(&self, index: usize) -> Option<&Ranked> {
        if !self.taste_applied() {
            return None;
        }
        self.taste
            .ranked
            .iter()
            .find_map(|(candidate, ranked)| (*candidate == index).then_some(ranked))
    }

    pub fn taste_status(&self) -> &'static str {
        if !self.library.taste.enabled {
            return "Using the original queue order.";
        }
        if !self.taste_applied() {
            return "Taste ranking applies to the To hear view with Similar matches only off.";
        }
        if self.taste.requested.is_some() || self.taste.jobs.current().is_some() {
            return "Updating taste ranking…";
        }
        if self.library.taste.counts().0 == 0 {
            return "Keep a track to start building your profile.";
        }
        if self.taste.ranked.is_empty() {
            return "Queue order is active until there is a match to kept audio or catalog details.";
        }
        if self.taste.local_index_unavailable {
            return "Ranked with catalog details. The local audio index could not be read.";
        }
        "Ranked using the available catalog details and indexed audio."
    }

    pub(super) fn poll_taste(&mut self) {
        if self.page == Page::Queue && !self.taste.initialized {
            self.taste.initialized = true;
            self.invalidate_taste();
        }
        if let Some(Ok((revision, ranked, unavailable))) = self.taste.jobs.poll(&self.ctx)
            && revision == self.taste.revision
        {
            self.taste.ranked = ranked;
            self.taste.local_index_unavailable = unavailable;
        }
        if !self.taste_applied()
            || self.page != Page::Queue
            || self
                .taste
                .requested
                .is_none_or(|requested| requested.elapsed() < Duration::from_millis(180))
        {
            return;
        }
        self.taste.requested = None;
        let recent = self.library.taste.feedback.iter().rev();
        let mut feedback: Vec<_> = recent
            .clone()
            .filter(|item| item.verdict == Verdict::Keep)
            .take(FEEDBACK_LIMIT / 2)
            .chain(
                recent
                    .filter(|item| item.negative_weight() > 0.0)
                    .take(FEEDBACK_LIMIT / 2),
            )
            .cloned()
            .collect();
        feedback.reverse();
        if !feedback.iter().any(|item| item.verdict == Verdict::Keep) {
            return;
        }
        let (indices, candidates): (Vec<_>, Vec<_>) = self
            .library
            .queue
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.verdict == Verdict::Pending)
            .take(RANK_LIMIT)
            .map(|(index, entry)| {
                (
                    index,
                    Item {
                        record: entry.record.clone(),
                        range: entry.match_range,
                    },
                )
            })
            .unzip();
        if candidates.is_empty() {
            return;
        }
        let revision = self.taste.revision;
        let adventure = self.library.taste.adventure;
        let result = self.taste.jobs.submit(
            &self.ctx,
            "Ranking queue",
            Priority::Interactive,
            move || {
                let items: Vec<_> = candidates
                    .iter()
                    .cloned()
                    .chain(feedback.iter().map(|item| Item {
                        record: item.record.clone(),
                        range: item.range,
                    }))
                    .collect();
                let index_path = files::app_data_dir().map(|path| path.join("sample-index"));
                let vectors = index_path
                    .filter(|path| path.join("samples.sqlite3").is_file())
                    .map(|path| Index::open(&path)?.taste_vectors(&items))
                    .transpose();
                crate::background::check()?;
                let unavailable = vectors.is_err();
                let ranked = recommendations::rank(
                    &candidates,
                    &feedback,
                    &vectors.ok().flatten().unwrap_or_default(),
                    adventure,
                )
                .into_iter()
                .map(|row| (indices[row.candidate], row))
                .collect();
                Ok((revision, ranked, unavailable))
            },
        );
        if result.is_err() {
            self.taste.local_index_unavailable = true;
        }
    }

    pub(super) fn taste_waiting(&self) -> bool {
        self.taste.jobs.current().is_some()
            || (self.page == Page::Queue && self.taste_applied() && self.taste.requested.is_some())
    }

    pub(super) fn poll_taste_listening(&mut self) {
        let heard = self
            .output
            .as_ref()
            .filter(|_| self.library.taste.enabled)
            .and_then(|output| {
                if output.meters.listening()
                    && let Some(source) = &self.queue_playing
                {
                    return Some((
                        source,
                        false,
                        output.meters.audition_frames(false),
                        output.sample_rate,
                    ));
                }
                if self.audition.gain > 0.0
                    && output.meters.beat_audition() == 2
                    && let Some(source) = self
                        .audition
                        .preview
                        .as_ref()
                        .and_then(|preview| preview.queue_origin.as_ref())
                {
                    return Some((
                        source,
                        true,
                        output.meters.audition_frames(true),
                        output.sample_rate,
                    ));
                }
                None
            });
        let Some((source, beat, frames, rate)) = heard else {
            self.taste.listening = None;
            return;
        };
        if self
            .taste
            .listening
            .as_ref()
            .is_none_or(|(previous, context, _)| previous != source || *context != beat)
        {
            self.taste.listening = Some((source.clone(), beat, frames));
            return;
        }
        if self
            .taste
            .listening
            .as_ref()
            .is_some_and(|(_, _, start)| frames.saturating_sub(*start) >= 2 * u64::from(rate))
            && let Some(entry) = self
                .library
                .queue
                .iter_mut()
                .find(|entry| &entry.record.source == source && !entry.taste_heard)
        {
            entry.taste_heard = true;
            self.persist_library();
        }
    }
}
