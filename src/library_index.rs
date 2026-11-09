mod taste;

use crate::{
    analysis::AudioProfile,
    audio::Clip,
    background,
    files::{self, Result},
    model::Record,
    search_worker::{BATCH, MODEL, Worker},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

pub const MAX_PASSAGES: usize = 10_000;
const MAX_FILES: usize = 10_000;
const MAX_CUT_BYTES: u64 = 128 * 1024 * 1024;
const DOWNLOADS: &str = "downloads";
const CUTS: &str = "cuts";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i64)]
pub enum Kind {
    #[default]
    Folder = 0,
    Download = 1,
    Cut = 2,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Folder, Self::Download, Self::Cut];

    pub fn label(self) -> &'static str {
        match self {
            Self::Folder => "Folder",
            Self::Download => "Downloaded",
            Self::Cut => "Saved cut",
        }
    }
}

#[derive(Clone)]
pub struct Root {
    pub key: String,
    pub name: String,
}

#[derive(Default)]
pub struct Summary {
    pub roots: Vec<Root>,
    pub passages: usize,
    pub pending: usize,
    pub failed: usize,
    pub errors: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Cut {
    pub name: String,
    pub source: Option<Record>,
    pub clip: Arc<Clip>,
    pub range: (f64, f64),
    pub instrument: String,
    pub bpm: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub bytes: u64,
    pub modified: String,
}

impl Stamp {
    pub fn read(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() {
            return Err("Audio path is no longer a file.".into());
        }
        let modified = metadata.modified()?;
        let modified = match modified.duration_since(UNIX_EPOCH) {
            Ok(time) => time.as_nanos().to_string(),
            Err(error) => format!("-{}", error.duration().as_nanos()),
        };
        Ok(Self {
            bytes: metadata.len(),
            modified,
        })
    }
}

pub struct Index {
    pub(crate) db: Connection,
    directory: PathBuf,
}

impl Index {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let db = Connection::open(directory.join("samples.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(3))?;
        db.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             PRAGMA max_page_count=40960;
             PRAGMA journal_size_limit=4194304;
             CREATE TABLE IF NOT EXISTS roots (
                 key TEXT PRIMARY KEY, path TEXT NOT NULL, kind INTEGER NOT NULL, tag TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS locations (
                 id INTEGER PRIMARY KEY, root TEXT NOT NULL REFERENCES roots(key) ON DELETE CASCADE,
                 key TEXT NOT NULL, path TEXT NOT NULL, title TEXT NOT NULL, origin TEXT, timeline REAL NOT NULL DEFAULT 0,
                 bytes INTEGER NOT NULL, modified TEXT NOT NULL, content TEXT, error TEXT, bpm INTEGER, tag TEXT,
                 UNIQUE(root, key)
             );
             CREATE INDEX IF NOT EXISTS locations_content ON locations(content);
             CREATE TABLE IF NOT EXISTS builds (
                 content TEXT NOT NULL, model TEXT NOT NULL, next INTEGER NOT NULL DEFAULT 0,
                 done INTEGER NOT NULL DEFAULT 0, bpm INTEGER, PRIMARY KEY (content, model)
             );
             CREATE TABLE IF NOT EXISTS passages (
                 content TEXT NOT NULL, model TEXT NOT NULL, ordinal INTEGER NOT NULL, start REAL NOT NULL,
                 end REAL NOT NULL, vector BLOB NOT NULL, features TEXT, digest TEXT NOT NULL,
                 PRIMARY KEY(content, model, ordinal),
                 FOREIGN KEY(content, model) REFERENCES builds(content, model) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS passages_model ON passages(model);",
        )?;
        Ok(Self {
            db,
            directory: directory.to_owned(),
        })
    }

    pub fn summary(&self) -> Result<Summary> {
        let mut query = self
            .db
            .prepare("SELECT key,path,kind FROM roots ORDER BY kind,path")?;
        let roots = query
            .query_map([], |row| {
                let kind = row.get::<_, i64>(2)?;
                Ok(Root {
                    key: row.get(0)?,
                    name: match kind {
                        1 => "Cached downloads".into(),
                        2 => "Indexed saved cuts".into(),
                        _ => row.get(1)?,
                    },
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let passages = self.db.query_row(
            "SELECT count(*) FROM passages WHERE model=?1",
            [MODEL],
            |r| r.get(0),
        )?;
        let pending = self.db.query_row(
            "SELECT count(*) FROM locations l LEFT JOIN builds b ON b.content=l.content AND b.model=?1
             WHERE l.error IS NULL AND coalesce(b.done,0)=0", [MODEL], |r| r.get(0),
        )?;
        let failed = self.db.query_row(
            "SELECT count(*) FROM locations WHERE error IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        let errors = self
            .db
            .prepare(
                "SELECT title,error FROM locations WHERE error IS NOT NULL ORDER BY id LIMIT 8",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Summary {
            roots,
            passages,
            pending,
            failed,
            errors,
        })
    }

    pub fn add_folder(&self, path: &Path, tag: &str) -> Result<()> {
        let path = fs::canonicalize(path)?;
        if !path.is_dir() {
            return Err("Choose an audio folder.".into());
        }
        let path = path
            .to_str()
            .ok_or("The folder path cannot be represented in the index.")?;
        self.add_root(path, path, Kind::Folder, tag)
    }

    pub fn add_downloads(&self) -> Result<()> {
        let path = files::app_data_dir()
            .ok_or("No application data folder.")?
            .join("youtube");
        self.add_root(DOWNLOADS, &path.to_string_lossy(), Kind::Download, "")
    }

    fn add_root(&self, key: &str, path: &str, kind: Kind, tag: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO roots(key,path,kind,tag) VALUES(?1,?2,?3,?4)
                         ON CONFLICT(key) DO UPDATE SET tag=excluded.tag",
            params![key, path, kind as i64, tag],
        )?;
        Ok(())
    }

    pub fn remove(&self, root: &str) -> Result<()> {
        self.db.execute("DELETE FROM roots WHERE key=?1", [root])?;
        self.collect_unused()?;
        Ok(())
    }

    pub fn rebuild(&self) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute("DELETE FROM builds WHERE model=?1", [MODEL])?;
        tx.execute("UPDATE locations SET error=NULL", [])?;
        tx.commit()?;
        Ok(())
    }

    fn collect_unused(&self) -> Result<()> {
        self.db.execute(
            "DELETE FROM builds WHERE content NOT IN
                         (SELECT content FROM locations WHERE content IS NOT NULL)",
            [],
        )?;
        let cuts = self.directory.join(CUTS);
        if let Ok(actual) = fs::canonicalize(&cuts)
            && actual == fs::canonicalize(&self.directory)?.join(CUTS)
            && let Ok(entries) = fs::read_dir(cuts)
        {
            let mut lookup = self
                .db
                .prepare("SELECT 1 FROM locations WHERE root='cuts' AND path=?1")?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "wav")
                    && path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                    && !lookup.exists([path.to_string_lossy().as_ref()])?
                {
                    fs::remove_file(path)?;
                }
            }
        }
        Ok(())
    }

    pub fn add_cut(&self, cut: &Cut) -> Result<()> {
        let frames = cut
            .clip
            .region(cut.range)
            .ok_or("The saved cut is empty.")?;
        let digest = crate::analysis_worker::fingerprint_frames(cut.clip.rate, frames)?;
        let directory = self.directory.join(CUTS);
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{digest}.wav"));
        let registered: usize = self
            .db
            .query_row("SELECT count(*) FROM locations", [], |row| row.get(0))?;
        if registered >= MAX_FILES {
            return Err("The index has reached its file limit.".into());
        }
        if !path.is_file() {
            let used: u64 = fs::read_dir(&directory)?
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .map(|metadata| metadata.len())
                .sum();
            if used + frames.len() as u64 * 8 + 128 > MAX_CUT_BYTES {
                return Err(
                    "The saved-cut audio cache is full. Remove indexed cuts to free space.".into(),
                );
            }
            files::write_atomically(&path, |file| {
                let mut writer = hound::WavWriter::new(
                    file,
                    hound::WavSpec {
                        channels: 2,
                        sample_rate: cut.clip.rate,
                        bits_per_sample: 32,
                        sample_format: hound::SampleFormat::Float,
                    },
                )?;
                for block in frames.chunks(4096) {
                    background::check()?;
                    for sample in block.iter().flatten() {
                        writer.write_sample(*sample)?;
                    }
                }
                writer.finalize()?;
                Ok(())
            })?;
        }
        self.add_root(CUTS, "", Kind::Cut, "")?;
        let origin = cut.source.as_ref().map(serde_json::to_string).transpose()?;
        let stamp = Stamp::read(&path)?;
        let key = serde_json::to_string(&(&digest, &cut.name, &origin, cut.range))?;
        self.db.execute(
            "INSERT INTO locations(root,key,path,title,origin,timeline,bytes,modified,content,bpm,tag)
             VALUES('cuts',?10,?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(root,key) DO UPDATE SET
             title=excluded.title,origin=excluded.origin,timeline=excluded.timeline,bpm=excluded.bpm,tag=excluded.tag",
            params![path.to_string_lossy(), cut.name, origin, cut.range.0, stamp.bytes, stamp.modified,
                format!("{digest}:cut"), cut.bpm, cut.instrument, key],
        )?;
        Ok(())
    }

    pub fn scan(&mut self) -> Result<()> {
        background::stage("Scanning local audio folders")?;
        let roots = {
            let mut query = self
                .db
                .prepare("SELECT key,path,kind FROM roots WHERE kind != 2 ORDER BY key")?;
            query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (key, folder, kind) in roots {
            background::check()?;
            let paths = paths(Path::new(&folder), kind == Kind::Download as i64)?;
            let tx = self.db.transaction()?;
            let mut existing = {
                let mut query = tx.prepare("SELECT path FROM locations WHERE root=?1")?;
                query
                    .query_map([&key], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<HashSet<_>, _>>()?
            };
            let registered: usize =
                tx.query_row("SELECT count(*) FROM locations", [], |r| r.get(0))?;
            let mut available = MAX_FILES.saturating_sub(registered - existing.len());
            for path in paths {
                background::check()?;
                if available == 0 {
                    break;
                }
                let Some(path_text) = path.to_str() else {
                    continue;
                };
                let stamp = Stamp::read(&path)?;
                let record = if kind == Kind::Download as i64 {
                    crate::youtube::cached_record(&path)
                } else {
                    None
                };
                let title = record.as_ref().map_or_else(
                    || {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    },
                    |record| record.title.clone(),
                );
                let origin = record.as_ref().map(serde_json::to_string).transpose()?;
                tx.execute(
                    "INSERT INTO locations(root,key,path,title,origin,bytes,modified) VALUES(?1,?2,?2,?3,?4,?5,?6)
                     ON CONFLICT(root,key) DO UPDATE SET title=excluded.title,origin=excluded.origin,
                     content=CASE WHEN locations.modified=excluded.modified AND locations.bytes=excluded.bytes THEN locations.content END,
                     error=CASE WHEN locations.modified=excluded.modified AND locations.bytes=excluded.bytes THEN locations.error END,
                     bytes=excluded.bytes,modified=excluded.modified",
                    params![key, path_text, title, origin, stamp.bytes, stamp.modified],
                )?;
                existing.remove(path_text);
                available -= 1;
            }
            for stale in existing {
                tx.execute(
                    "DELETE FROM locations WHERE root=?1 AND path=?2",
                    params![key, stale],
                )?;
            }
            tx.commit()?;
        }
        let cuts = {
            let mut query = self
                .db
                .prepare("SELECT id,path,bytes,modified FROM locations WHERE root='cuts'")?;
            query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        PathBuf::from(r.get::<_, String>(1)?),
                        Stamp {
                            bytes: r.get(2)?,
                            modified: r.get(3)?,
                        },
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (id, path, stamp) in cuts {
            background::check()?;
            if Stamp::read(&path).ok().as_ref() != Some(&stamp) {
                self.db.execute("DELETE FROM locations WHERE id=?1", [id])?;
            }
        }
        self.collect_unused()
    }

    pub fn index(&mut self, worker: &mut Option<Worker>, python: &Path) -> Result<()> {
        let files = {
            let mut query = self.db.prepare(
                "SELECT l.id,l.path,l.modified,l.bytes,r.kind FROM locations l JOIN roots r ON r.key=l.root
                 LEFT JOIN builds b ON b.content=l.content AND b.model=?1
                 WHERE l.error IS NULL AND coalesce(b.done,0)=0 ORDER BY l.id",
            )?;
            query
                .query_map([MODEL], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        PathBuf::from(r.get::<_, String>(1)?),
                        Stamp {
                            modified: r.get(2)?,
                            bytes: r.get(3)?,
                        },
                        r.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (done, (id, path, stamp, kind)) in files.iter().enumerate() {
            background::stage("Indexing local passages")?;
            background::progress(done, files.len())?;
            if self.passages()? >= MAX_PASSAGES {
                break;
            }
            let clip = match files::decode_path(path) {
                Ok(clip) => clip,
                Err(error) => {
                    background::check()?;
                    self.db.execute(
                        "UPDATE locations SET error=?1 WHERE id=?2",
                        params![error.to_string(), id],
                    )?;
                    continue;
                }
            };
            if Stamp::read(path).ok().as_ref() != Some(stamp) {
                continue;
            }
            let cut = *kind == Kind::Cut as i64;
            let hash = crate::analysis_worker::fingerprint(&clip)?;
            let content = if cut { format!("{hash}:cut") } else { hash };
            self.db.execute(
                "UPDATE locations SET content=?1 WHERE id=?2",
                params![content, id],
            )?;
            let build = self
                .db
                .query_row(
                    "SELECT next,done FROM builds WHERE content=?1 AND model=?2",
                    params![content, MODEL],
                    |r| Ok((r.get::<_, usize>(0)?, r.get::<_, bool>(1)?)),
                )
                .optional()?;
            if build.is_some_and(|(_, done)| done) {
                continue;
            }
            if build.is_none() {
                let bpm = (clip.duration() >= 6.0)
                    .then(|| crate::tempo::estimate(&clip, Some((0.0, clip.duration().min(24.0)))))
                    .flatten()
                    .filter(|estimate| estimate.strength >= 35)
                    .map(|estimate| estimate.bpm);
                self.db.execute(
                    "INSERT INTO builds(content,model,bpm) VALUES(?1,?2,?3)",
                    params![content, MODEL, bpm],
                )?;
            }
            let ranges = windows(clip.duration(), cut);
            let mut next = build.map_or(0, |(next, _)| next);
            while next < ranges.len() {
                background::check()?;
                let count = (ranges.len() - next)
                    .min(BATCH)
                    .min(MAX_PASSAGES - self.passages()?);
                if count == 0 {
                    return Ok(());
                }
                let batch = &ranges[next..next + count];
                let vectors = Worker::with(worker, python, |worker| worker.audio(&clip, batch))?;
                let tx = self.db.transaction()?;
                for (offset, (&range, vector)) in batch.iter().zip(vectors).enumerate() {
                    let frames = clip.region(range).ok_or("Passage is empty.")?;
                    let digest = crate::analysis_worker::fingerprint_frames(clip.rate, frames)?;
                    let features = AudioProfile::new(&clip, range)
                        .sound()
                        .map(|sound| serde_json::to_string(&sound))
                        .transpose()?;
                    background::check()?;
                    let blob: Vec<_> = vector.into_iter().flat_map(f32::to_le_bytes).collect();
                    tx.execute("INSERT INTO passages(content,model,ordinal,start,end,vector,features,digest)
                                VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![content, MODEL, next + offset, range.0, range.1, blob, features, digest])?;
                }
                next += count;
                tx.execute(
                    "UPDATE builds SET next=?1 WHERE content=?2 AND model=?3",
                    params![next, content, MODEL],
                )?;
                tx.commit()?;
            }
            self.db.execute(
                "UPDATE builds SET done=1 WHERE content=?1 AND model=?2",
                params![content, MODEL],
            )?;
        }
        Ok(())
    }

    fn passages(&self) -> Result<usize> {
        Ok(self.db.query_row(
            "SELECT count(*) FROM passages WHERE model=?1",
            [MODEL],
            |row| row.get(0),
        )?)
    }
}

fn paths(folder: &Path, downloads: bool) -> Result<Vec<PathBuf>> {
    if !folder.exists() {
        return Ok(Vec::new());
    }
    let mut directories = vec![folder.to_owned()];
    let mut paths = Vec::new();
    let mut visited = 0;
    while let Some(directory) = directories.pop() {
        background::check()?;
        visited += 1;
        if visited > MAX_FILES {
            return Err("The folder contains too many directories.".into());
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_dir() && !downloads {
                directories.push(path);
            } else if ty.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        if downloads {
                            ext.eq_ignore_ascii_case("m4a")
                        } else {
                            files::AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                        }
                    })
            {
                paths.push(path);
                if paths.len() > MAX_FILES {
                    return Err("Choose a folder with at most 10,000 audio files.".into());
                }
            }
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

pub fn windows(seconds: f64, cut: bool) -> Vec<(f64, f64)> {
    let (width, hop) = if cut { (2.0, 1.0) } else { (8.0, 4.0) };
    if seconds <= 0.0 {
        return Vec::new();
    }
    if seconds <= width {
        return vec![(0.0, seconds)];
    }
    let mut ranges = Vec::new();
    let mut start = 0.0;
    while start + width < seconds {
        ranges.push((start, start + width));
        start += hop;
    }
    ranges.push((seconds - width, seconds));
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_saved_cut_audio_with_each_provenance_and_removes_only_its_owned_copy() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("index");
        let source = dir.path().join("embedded.wav");
        fs::write(&source, b"original source")?;
        let clip = Arc::new(Clip::new(8_000, vec![[0.125; 2]; 8_000]));
        let mut cut = Cut {
            name: "Verse pickup".into(),
            source: Some(files::local_record(source.clone())),
            clip,
            range: (0.25, 0.5),
            instrument: "Keys".into(),
            bpm: Some(90),
        };
        let index = Index::open(&root)?;
        index.add_cut(&cut)?;
        cut.name = "Chorus pickup".into();
        cut.range = (0.5, 0.75);
        index.add_cut(&cut)?;
        let timestamps: Vec<f64> = index
            .db
            .prepare("SELECT timeline FROM locations ORDER BY timeline")?
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        assert_eq!(timestamps, [0.25, 0.5]);
        let entries = fs::read_dir(root.join(CUTS))?.collect::<std::io::Result<Vec<_>>>()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(files::decode_path(&entries[0].path())?.frames.len(), 2_000);
        index.remove(CUTS)?;
        assert!(fs::read_dir(root.join(CUTS))?.next().is_none());
        assert_eq!(fs::read(&source)?, b"original source");
        assert_eq!(windows(13.0, false), [(0.0, 8.0), (4.0, 12.0), (5.0, 13.0)]);
        assert_eq!(windows(3.0, true), [(0.0, 2.0), (1.0, 3.0)]);
        Ok(())
    }
}
