use crate::{
    audio::Clip,
    files::{self, Result},
    model::{Record, Source},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufReader, ErrorKind},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use url::Url;

pub struct VideoLink {
    pub id: String,
    pub start: Option<f64>,
}

impl VideoLink {
    pub fn parse(input: &str) -> Result<Self> {
        let invalid = "Paste a link to one YouTube video.";
        let url = Url::parse(input.trim()).map_err(|_| invalid)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(invalid.into());
        }
        let mut path = url.path_segments().ok_or(invalid)?;
        let first = path.next();
        let id = match (url.host_str(), first) {
            (Some("youtu.be"), Some(id)) if path.next().is_none() => Some(id.to_owned()),
            (
                Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"),
                Some("watch"),
            ) => url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned()),
            (
                Some("youtube.com" | "www.youtube.com" | "m.youtube.com"),
                Some("shorts" | "live" | "embed"),
            ) => path
                .next()
                .filter(|_| path.next().is_none())
                .map(str::to_owned),
            _ => None,
        }
        .filter(|id| valid_id(id))
        .ok_or(invalid)?;
        let start = url
            .query_pairs()
            .find(|(key, _)| key == "t" || key == "start")
            .and_then(|(_, value)| timestamp(&value))
            .or_else(|| url.fragment()?.strip_prefix("t=").and_then(timestamp));
        Ok(Self { id, start })
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn timestamp(value: &str) -> Option<f64> {
    if let Ok(seconds) = value.parse::<u32>() {
        return Some(f64::from(seconds));
    }
    let mut rest = value;
    let mut total = 0;
    let mut previous = 3601;
    while let Some(index) = rest.find(|c: char| !c.is_ascii_digit()) {
        let seconds = rest.get(..index)?.parse::<u32>().ok()?;
        let multiplier = match rest.as_bytes()[index] {
            b'h' => 3600,
            b'm' => 60,
            b's' => 1,
            _ => return None,
        };
        if multiplier >= previous {
            return None;
        }
        previous = multiplier;
        total += seconds.checked_mul(multiplier)? as u64;
        rest = rest.get(index + 1..)?;
    }
    (rest.is_empty() && previous != 3601).then_some(total as f64)
}

pub fn watch_url(id: &str) -> Option<String> {
    valid_id(id).then(|| format!("https://www.youtube.com/watch?v={id}"))
}

const MIN_RECORD_SECONDS: f64 = 75.0;
const MAX_RECORD_SECONDS: f64 = 600.0;
const MAX_DIG_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Default)]
pub struct DigFilters {
    pub decade: Option<u16>,
    pub max_views: Option<u64>,
}

pub struct DigResult {
    pub record: Record,
    pub clip: Clip,
    pub views: Option<u64>,
}

#[derive(Deserialize)]
struct SearchResults {
    entries: Vec<Option<SearchEntry>>,
}

#[derive(Deserialize)]
struct SearchEntry {
    id: String,
    title: String,
    duration: Option<f64>,
    view_count: Option<u64>,
    live_status: Option<String>,
}

fn dig_query(genre: &str, seed: u64, decade: Option<u16>) -> String {
    let variant = ((seed >> 12) % 3) as usize;
    let style = match genre {
        "Jazz" => ["jazz funk", "soul jazz", "spiritual jazz"][variant],
        "Soul" => ["deep soul", "northern soul", "sweet soul"][variant],
        "Fusion" => "jazz fusion",
        "Psych" => "psychedelic rock",
        "Library" => "library music",
        "Synth" => "synth electronic",
        "World" => ["world groove", "afrobeat", "anatolian folk"][variant],
        "Hip Hop" => "underground hip hop",
        "Other" => "instrumental groove",
        other => other,
    };
    let decades = match genre {
        "Classical" | "Blues" | "Country" | "Gospel" | "Folk" => [1950, 1960, 1970],
        "Hip Hop" | "Ambient" | "Synth" => [1980, 1990, 2000],
        "Rock" | "Psych" | "Soul" | "Pop" => [1960, 1970, 1980],
        _ => [1970, 1970, 1980],
    };
    let format = match genre {
        "Ambient" | "Disco" | "Fusion" | "Hip Hop" | "Jazz" | "Library" | "Soundtrack"
        | "Synth" => "vinyl rip",
        _ => "45 rpm",
    };
    let decade = decade.unwrap_or(decades[(seed % 3) as usize]);
    format!("{decade}s rare {style} {format}")
}

fn title_decade(title: &str) -> Option<u16> {
    let mut detected = None;
    for (start, digit) in title.char_indices() {
        let prefix = &title[..start];
        if !digit.is_ascii_digit()
            || prefix
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
        {
            continue;
        }
        let length = title[start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if !matches!(length, 2 | 4) {
            continue;
        }
        let suffix = &title[start + length..];
        let decade_suffix = suffix
            .trim_start_matches(['\'', '’'])
            .strip_prefix(['s', 'S'])
            .is_some_and(|tail| !tail.starts_with(char::is_alphanumeric));
        if !decade_suffix && suffix.starts_with(char::is_alphanumeric) {
            continue;
        }
        let number = title[start..start + length].parse::<u16>().ok()?;
        let quoted = prefix
            .strip_suffix(['\'', '’'])
            .is_some_and(|prefix| !prefix.ends_with(char::is_alphanumeric));
        let year = match (length, number) {
            (4, 1900..=2029) => number,
            (2, 30..=99) if quoted || (decade_suffix && number.is_multiple_of(10)) => 1900 + number,
            (2, 0..=19) if quoted || (decade_suffix && number.is_multiple_of(10)) => 2000 + number,
            _ => continue,
        };
        let decade = year / 10 * 10;
        if detected.is_some_and(|previous| previous != decade) {
            return None;
        }
        detected = Some(decade);
    }
    detected
}

fn search(query: &str) -> Result<SearchResults> {
    crate::background::stage("Searching YouTube")?;
    let search_url = format!("ytsearch32:{query}");
    let output = run(
        yt_dlp()?.args([
            "--flat-playlist",
            "--skip-download",
            "--dump-single-json",
            "--",
            &search_url,
        ]),
        "YouTube search",
    )?;
    Ok(serde_json::from_slice(&output)?)
}

fn candidates(
    results: SearchResults,
    seed: u64,
    active: Option<&str>,
    heard: &HashSet<String>,
    filters: DigFilters,
) -> Vec<SearchEntry> {
    let mut unique = HashSet::new();
    let mut entries = results
        .entries
        .into_iter()
        .flatten()
        .filter(|entry| {
            valid_id(&entry.id)
                && Some(entry.id.as_str()) != active
                && filters
                    .max_views
                    .is_none_or(|limit| entry.view_count.is_some_and(|views| views <= limit))
                && filters
                    .decade
                    .is_none_or(|decade| title_decade(&entry.title) == Some(decade))
                && entry.duration.is_some_and(|duration| {
                    (MIN_RECORD_SECONDS..=MAX_RECORD_SECONDS).contains(&duration)
                })
                && !matches!(
                    entry.live_status.as_deref(),
                    Some("is_live" | "is_upcoming")
                )
                && {
                    let title = entry.title.to_ascii_lowercase();
                    ![
                        "tutorial",
                        "reaction",
                        "type beat",
                        "dj set",
                        "full album",
                        "compilation",
                        "records for sale",
                        "vinyl finds",
                        "ai generated",
                        "ai-generated",
                    ]
                    .iter()
                    .any(|term| title.contains(term))
                }
                && unique.insert(entry.id.clone())
        })
        .collect::<Vec<_>>();
    entries.sort_by_cached_key(|entry| {
        let mut random = DefaultHasher::new();
        seed.hash(&mut random);
        entry.id.hash(&mut random);
        (
            heard.contains(&entry.id),
            entry.view_count.is_some_and(|views| views >= 250_000),
            random.finish(),
        )
    });
    entries
}

pub fn discover(
    genre: &str,
    seed: u64,
    excluded: &HashSet<String>,
    filters: DigFilters,
    release: &crate::catalog::ReleaseFilters,
) -> Result<Vec<crate::model::QueueEntry>> {
    use crate::model::QueueEntry;
    let heard = HashSet::new();
    if release.active() {
        let mut found = Vec::new();
        let mut used = excluded.clone();
        let tracks = crate::catalog::tracks(genre, release, filters.decade, seed)?;
        for track in tracks.into_iter().take(6) {
            let artist = search_words(&track.artist);
            let title = search_words(&track.title);
            if artist.is_empty() || title.is_empty() {
                continue;
            }
            let query = format!("{artist} {title}");
            let options = candidates(
                search(&query)?,
                seed,
                None,
                &heard,
                DigFilters {
                    decade: None,
                    max_views: filters.max_views,
                },
            );
            if let Some(entry) = options.into_iter().find(|entry| {
                let video = search_words(&entry.title).to_lowercase();
                let contains = |text: &str| {
                    text.to_lowercase()
                        .split_whitespace()
                        .all(|word| video.split_whitespace().any(|token| token == word))
                };
                contains(&artist) && contains(&title) && used.insert(entry.id.clone())
            }) {
                found.push(QueueEntry::new(
                    Record {
                        title: track.title,
                        artist: track.artist,
                        genre: genre.to_owned(),
                        source: Source::YouTube { id: entry.id },
                        release: Some(Box::new(track.release)),
                    },
                    entry.view_count,
                ));
            }
        }
        if found.is_empty() {
            return Err(
                "No new YouTube tracks matched these Discogs releases and view filters.".into(),
            );
        }
        return Ok(found);
    }
    let query = dig_query(genre, seed, filters.decade);
    let collect = |results| {
        candidates(results, seed, None, &heard, filters)
            .into_iter()
            .filter(|entry| !excluded.contains(&entry.id))
            .take(12)
            .map(|entry| {
                QueueEntry::new(
                    Record {
                        title: entry.title,
                        artist: "YouTube".into(),
                        genre: genre.to_owned(),
                        source: Source::YouTube { id: entry.id },
                        release: None,
                    },
                    entry.view_count,
                )
            })
            .collect::<Vec<_>>()
    };
    let entries = collect(search(&query)?);
    if !entries.is_empty() {
        return Ok(entries);
    }
    let fallback = filters.decade.map_or_else(
        || format!("{genre} original single vinyl rip"),
        |decade| format!("{decade}s {genre} original single vinyl rip"),
    );
    let entries = collect(search(&fallback)?);
    if entries.is_empty() {
        return Err(
            "No unheard tracks matched these filters. Try another genre or broaden the filters."
                .into(),
        );
    }
    Ok(entries)
}

fn search_words(value: &str) -> String {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn dig(
    genre: &str,
    seed: u64,
    active: Option<&str>,
    heard: &HashSet<String>,
    filters: DigFilters,
) -> Result<DigResult> {
    let cache = files::app_data_dir().map(|dir| dir.join("youtube"));
    dig_at(genre, seed, active, heard, filters, cache.as_deref())
}

fn dig_at(
    genre: &str,
    seed: u64,
    active: Option<&str>,
    heard: &HashSet<String>,
    filters: DigFilters,
    cache: Option<&Path>,
) -> Result<DigResult> {
    let query = dig_query(genre, seed, filters.decade);
    let mut entries = candidates(search(&query)?, seed, active, heard, filters);
    if entries.is_empty() {
        let fallback = filters.decade.map_or_else(
            || format!("{genre} original single vinyl rip"),
            |decade| format!("{decade}s {genre} original single vinyl rip"),
        );
        entries = candidates(search(&fallback)?, seed, active, heard, filters);
    }
    let mut last_error = None;
    for entry in entries.into_iter().take(MAX_DIG_ATTEMPTS) {
        crate::background::check()?;
        match load_at(&entry.id, cache) {
            Ok((mut record, clip)) => {
                record.genre = genre.to_owned();
                return Ok(DigResult {
                    record,
                    clip,
                    views: entry.view_count,
                });
            }
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(format!("Could not load a {genre} record. Try Dig again. {error}").into());
    }
    if filters.decade.is_some() || filters.max_views.is_some() {
        return Err(format!("No {genre} tracks matched these filters. Try Dig again or adjust the decade or view limit.").into());
    }
    Err(format!("No matching {genre} tracks found. Try Dig again or choose another genre.").into())
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    id: String,
    title: String,
    uploader: Option<String>,
}

impl Metadata {
    fn record(self) -> Record {
        Record {
            title: self.title,
            artist: self.uploader.unwrap_or_else(|| "YouTube".into()),
            genre: "Other".into(),
            source: Source::YouTube { id: self.id },
            release: None,
        }
    }
}

pub fn load(id: &str) -> Result<(Record, Clip)> {
    let cache = files::app_data_dir().map(|dir| dir.join("youtube"));
    load_at(id, cache.as_deref())
}

fn load_at(id: &str, cache: Option<&Path>) -> Result<(Record, Clip)> {
    let url = watch_url(id).ok_or("Invalid YouTube video ID")?;
    let cache_paths = cache.map(|dir| {
        (
            dir.join(format!("{id}.m4a")),
            dir.join(format!("{id}.json")),
        )
    });
    if let Some((audio, metadata)) = &cache_paths
        && let Ok((metadata, clip)) = read_source(id, audio, metadata)
    {
        return Ok((metadata.record(), clip));
    }

    let temp = tempfile::tempdir()?;
    download(&url, temp.path())?;
    let audio = temp.path().join("audio.m4a");
    let metadata = temp.path().join("audio.info.json");
    if !audio.is_file() {
        return Err("No audio downloaded. Import a video up to 10 minutes and 64 MB.".into());
    }
    let (metadata, clip) = read_source(id, &audio, &metadata)?;
    if let Some(cache) = cache
        && let Some((cached_audio, cached_metadata)) = cache_paths
    {
        // The downloaded clip is usable even when the cache cannot be written.
        let cache_source = || -> Result<()> {
            fs::create_dir_all(cache)?;
            files::write_atomically(&cached_metadata, |file| {
                serde_json::to_writer(file, &metadata)?;
                Ok(())
            })?;
            files::write_atomically(&cached_audio, |file| {
                std::io::copy(&mut fs::File::open(&audio)?, file)?;
                Ok(())
            })
        };
        let _ = cache_source();
    }
    Ok((metadata.record(), clip))
}

pub fn cached_record(audio: &Path) -> Option<Record> {
    let id = audio.file_stem()?.to_str()?;
    watch_url(id)?;
    let metadata: Metadata = serde_json::from_reader(BufReader::new(fs::File::open(audio.with_extension("json")).ok()?)).ok()?;
    (metadata.id == id).then(|| metadata.record())
}

fn read_source(id: &str, audio: &Path, metadata: &Path) -> Result<(Metadata, Clip)> {
    let metadata: Metadata = serde_json::from_reader(BufReader::new(fs::File::open(metadata)?))?;
    if metadata.id != id {
        return Err("YouTube metadata does not match the requested video".into());
    }
    Ok((metadata, files::decode_path(audio)?))
}

pub(crate) fn downloader() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["yt-dlp.exe", "yt-dlp.cmd"]
    } else {
        &["yt-dlp"]
    };
    let app_dir = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let path = env::var_os("PATH");
    app_dir
        .into_iter()
        .chain(path.as_deref().into_iter().flat_map(env::split_paths))
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

fn yt_dlp() -> Result<Command> {
    let executable = downloader().ok_or("YouTube needs yt-dlp on your PATH.")?;
    let mut command = Command::new(executable);
    command.stdin(Stdio::null()).args([
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-update",
        "--no-progress",
        "--socket-timeout",
        "12",
        "--retries",
        "1",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    Ok(command)
}

fn download(url: &str, directory: &Path) -> Result<()> {
    crate::background::stage("Downloading source audio")?;
    let mut command = yt_dlp()?;
    command.current_dir(directory).stdout(Stdio::null()).args([
        "--no-playlist",
        "--js-runtimes",
        "node",
        "--fragment-retries",
        "1",
        "--match-filters",
        "duration <= 600 & !is_live",
        "--max-filesize",
        "64M",
        "--write-info-json",
        "-f",
        "bestaudio[ext=m4a]",
        "-o",
        "audio.m4a",
        "--",
        url,
    ]);
    run(&mut command, "YouTube import")?;
    Ok(())
}

fn run(command: &mut Command, operation: &str) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command.stdout(stdout.try_clone()?).stderr(stderr.try_clone()?).spawn().map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            "Could not start yt-dlp. Check that it is installed and available on PATH.".to_owned()
        } else {
            format!("Could not start yt-dlp: {error}")
        }
    })?;
    let status = loop {
        match crate::background::check().and_then(|_| child.try_wait()) {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(80)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    };
    if !status.success() {
        let start = stderr.metadata()?.len().saturating_sub(16 * 1024);
        stderr.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        let stderr = String::from_utf8_lossy(&bytes);
        let message = stderr
            .lines()
            .rev()
            .find(|line| line.starts_with("ERROR:"))
            .or_else(|| stderr.lines().rev().find(|line| !line.trim().is_empty()))
            .unwrap_or("The downloader did not return an error message.");
        return Err(format!("{operation} failed. {message}").into());
    }
    stdout.rewind()?;
    let mut bytes = Vec::new();
    stdout.take(8 * 1024 * 1024).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dig_candidates_prefer_unheard_records_and_skip_non_tracks() {
        let entry = |id: &str, title: &str, duration, views| SearchEntry {
            id: id.into(),
            title: title.into(),
            duration: Some(duration),
            view_count: Some(views),
            live_status: None,
        };
        let results = SearchResults {
            entries: vec![
                Some(entry("aaaaaaaaaaa", "Single A", 180.0, 300)),
                Some(entry("bbbbbbbbbbb", "Single B", 210.0, 300)),
                Some(entry("ccccccccccc", "Single C", 160.0, 700_000)),
                Some(entry("bbbbbbbbbbb", "Duplicate", 210.0, 300)),
                Some(entry("ddddddddddd", "Long mix", 800.0, 50)),
                Some(entry("eeeeeeeeeee", "Mixing tutorial", 300.0, 50)),
                Some(entry("fffffffffff", "Current track", 240.0, 50)),
            ],
        };
        let heard = HashSet::from(["aaaaaaaaaaa".into()]);
        let ordered = candidates(
            results,
            1,
            Some("fffffffffff"),
            &heard,
            DigFilters::default(),
        );
        let ids = ordered
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["bbbbbbbbbbb", "ccccccccccc", "aaaaaaaaaaa"]);
    }

    #[test]
    fn dig_filters_require_matching_era_and_a_known_count_within_the_cap() {
        let entry = |id: &str, title: &str, view_count| {
            Some(SearchEntry {
                id: id.into(),
                title: title.into(),
                duration: Some(180.0),
                view_count,
                live_status: None,
            })
        };
        let results = SearchResults {
            entries: vec![
                entry("aaaaaaaaaaa", "1976 Rare Funk 45", Some(5_000)),
                entry("bbbbbbbbbbb", "Rare ’70s Funk 45", Some(500)),
                entry("ccccccccccc", "Rare Funk ’74", Some(500)),
                entry("ddddddddddd", "1980 Funk 45", Some(200)),
                entry("eeeeeeeeeee", "1971 Funk 45", Some(5_001)),
                entry("fffffffffff", "Funk (1974)", None),
                entry("ggggggggggg", "Rare Funk 45", Some(200)),
                entry("hhhhhhhhhhh", "1968 vs 1972 versions", Some(200)),
            ],
        };
        let filters = DigFilters {
            decade: Some(1970),
            max_views: Some(5_000),
        };
        let mut ids = candidates(results, 1, None, &HashSet::new(), filters)
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc"]);
    }

    #[test]
    #[ignore = "searches YouTube and downloads one recording into a temporary directory"]
    fn digs_a_playable_youtube_record_by_genre() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let filters = DigFilters {
            decade: Some(1970),
            max_views: Some(5_000),
        };
        let DigResult {
            record,
            clip,
            views,
        } = dig_at(
            "Funk",
            0,
            None,
            &HashSet::new(),
            filters,
            Some(directory.path()),
        )?;
        assert_eq!(record.genre, "Funk");
        assert!(matches!(record.source, Source::YouTube { .. }));
        assert!(views.is_some_and(|count| count <= 5_000));
        assert!((MIN_RECORD_SECONDS..=MAX_RECORD_SECONDS).contains(&clip.duration()));
        assert!(clip.peaks.iter().any(|&peak| peak > 0.02));
        Ok(())
    }

    #[test]
    #[ignore = "searches Discogs and YouTube and decodes one catalog match"]
    fn digs_a_playable_track_with_release_metadata() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let entries = discover(
            "Soul",
            0,
            &HashSet::new(),
            DigFilters {
                decade: Some(1970),
                max_views: None,
            },
            &crate::catalog::ReleaseFilters {
                year: Some(1975),
                country: "US".into(),
                label: "Motown".into(),
            },
        )?;
        assert!(!entries.is_empty());
        for entry in &entries {
            let release = entry.record.release.as_ref().unwrap();
            assert_eq!(release.year, 1975);
            assert_eq!(release.country, "US");
            assert_eq!(release.label, "Motown");
        }
        let mut failures = Vec::new();
        for entry in entries {
            let Source::YouTube { id } = &entry.record.source else {
                unreachable!()
            };
            match load_at(id, Some(directory.path())) {
                Ok((_, clip)) => {
                    assert!(clip.duration() >= MIN_RECORD_SECONDS);
                    assert!(clip.peaks.iter().any(|&peak| peak > 0.02));
                    return Ok(());
                }
                Err(error) => failures.push(error.to_string()),
            }
        }
        Err(format!("No catalog match decoded: {}", failures.join("; ")).into())
    }

    #[test]
    fn parses_single_video_links_and_timestamps() -> Result<()> {
        let watch =
            VideoLink::parse("https://www.youtube.com/watch?v=ac7KhViaVqc&t=1m12s&list=ignored")?;
        let short = VideoLink::parse(" https://youtu.be/ac7KhViaVqc#t=72 ")?;
        assert_eq!(watch.id, short.id);
        assert_eq!(watch.start, Some(72.0));
        assert_eq!(short.start, watch.start);
        assert!(VideoLink::parse("https://youtube.com.evil.test/watch?v=ac7KhViaVqc").is_err());
        assert!(VideoLink::parse("https://youtu.be/../../audio.m4a").is_err());
        assert!(VideoLink::parse("https://www.youtube.com/playlist?list=123").is_err());
        Ok(())
    }

    #[test]
    #[ignore = "imports one open-movie trailer from YouTube"]
    fn imports_and_reopens_cached_youtube_audio() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let id = "ac7KhViaVqc";
        let (record, clip) = load_at(id, Some(directory.path()))?;
        assert!(record.title.contains("Sintel"));
        assert!(clip.duration() > 15.0);
        let cache = read_source(
            id,
            &directory.path().join(format!("{id}.m4a")),
            &directory.path().join(format!("{id}.json")),
        )?;
        assert_eq!(cache.0.record(), record);
        assert_eq!(cache.1.frames, clip.frames);
        Ok(())
    }
}
