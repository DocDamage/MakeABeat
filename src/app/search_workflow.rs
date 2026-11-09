use super::{App, Job, Page};
use crate::{
    analysis_jobs::{Priority, Queue},
    audio::Clip,
    files::{self, Project},
    library_index::{Cut, Index, Summary},
    model::{MAX_NAMED_SAMPLES, MAX_SAMPLE_NAME, NamedSample, Record, Source},
    search::{self, Filters, Hit, Query},
    search_worker::Worker,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy)]
pub(crate) enum UseHit {
    Audition,
    Assign(usize),
    Save,
}

enum Completion {
    Updated(Summary, bool),
    Setup,
    Ranked(u64, String, Result<Vec<Hit>, String>),
    Audition(Hit, Clip, Option<LinkedSource>),
    Loaded(UseHit, Arc<Project>, Hit, Clip),
}

struct LinkedSource {
    index: usize,
    clip: Arc<Clip>,
    range: (f64, f64),
}

#[derive(Default)]
pub(crate) struct Search {
    pub text: String,
    pub tag: String,
    pub filters: Filters,
    pub summary: Summary,
    pub results: Vec<Hit>,
    pub results_for: Option<String>,
    pub status: Option<(String, bool)>,
    initialized: bool,
    revision: u64,
    jobs: Queue<Completion>,
    worker: Arc<Mutex<Option<Worker>>>,
}

impl App {
    fn submit_search(
        &mut self,
        label: &'static str,
        priority: Priority,
        work: impl Fn() -> files::Result<Completion> + Send + Sync + 'static,
    ) {
        if let Err(error) = self.search.jobs.submit(&self.ctx, label, priority, work) {
            self.search.status = Some((error.to_string(), true));
        }
    }

    pub fn open_search(&mut self) {
        self.page = Page::Search;
        if !self.search.initialized {
            self.search.initialized = true;
            self.refresh_search_summary();
        }
    }

    fn refresh_search_summary(&mut self) {
        self.submit_search("Reading sample index", Priority::Foreground, || {
            Ok(Completion::Updated(open_index()?.summary()?, false))
        });
    }

    pub fn search_job(&self) -> Option<(&'static str, crate::background::State, usize)> {
        self.search.jobs.current()
    }

    pub fn pause_sample_search(&mut self) {
        self.search.jobs.clear();
        self.search.status = Some((
            "Paused. Indexed passages are available for search.".into(),
            false,
        ));
        self.refresh_search_summary();
    }

    pub fn search_python(&self) -> PathBuf {
        self.library
            .search_python
            .clone()
            .unwrap_or_else(|| PathBuf::from("python"))
    }

    pub fn choose_search_python(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Choose the pinned CLAP Python executable")
            .pick_file()
        {
            self.library.search_python = Some(path);
            self.persist_library();
        }
    }

    pub fn setup_sample_search(&mut self) {
        let python = self.search_python();
        self.submit_search("Downloading CLAP model", Priority::Foreground, move || {
            Worker::setup(&python)?;
            Ok(Completion::Setup)
        });
    }

    pub fn index_search_folder(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Index a local sample folder")
            .pick_folder()
        else {
            return;
        };
        let tag = self.search.tag.trim().to_owned();
        self.change_index(
            "Adding sample folder",
            move |index| index.add_folder(&path, &tag),
            true,
        );
    }

    pub fn index_search_downloads(&mut self) {
        self.change_index(
            "Adding cached downloads",
            |index| index.add_downloads(),
            true,
        );
    }

    pub fn index_search_cuts(&mut self) {
        let cuts: Vec<_> = self
            .samples
            .iter()
            .filter_map(|sample| {
                let clip = self.source_arc(sample.source)?;
                let bpm = self
                    .studio
                    .beat_maps
                    .get(&sample.source)
                    .and_then(|map| crate::groove::loop_beats(map, sample.range(), 8))
                    .map(|beats| {
                        ((beats.len() - 1) as f64 * 60.0
                            / (beats.last().unwrap().seconds - beats[0].seconds))
                            .round() as u16
                    });
                Some(Cut {
                    name: sample.name.clone(),
                    source: self.source_record(sample.source).cloned(),
                    clip,
                    range: sample.range(),
                    instrument: self.search.tag.trim().to_owned(),
                    bpm,
                })
            })
            .collect();
        if cuts.is_empty() {
            return;
        }
        self.change_index(
            "Caching saved cuts",
            move |index| {
                for cut in &cuts {
                    index.add_cut(cut)?;
                }
                Ok(())
            },
            true,
        );
    }

    pub fn remove_search_root(&mut self, key: String) {
        self.search.results.clear();
        self.search.results_for = None;
        self.change_index(
            "Removing source from search",
            move |index| index.remove(&key),
            false,
        );
    }

    pub fn rebuild_sample_search(&mut self) {
        self.search.results.clear();
        self.search.results_for = None;
        self.change_index("Resetting passage analysis", |index| index.rebuild(), true);
    }

    fn change_index(
        &mut self,
        label: &'static str,
        action: impl Fn(&Index) -> files::Result<()> + Send + Sync + 'static,
        resume: bool,
    ) {
        self.search.status = None;
        self.submit_search(label, Priority::Foreground, move || {
            let index = open_index()?;
            action(&index)?;
            Ok(Completion::Updated(index.summary()?, resume))
        });
    }

    pub fn resume_sample_index(&mut self) {
        let python = self.search_python();
        let worker = self.search.worker.clone();
        self.search.status = None;
        self.submit_search("Scanning and indexing", Priority::Indexing, move || {
            let mut index = open_index()?;
            index.scan()?;
            index.index(
                &mut *worker.lock().map_err(|_| "CLAP worker is unavailable.")?,
                &python,
            )?;
            Ok(Completion::Updated(index.summary()?, false))
        });
    }

    pub fn search_sample_text(&mut self) {
        let text = self.search.text.trim().to_owned();
        if !text.is_empty() {
            self.find_samples(text.clone(), Query::Text(text));
        }
    }

    pub fn search_similar_cut(&mut self, index: usize) {
        let Some(cut) = self.samples.get(index) else {
            return;
        };
        let Some(clip) = self.source_arc(cut.source) else {
            return;
        };
        let name = cut.name.clone();
        let query = Query::Cut(clip, cut.range(), self.match_priority);
        self.open_search();
        self.find_samples(format!("Similar to {name}"), query);
    }

    fn find_samples(&mut self, description: String, query: Query) {
        let python = self.search_python();
        let worker = self.search.worker.clone();
        let filters = self.search.filters.clone();
        self.search.revision += 1;
        let revision = self.search.revision;
        self.search.status = None;
        self.submit_search(
            "Searching local samples",
            Priority::Interactive,
            move || {
                let run = || -> files::Result<Vec<Hit>> {
                    let index = open_index()?;
                    let embedding = Worker::with(
                        &mut *worker.lock().map_err(|_| "CLAP worker is unavailable.")?,
                        &python,
                        |worker| query.embed(worker),
                    )?;
                    search::rank(&index, &embedding, &filters, 40)
                };
                Ok(Completion::Ranked(
                    revision,
                    description.clone(),
                    run().map_err(|error| error.to_string()),
                ))
            },
        );
    }

    pub fn use_search_hit(&mut self, hit: Hit, action: UseHit) {
        if !matches!(action, UseHit::Audition) && (!self.can_add_source() || self.recording) {
            return;
        }
        if matches!(action, UseHit::Audition) {
            let project_sources: Vec<_> = (0..=self.studio.sources.len())
                .filter_map(|index| {
                    Some((
                        index,
                        self.source_record(index)?.source.clone(),
                        self.source_arc(index)?,
                    ))
                })
                .collect();
            self.submit_search(
                "Reading indexed passage",
                Priority::Interactive,
                move || {
                    let clip = hit.load()?;
                    let range = (
                        hit.range.0 + hit.location.timeline,
                        hit.range.1 + hit.location.timeline,
                    );
                    let linked = project_sources
                        .iter()
                        .find_map(|(index, source, original)| {
                            if !hit.location.matches_source(source)
                                || original.rate != clip.rate
                                || original.region(range) != clip.region(hit.range)
                            {
                                return None;
                            }
                            Some(LinkedSource {
                                index: *index,
                                clip: original.clone(),
                                range,
                            })
                        });
                    Ok(Completion::Audition(hit.clone(), clip, linked))
                },
            );
        } else {
            self.observe_edits(false);
            let base = self.session.current.clone();
            self.submit_search(
                "Reading indexed passage",
                Priority::Interactive,
                move || {
                    let clip = hit.load()?;
                    let clip = Clip::new(
                        clip.rate,
                        clip.region(hit.range)
                            .ok_or("The passage is empty.")?
                            .to_vec(),
                    );
                    Ok(Completion::Loaded(action, base.clone(), hit.clone(), clip))
                },
            );
        }
    }

    pub(super) fn poll_sample_search(&mut self) {
        let job = self.search_job().map(|(label, _, _)| label);
        let Some(result) = self.search.jobs.poll(&self.ctx) else {
            return;
        };
        match result {
            Ok(Completion::Updated(summary, resume)) => {
                self.search.summary = summary;
                self.invalidate_taste();
                if resume {
                    self.resume_sample_index();
                }
            }
            Ok(Completion::Setup) => {
                self.search.status = Some((
                    "The CLAP model is cached. Index a source or run a search.".into(),
                    false,
                ))
            }
            Ok(Completion::Ranked(revision, description, result))
                if revision == self.search.revision =>
            {
                match result {
                    Ok(hits) => {
                        self.search.results = hits;
                        self.search.results_for = Some(description);
                    }
                    Err(error) => self.search.status = Some((error, true)),
                }
            }
            Ok(Completion::Ranked(..)) => (),
            Ok(Completion::Audition(hit, clip, linked)) => {
                if let Some(linked) = linked.filter(|source| {
                    self.source_arc(source.index)
                        .is_some_and(|clip| Arc::ptr_eq(&source.clip.frames, &clip.frames))
                }) {
                    self.audition_with_beat(
                        hit.location.title,
                        linked.clip,
                        linked.range,
                        Some(linked.index),
                    );
                } else {
                    self.audition_with_beat(hit.location.title, Arc::new(clip), hit.range, None);
                }
            }
            Ok(Completion::Loaded(action, base, hit, clip)) => {
                self.attach_search_hit(action, &base, hit, clip)
            }
            Err(error) => {
                self.search.status = Some((error, true));
                if job == Some("Scanning and indexing") {
                    self.refresh_search_summary();
                }
            }
        }
    }

    fn attach_search_hit(&mut self, action: UseHit, base: &Arc<Project>, hit: Hit, clip: Clip) {
        if !Arc::ptr_eq(&self.session.current, base) || self.recording || !self.can_add_source() {
            self.search.status = Some((
                "The project changed while loading the passage. Choose the action again.".into(),
                true,
            ));
            return;
        }
        if self.project_frame_count() + clip.frames.len() > files::MAX_PROJECT_FRAMES {
            self.search.status = Some((
                "This passage exceeds the remaining project audio limit.".into(),
                true,
            ));
            return;
        }
        let name: String = hit.location.title.chars().take(MAX_SAMPLE_NAME).collect();
        let duration = clip.duration();
        let record = Record {
            title: name.clone(),
            artist: "Indexed passage".into(),
            genre: String::new(),
            source: Source::Generated(self.random()),
            release: hit.location.origin.and_then(|record| record.release),
        };
        match action {
            UseHit::Assign(pad) if self.studio.pads[pad].is_none() => {
                self.apply_job(Job::SourceAdded(record, clip, Some(pad)))
            }
            UseHit::Save if self.samples.len() < MAX_NAMED_SAMPLES => {
                self.apply_job(Job::SourceAdded(record, clip, None));
                self.samples.push(NamedSample {
                    source: self.active_source,
                    name,
                    start: 0.0,
                    end: duration,
                });
                self.selected_sample = Some(self.samples.len() - 1);
                self.persist_samples();
            }
            _ => return,
        }
        self.observe_edits(false);
    }

    pub(super) fn close_sample_search(&mut self) {
        self.search.jobs.clear();
    }
}

fn open_index() -> files::Result<Index> {
    let path = files::app_data_dir()
        .ok_or("No application data folder.")?
        .join("sample-index");
    Index::open(&path)
}
