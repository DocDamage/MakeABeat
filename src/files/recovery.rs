use super::{
    Clip, MAX_FRAMES, MAX_PROJECT_FRAMES, MAX_PROJECT_METADATA, Project, Result, open_project,
    read_project_clip, validate_loaded_project, validate_snapshot_sources, write_atomically,
    write_project_clip,
};
use crate::model::MAX_SOURCES;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

pub const SLOT_COUNT: usize = 2;
const SLOTS: [&str; SLOT_COUNT] = ["snapshot-0.recovery", "snapshot-1.recovery"];
const LEGACY_SLOTS: [&str; SLOT_COUNT] = ["snapshot-0.flip", "snapshot-1.flip"];
const VERSION: u8 = 1;
const MANIFEST_LIMIT: usize = MAX_PROJECT_METADATA + 4096;

#[derive(Serialize, Deserialize)]
struct Manifest<P> {
    version: u8,
    project: P,
    audio: Vec<u64>,
}

struct Blob {
    id: u64,
    rate: u32,
    identity: Weak<()>,
}

#[derive(Default)]
pub struct Writer {
    slots: [Vec<u64>; SLOT_COUNT],
    blobs: Vec<Blob>,
    last_id: u64,
}

impl Writer {
    pub fn save(
        &mut self,
        dir: &Path,
        slot: usize,
        project: &Project,
        clip: Option<&Clip>,
    ) -> Result<()> {
        let result = self.publish(dir, slot, project, clip);
        // Both published slots must stay readable, even when replacing one fails.
        self.blobs.retain(|blob| {
            self.slots.iter().any(|ids| ids.contains(&blob.id))
                || remove_file(&blob_path(dir, blob.id)).is_err()
        });
        result
    }

    fn publish(
        &mut self,
        dir: &Path,
        slot: usize,
        project: &Project,
        clip: Option<&Clip>,
    ) -> Result<()> {
        let extras = project.studio.sources.len();
        if extras >= MAX_SOURCES || extras > clip.map_or(0, |root| root.sources.len()) {
            return Err("Project source audio is incomplete.".into());
        }
        let count = extras + usize::from(clip.is_some());
        validate_sources(project, count)?;
        let mut remaining = MAX_PROJECT_FRAMES;
        let sources = clip.into_iter().flat_map(|root| {
            std::iter::once(root).chain(root.sources.iter().take(extras).map(Arc::as_ref))
        });
        let mut audio = Vec::with_capacity(count);
        fs::create_dir_all(dir)?;
        for source in sources {
            let frames = source.frames.len();
            if frames == 0
                || frames > MAX_FRAMES
                || frames > remaining
                || !(1_000..=384_000).contains(&source.rate)
            {
                return Err("Invalid recovery audio.".into());
            }
            remaining -= frames;
            audio.push(self.store_blob(dir, source)?);
        }
        let manifest = Manifest {
            version: VERSION,
            project,
            audio,
        };
        let json = serde_json::to_vec(&manifest)?;
        if json.len() > MANIFEST_LIMIT {
            return Err("This project has too much pattern data to recover.".into());
        }
        write_atomically(&dir.join(SLOTS[slot]), |file| Ok(file.write_all(&json)?))?;
        self.slots[slot] = manifest.audio;
        Ok(())
    }

    fn store_blob(&mut self, dir: &Path, source: &Clip) -> Result<u64> {
        let identity = source.audio_id();
        if let Some(blob) = self.blobs.iter().find(|blob| {
            blob.rate == source.rate
                && blob.identity.ptr_eq(&identity)
                && blob_path(dir, blob.id).is_file()
        }) {
            return Ok(blob.id);
        }
        // A writer owns one session directory. Advance the name even if a write fails.
        self.last_id += 1;
        let id = self.last_id;
        write_atomically(&blob_path(dir, id), |file| {
            let mut writer = BufWriter::new(file);
            write_project_clip(&mut writer, Some(source))?;
            writer.flush()?;
            Ok(())
        })?;
        self.blobs.push(Blob {
            id,
            rate: source.rate,
            identity,
        });
        Ok(id)
    }

    pub fn clear(&mut self, dir: &Path) -> Result<()> {
        remove(dir)?;
        self.slots = Default::default();
        self.blobs.clear();
        Ok(())
    }
}

fn blob_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("audio-{id}.pcm"))
}

fn validate_sources(project: &Project, count: usize) -> Result<()> {
    if count > MAX_SOURCES
        || (count != 0 && count != project.studio.sources.len() + 1)
        || (count == 0 && !project.studio.sources.is_empty())
    {
        return Err("Invalid recovery source list.".into());
    }
    if !project.source_ids.is_empty() || !project.snapshots.is_empty() {
        validate_snapshot_sources(project, count)?;
    }
    Ok(())
}

pub fn open(path: &Path) -> Result<(Project, Option<Clip>)> {
    if path
        .extension()
        .is_some_and(|extension| extension == "flip")
    {
        return open_project(path);
    }
    let mut json = Vec::new();
    File::open(path)?
        .take((MANIFEST_LIMIT + 1) as u64)
        .read_to_end(&mut json)?;
    if json.len() > MANIFEST_LIMIT {
        return Err("Invalid recovery metadata.".into());
    }
    let Manifest {
        version,
        mut project,
        audio,
    } = serde_json::from_slice::<Manifest<Project>>(&json)?;
    if version != VERSION {
        return Err("Unsupported recovery format.".into());
    }
    validate_sources(&project, audio.len())?;
    let dir = path.parent().ok_or("Missing recovery directory.")?;
    let mut remaining = MAX_PROJECT_FRAMES;
    let mut sources = audio.into_iter().map(|id| -> Result<Clip> {
        let mut reader = BufReader::new(File::open(blob_path(dir, id))?);
        read_project_clip(&mut reader, &mut remaining)?
            .ok_or_else(|| "Missing recovery audio.".into())
    });
    let mut clip = sources.next().transpose()?;
    if let Some(root) = &mut clip {
        for source in sources {
            root.sources.push(Arc::new(source?));
        }
    }
    validate_loaded_project(&mut project, clip.as_ref());
    Ok((project, clip))
}

pub fn find(root: &Path) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .flat_map(|entry| {
            SLOTS
                .into_iter()
                .chain(LEGACY_SLOTS)
                .map(move |name| entry.path().join(name))
        })
        .filter_map(|path| {
            let metadata = path.metadata().ok()?;
            metadata
                .is_file()
                .then_some((metadata.modified().ok()?, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

pub fn remove(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err("Invalid recovery directory.".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    for name in SLOTS.into_iter().chain(LEGACY_SLOTS) {
        remove_file(&dir.join(name))?;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let blob = name
            .strip_prefix("audio-")
            .and_then(|name| name.strip_suffix(".pcm"))
            .is_some_and(|id| id.parse::<u64>().is_ok());
        if blob || name.starts_with(".tmp") {
            remove_file(&entry.path())?;
        }
    }
    fs::remove_dir(dir)?;
    Ok(())
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(sample: f32) -> Clip {
        Clip::new(8_000, vec![[sample; 2]; 1_600])
    }

    #[test]
    fn recovery_reuses_unchanged_audio_and_keeps_blobs_for_both_slots() -> Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("session");
        let mut writer = Writer::default();
        let mut project = Project::default();
        let mut primary = clip(0.25);
        let extra = Arc::new(clip(0.5));
        project
            .studio
            .sources
            .push(super::super::local_record(PathBuf::from("extra.wav")));
        primary.sources.push(extra.clone());
        writer.save(&dir, 0, &project, Some(&primary))?;
        let first_ids = writer.slots[0].clone();

        project.studio.bpm = 123;
        writer.save(&dir, 1, &project, Some(&primary.clone()))?;
        assert_eq!(writer.slots[1], first_ids);
        assert_eq!(writer.last_id, 2);
        let (restored, audio) = open(&dir.join(SLOTS[1]))?;
        assert_eq!(restored.studio.bpm, 123);
        let audio = audio.unwrap();
        assert_eq!(audio.frames[0], [0.25; 2]);
        assert_eq!(audio.sources[0].frames[0], [0.5; 2]);

        let mut replacement = clip(0.75);
        replacement.sources.push(extra);
        writer.save(&dir, 0, &project, Some(&replacement))?;
        assert_eq!(writer.slots[0][1], first_ids[1]);
        assert!(blob_path(&dir, first_ids[0]).exists());
        assert_eq!(open(&dir.join(SLOTS[1]))?.1.unwrap().frames[0], [0.25; 2]);
        writer.save(&dir, 1, &project, Some(&replacement))?;
        assert!(!blob_path(&dir, first_ids[0]).exists());
        assert_eq!(writer.blobs.len(), 2);
        writer.clear(&dir)?;
        assert!(!dir.exists());
        Ok(())
    }

    #[test]
    fn failed_manifest_publish_keeps_the_previous_snapshot_and_reclaims_new_audio() -> Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path();
        let mut writer = Writer::default();
        let project = Project::default();
        writer.save(dir, 0, &project, Some(&clip(0.25)))?;
        fs::create_dir(dir.join(SLOTS[1]))?;
        assert!(writer.save(dir, 1, &project, Some(&clip(0.5))).is_err());
        assert!(writer.slots[1].is_empty());
        assert_eq!(writer.blobs.len(), 1);
        assert_eq!(open(&dir.join(SLOTS[0]))?.1.unwrap().frames[0], [0.25; 2]);
        assert!(!blob_path(dir, 2).exists());
        Ok(())
    }

    #[test]
    fn discovers_and_opens_legacy_recovery_projects() -> Result<()> {
        let root = tempfile::tempdir()?;
        let dir = root.path().join("old-session");
        fs::create_dir(&dir)?;
        let path = dir.join(LEGACY_SLOTS[0]);
        super::super::save_project(&path, &Project::default(), Some(&clip(0.25)))?;
        assert_eq!(find(root.path()), Some(path.clone()));
        assert_eq!(open(&path)?.1.unwrap().frames[0], [0.25; 2]);
        remove(&dir)?;
        assert!(find(root.path()).is_none());
        Ok(())
    }
}
