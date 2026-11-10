use crate::{
    analysis::{AudioProfile, MatchBy, Sound},
    audio::Clip,
    background,
    files::{self, Result},
    library_index::{Index, Kind, Stamp},
    model::{Record, Source},
    search_worker::{DIMENSIONS, MODEL, Worker},
};
use rusqlite::params;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

#[derive(Clone)]
pub enum Query {
    Text(String),
    Cut(Arc<Clip>, (f64, f64), MatchBy),
}

#[derive(Clone, Default)]
pub struct Filters {
    pub source: Option<Kind>,
    pub min_seconds: f64,
    pub max_seconds: f64,
    pub min_bpm: u16,
    pub max_bpm: u16,
    pub instrument: String,
}

#[derive(Clone)]
pub struct Location {
    pub path: PathBuf,
    pub title: String,
    pub origin: Option<Record>,
    pub timeline: f64,
    pub stamp: Stamp,
    pub kind: Kind,
    pub bpm: Option<u16>,
    pub instrument: String,
}

impl Location {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            path: PathBuf::from(row.get::<_, String>(0)?),
            title: row.get(1)?,
            origin: row
                .get::<_, Option<String>>(2)?
                .and_then(|json| serde_json::from_str(&json).ok()),
            timeline: row.get(3)?,
            stamp: Stamp {
                bytes: row.get(4)?,
                modified: row.get(5)?,
            },
            kind: Kind::ALL[row.get::<_, usize>(6)?.min(2)],
            bpm: row.get(7)?,
            instrument: row.get(8)?,
        })
    }

    fn current(&self) -> bool {
        Stamp::read(&self.path).ok().as_ref() == Some(&self.stamp)
    }

    pub fn matches_source(&self, source: &Source) -> bool {
        self.origin
            .as_ref()
            .is_some_and(|record| &record.source == source)
            || (self.kind == Kind::Folder
                && matches!(source, Source::File(path)
                if std::fs::canonicalize(path).is_ok_and(|canonical| canonical == self.path)))
    }
}

#[derive(Clone)]
pub struct Hit {
    pub location: Location,
    pub range: (f64, f64),
    pub score: Option<f32>,
    content: String,
}

impl Hit {
    pub fn load(&self) -> Result<Clip> {
        let validate = || -> Result<()> {
            if !self.location.current() {
                return Err("This recording changed or is missing. Rescan the index.".into());
            }
            Ok(())
        };
        validate()?;
        let clip = files::decode_path(&self.location.path)?;
        validate()?;
        if clip.region(self.range).is_none() {
            return Err("The passage is no longer available.".into());
        }
        Ok(clip)
    }
}

pub fn search_files(index: &Index, text: &str, filters: &Filters, limit: usize) -> Result<Vec<Hit>> {
    background::stage("Searching filenames and tags")?;
    let text = text.to_lowercase();
    let mut query = index.db.prepare(
        "SELECT l.path,l.title,l.origin,l.timeline,l.bytes,l.modified,r.kind,
                coalesce(l.bpm,b.bpm),coalesce(l.tag,r.tag)
         FROM locations l JOIN roots r ON r.key=l.root
         LEFT JOIN builds b ON b.content=l.content AND b.model=?1
         WHERE (?2 IS NULL OR r.kind=?2)
             AND (?3=0 OR coalesce(l.bpm,b.bpm)>=?3)
             AND (?4=0 OR coalesce(l.bpm,b.bpm)<=?4)
         ORDER BY l.title COLLATE NOCASE,l.path,r.kind DESC,l.id",
    )?;
    let candidates = query.query_map(
        params![MODEL, filters.source.map(|kind| kind as i64), filters.min_bpm, filters.max_bpm],
        Location::read,
    )?;
    let instrument = filters.instrument.trim().to_lowercase();
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    for location in candidates {
        if hits.len() >= limit {
            break;
        }
        background::check()?;
        let location = location?;
        let tag = location.instrument.to_lowercase();
        let title = location.title.to_lowercase();
        let filename = location.path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        if !tag.contains(&instrument)
            || !text.split_whitespace().all(|word| {
                title.contains(word) || filename.contains(word) || tag.contains(word)
            })
            || !location.current()
        {
            continue;
        }
        let Ok(duration) = files::audio_duration(&location.path) else {
            continue;
        };
        if duration < filters.min_seconds
            || (filters.max_seconds > 0.0 && duration > filters.max_seconds)
            || !location.current()
            || !seen.insert((location.path.clone(), location.timeline.to_bits()))
        {
            continue;
        }
        hits.push(Hit {
            location,
            range: (0.0, duration),
            score: None,
            content: String::new(),
        });
    }
    background::check()?;
    Ok(hits)
}

pub struct Embedding {
    vector: Vec<f32>,
    reference: Option<(Sound, MatchBy)>,
    exclude: Option<String>,
    exclude_source: Option<(String, (f64, f64))>,
}

impl Query {
    pub fn embed(&self, worker: &mut Worker) -> Result<Embedding> {
        match self {
            Self::Text(text) => Ok(Embedding {
                vector: worker.text(text)?,
                reference: None,
                exclude: None,
                exclude_source: None,
            }),
            Self::Cut(clip, range, priority) => {
                let frames = clip.region(*range).ok_or("The reference cut is empty.")?;
                let exclude = crate::analysis_worker::fingerprint_frames(clip.rate, frames)?;
                let middle = (range.0 + range.1) / 2.0;
                let sample = ((middle - 4.0).max(range.0), (middle + 4.0).min(range.1));
                let vector = worker.audio(clip, &[sample])?.pop().unwrap();
                let reference = AudioProfile::new(clip, sample)
                    .sound()
                    .map(|sound| (sound, *priority));
                Ok(Embedding {
                    vector,
                    reference,
                    exclude: Some(exclude),
                    exclude_source: Some((crate::analysis_worker::fingerprint(clip)?, *range)),
                })
            }
        }
    }
}

struct Candidate {
    content: String,
    digest: String,
    range: (f64, f64),
    score: f32,
}

pub fn rank(
    index: &Index,
    embedding: &Embedding,
    filters: &Filters,
    limit: usize,
) -> Result<Vec<Hit>> {
    background::stage("Scoring local passages")?;
    let mut query = index.db.prepare(
        "SELECT content,start,end,vector,features,digest FROM passages
         WHERE model=?1 AND end-start>=?2 AND (?3=0 OR end-start<=?3)",
    )?;
    let mut rows = query.query(params![MODEL, filters.min_seconds, filters.max_seconds])?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next()? {
        if candidates.len().is_multiple_of(128) {
            background::check()?;
        }
        let digest: String = row.get(5)?;
        let content: String = row.get(0)?;
        let range = (row.get::<_, f64>(1)?, row.get::<_, f64>(2)?);
        if embedding
            .exclude
            .as_ref()
            .is_some_and(|hash| hash == &digest || hash == content.trim_end_matches(":cut"))
            || embedding
                .exclude_source
                .as_ref()
                .is_some_and(|(hash, cut)| {
                    hash == content.trim_end_matches(":cut") && range.0 < cut.1 && range.1 > cut.0
                })
        {
            continue;
        }
        let vector = row.get_ref(3)?.as_blob()?;
        if vector.len() != DIMENSIONS * 4 {
            continue;
        }
        let cosine = vector
            .as_chunks::<4>()
            .0
            .iter()
            .zip(&embedding.vector)
            .map(|(&bytes, &target)| f32::from_le_bytes(bytes) * target)
            .sum::<f32>();
        if !cosine.is_finite() {
            continue;
        }
        let score = if let Some((reference, priority)) = embedding.reference {
            let features: Option<String> = row.get(4)?;
            let sound = features.and_then(|json| serde_json::from_str::<Sound>(&json).ok());
            let similarity = sound.map_or(0.0, |sound| {
                (-2.5 * sound.distance(reference, priority).0).exp() as f32
            });
            cosine * 0.78 + similarity * 0.22
        } else {
            cosine
        };
        candidates.push(Candidate {
            content,
            digest,
            range,
            score,
        });
    }
    candidates.sort_unstable_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.content.cmp(&b.content))
            .then_with(|| a.range.0.total_cmp(&b.range.0))
    });
    let mut lookup = index.db.prepare_cached(
        "SELECT l.path,l.title,l.origin,l.timeline,l.bytes,l.modified,r.kind,
                coalesce(l.bpm,b.bpm),coalesce(l.tag,r.tag)
         FROM locations l JOIN roots r ON r.key=l.root JOIN builds b ON b.content=l.content AND b.model=?1
         WHERE l.content=?2 AND l.error IS NULL AND (?3 IS NULL OR r.kind=?3)
             AND (?4=0 OR coalesce(l.bpm,b.bpm)>=?4)
             AND (?5=0 OR coalesce(l.bpm,b.bpm)<=?5)
             AND (?6='' OR instr(lower(coalesce(l.tag,r.tag)),lower(?6))>0)
         ORDER BY r.kind DESC,l.id",
    )?;
    let mut locations = HashMap::new();
    let mut seen = HashSet::new();
    let mut hits: Vec<Hit> = Vec::new();
    for candidate in &candidates {
        background::check()?;
        if hits.len() == limit {
            if hits
                .last()
                .and_then(|hit| hit.score)
                .is_none_or(|score| candidate.score < score - 0.055)
            {
                break;
            }
            if !hits
                .iter()
                .any(|hit| hit.content == candidate.content.trim_end_matches(":cut"))
            {
                continue;
            }
        }
        if seen.contains(candidate.digest.as_str()) {
            continue;
        }
        let location = match locations.entry(candidate.content.as_str()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut rows = lookup.query(params![
                    MODEL,
                    candidate.content,
                    filters.source.map(|kind| kind as i64),
                    filters.min_bpm,
                    filters.max_bpm,
                    filters.instrument.trim()
                ])?;
                let mut found = None;
                while let Some(row) = rows.next()? {
                    let location = Location::read(row)?;
                    if location.current() {
                        found = Some(location);
                        break;
                    }
                }
                entry.insert(found)
            }
        };
        let Some(location) = location else { continue };
        let content = candidate.content.trim_end_matches(":cut");
        let related = hits.iter_mut().filter(|hit| hit.content == content);
        let mut count = 0;
        let mut overlaps = false;
        for hit in related {
            count += 1;
            let overlap = hit.range.1.min(candidate.range.1) - hit.range.0.max(candidate.range.0);
            if overlap <= 0.0 {
                continue;
            }
            overlaps = true;
            let union = (
                hit.range.0.min(candidate.range.0),
                hit.range.1.max(candidate.range.1),
            );
            let cap = if filters.max_seconds > 0.0 {
                filters.max_seconds.min(12.0)
            } else {
                12.0
            };
            if hit.location.path == location.path
                && union.1 - union.0 <= cap
                && hit.score.is_some_and(|score| candidate.score >= score - 0.055)
            {
                hit.range = union;
            }
            break;
        }
        if overlaps {
            seen.insert(candidate.digest.as_str());
        } else if count < 3 && hits.len() < limit {
            seen.insert(candidate.digest.as_str());
            hits.push(Hit {
                location: location.clone(),
                range: candidate.range,
                score: Some(candidate.score),
                content: content.to_owned(),
            });
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::{
        collections::BTreeMap,
        path::Path,
        time::{Duration, Instant},
    };

    #[derive(Deserialize)]
    struct LabeledQuery {
        text: String,
        label: String,
        suite: String,
    }

    fn label(hit: &Hit) -> &str {
        hit.location
            .path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    }

    #[test]
    fn file_search_filters_unembedded_audio_and_rejects_stale_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let samples = temp.path().join("samples");
        std::fs::create_dir(&samples)?;
        let snare = samples.join("Bright Snare.wav");
        let kick = samples.join("Kick.wav");
        for (path, frames) in [(&snare, 8_000), (&kick, 16_000)] {
            let mut writer = hound::WavWriter::create(
                path,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 8_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )?;
            for _ in 0..frames {
                writer.write_sample(0_i16)?;
            }
            writer.finalize()?;
        }
        let mut index = Index::open(&temp.path().join("index"))?;
        index.add_folder(&samples, "DRUMS")?;
        index.scan()?;
        assert_eq!(index.summary()?.passages, 0);
        assert_eq!(search_files(&index, "", &Filters::default(), 40)?.len(), 2);

        let filters = Filters {
            source: Some(Kind::Folder),
            max_seconds: 1.5,
            ..Filters::default()
        };
        let hits = search_files(&index, "drums SNARE", &filters, 40)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].location.title, "Bright Snare");
        assert_eq!(hits[0].range, (0.0, 1.0));
        assert_eq!(hits[0].score, None);
        assert_eq!(hits[0].load()?.frames.len(), 8_000);
        assert!(search_files(&index, "drums kick", &filters, 40)?.is_empty());

        std::fs::write(&snare, [0; 10])?;
        assert!(hits[0].load().is_err());
        assert!(search_files(&index, "snare", &filters, 40)?.is_empty());
        Ok(())
    }

    #[test]
    fn merges_neighbors_and_validates_duplicate_locations_and_model_version() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let first = temp.path().join("primary");
        let second = temp.path().join("duplicate");
        std::fs::create_dir(&first)?;
        std::fs::create_dir(&second)?;
        let primary = first.join("break.wav");
        let duplicate = second.join("backup.wav");
        std::fs::write(&primary, [0; 24])?;
        std::fs::copy(&primary, &duplicate)?;
        let mut index = Index::open(&temp.path().join("index"))?;
        index.add_folder(&first, "drums")?;
        index.add_folder(&second, "drums")?;
        index.scan()?;
        index
            .db
            .execute("UPDATE locations SET content='shared'", [])?;
        let mut vector = vec![0.0_f32; DIMENSIONS];
        vector[0] = 1.0;
        let blob: Vec<_> = vector.iter().copied().flat_map(f32::to_le_bytes).collect();
        for version in [MODEL, "incompatible"] {
            index.db.execute(
                "INSERT INTO builds(content,model,done,bpm) VALUES('shared',?1,1,104)",
                [version],
            )?;
            for (id, start) in [0.0_f64, 4.0].into_iter().enumerate() {
                index.db.execute(
                    "INSERT INTO passages(content,model,ordinal,start,end,vector,digest)
                     VALUES('shared',?1,?2,?3,?4,?5,?6)",
                    params![version, id, start, start + 8.0, blob, format!("{id}")],
                )?;
            }
        }
        index.db.execute(
            "INSERT INTO passages(content,model,ordinal,start,end,vector,digest)
                          VALUES('shared','incompatible',2,20.0,28.0,?1,'incompatible-only')",
            [&blob],
        )?;
        let query = Embedding {
            vector,
            reference: None,
            exclude: None,
            exclude_source: None,
        };
        let mut filter = Filters {
            min_bpm: 100,
            max_bpm: 105,
            instrument: "Drum".into(),
            ..Default::default()
        };
        let merged = rank(&index, &query, &filter, 5)?;
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].range, (0.0, 12.0));
        std::fs::remove_file(&merged[0].location.path)?;
        let available = rank(&index, &query, &filter, 5)?;
        assert_eq!(available.len(), 1);
        assert_ne!(available[0].location.path, merged[0].location.path);
        filter.max_bpm = 103;
        assert!(rank(&index, &query, &filter, 5)?.is_empty());
        std::fs::write(&available[0].location.path, [0; 25])?;
        assert!(rank(&index, &query, &Filters::default(), 5)?.is_empty());
        index.scan()?;
        let summary = index.summary()?;
        assert_eq!((summary.passages, summary.pending), (0, 1));
        Ok(())
    }

    #[test]
    #[ignore = "requires pinned local CLAP weights and the prepared labeled audio corpus"]
    fn clap_retrieval_resume_and_warm_latency() -> Result<()> {
        let corpus = PathBuf::from(
            std::env::var_os("LOST_SAMPLES_SEARCH_CORPUS")
                .ok_or("Set LOST_SAMPLES_SEARCH_CORPUS.")?,
        );
        let python = PathBuf::from(
            std::env::var_os("LOST_SAMPLES_SEARCH_PYTHON").unwrap_or_else(|| "python".into()),
        );
        let labels: Vec<LabeledQuery> =
            serde_json::from_slice(&std::fs::read(corpus.join("queries.json"))?)?;
        assert!(labels.len() >= 50);
        let temp = tempfile::tempdir()?;
        let directory = temp.path().to_owned();
        let mut index = Index::open(&directory)?;
        index.add_folder(&corpus, "")?;
        index.scan()?;

        let control = background::Control::default();
        let pause = control.clone();
        let worker_dir = directory.clone();
        let worker_python = python.clone();
        let build = std::thread::spawn(move || {
            let mut index = Index::open(&worker_dir).unwrap();
            let mut worker = None;
            let result = control
                .run(|| index.index(&mut worker, &worker_python))
                .map_err(|error| error.to_string());
            (worker, result)
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        while index.summary()?.passages < 12 && !build.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        pause.cancel();
        let (mut worker, result) = build.join().unwrap();
        let partial = index.summary()?;
        assert!(
            partial.passages >= 12 && partial.pending > 0,
            "pause failed: {result:?}"
        );
        assert!(result.is_err());
        drop(index);
        let mut index = Index::open(&directory)?;
        index.scan()?;
        index.index(&mut worker, &python)?;
        let summary = index.summary()?;
        assert_eq!(summary.pending + summary.failed, 0);
        println!(
            "Resumed {} committed passages and finished with {}.",
            partial.passages, summary.passages
        );

        let mut suites: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for case in &labels {
            let query = Query::Text(case.text.clone());
            let embedding = Worker::with(&mut worker, &python, |worker| query.embed(worker))?;
            let hits = rank(&index, &embedding, &Filters::default(), 5)?;
            let matching = hits.iter().any(|hit| label(hit) == case.label);
            let count = suites.entry(&case.suite).or_default();
            count.0 += usize::from(matching);
            count.1 += 1;
            if !matching {
                println!(
                    "MISS [{}] {:?} expected {} got {:?}",
                    case.suite,
                    case.text,
                    case.label,
                    hits.iter().map(label).collect::<Vec<_>>()
                );
            }
        }
        println!("Top five by suite {suites:?}");
        let successes: usize = suites.values().map(|counts| counts.0).sum();

        let need = crate::library_index::MAX_PASSAGES - summary.passages;
        index.db.execute(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100)
             INSERT INTO passages(content,model,ordinal,start,end,vector,features,digest)
             SELECT p.content,p.model,n.x,p.start+n.x*10,p.end+n.x*10,p.vector,p.features,p.digest
             FROM passages p CROSS JOIN n WHERE p.ordinal=0 AND p.model=?1 LIMIT ?2",
            params![MODEL, need],
        )?;
        assert_eq!(
            index.summary()?.passages,
            crate::library_index::MAX_PASSAGES
        );
        let mut elapsed = Vec::new();
        for case in &labels {
            let query = Query::Text(case.text.clone());
            let started = Instant::now();
            let embedding = Worker::with(&mut worker, &python, |worker| query.embed(worker))?;
            assert!(!rank(&index, &embedding, &Filters::default(), 40)?.is_empty());
            elapsed.push(started.elapsed());
        }
        elapsed.sort_unstable();
        let p95 = elapsed[(elapsed.len() * 95).div_ceil(100) - 1];
        println!(
            "Warm p95 with 10,000 passages including CLAP text embedding: {:.1} ms",
            p95.as_secs_f64() * 1000.0
        );
        assert!(p95 < Duration::from_secs(1));
        assert!(
            successes * 5 >= labels.len() * 4,
            "{successes}/{} in top five",
            labels.len()
        );
        Ok(())
    }
}
