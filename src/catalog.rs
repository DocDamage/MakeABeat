use crate::{files::Result, model::ReleaseInfo};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct ReleaseFilters {
    pub year: Option<u16>,
    pub country: String,
    pub label: String,
}

impl ReleaseFilters {
    pub fn active(&self) -> bool {
        self.year.is_some() || !self.country.is_empty() || !self.label.is_empty()
    }
}

pub struct CatalogTrack {
    pub title: String,
    pub artist: String,
    pub release: ReleaseInfo,
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    results: Vec<Release>,
}

#[derive(Deserialize)]
struct Release {
    id: u64,
    title: String,
    year: Option<String>,
    #[serde(default)]
    country: String,
    #[serde(default)]
    label: Vec<String>,
}

#[derive(Deserialize)]
struct TrackList {
    #[serde(default)]
    tracklist: Vec<Track>,
}

#[derive(Deserialize)]
struct Track {
    title: String,
    #[serde(default)]
    artists: Vec<Artist>,
    #[serde(default)]
    type_: String,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}

pub fn tracks(
    genre: &str,
    filters: &ReleaseFilters,
    decade: Option<u16>,
    seed: u64,
) -> Result<Vec<CatalogTrack>> {
    let client = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent("LostSamplesDesktop/0.1")
        .build();
    let mut request = client
        .get("https://api.discogs.com/database/search")
        .query("type", "release")
        .query("per_page", "30");
    if let Ok(token) = std::env::var("DISCOGS_TOKEN")
        && !token.trim().is_empty()
    {
        request = request.set("Authorization", &format!("Discogs token={}", token.trim()));
    }
    let year_query = filters
        .year
        .map(|year| year.to_string())
        .or_else(|| decade.map(|year| format!("{year}-{}", year + 9)));
    if let Some(year) = year_query {
        request = request.query("year", &year);
    }
    if !filters.country.is_empty() {
        request = request.query("country", &filters.country);
    }
    if !filters.label.is_empty() {
        request = request.query("label", &filters.label);
    }
    let (field, term) = match genre {
        "Funk" | "Soul" | "Disco" => ("style", genre),
        "Hip Hop" => ("genre", "Hip Hop"),
        "World" => ("genre", "Folk, World, & Country"),
        "Psych" => ("style", "Psychedelic Rock"),
        "Synth" => ("style", "Synth-pop"),
        "All" | "Other" => ("genre", ""),
        "Rock" | "Jazz" | "Pop" | "Latin" | "Blues" | "Classical" => ("genre", genre),
        _ => ("style", genre),
    };
    if !term.is_empty() {
        request = request.query(field, term);
    }
    let response = request.call().map_err(|error| match error {
        ureq::Error::Status(401 | 403, _) => {
            "Discogs requires access for this search. Set DISCOGS_TOKEN and restart the app.".into()
        }
        ureq::Error::Status(429, _) => {
            "Discogs is rate limiting searches. Try again in a minute.".into()
        }
        error => format!("Discogs search failed: {error}"),
    })?;
    let mut releases = response.into_json::<Search>()?.results;
    releases.retain(|release| {
        release
            .year
            .as_deref()
            .and_then(|year| year.parse::<u16>().ok())
            .is_some_and(|year| {
                filters.year.is_none_or(|expected| year == expected)
                    && decade.is_none_or(|decade| (decade..decade + 10).contains(&year))
            })
            && (filters.country.is_empty()
                || release.country.eq_ignore_ascii_case(&filters.country))
            && (filters.label.is_empty()
                || release
                    .label
                    .iter()
                    .any(|label| label.eq_ignore_ascii_case(&filters.label)))
    });
    if releases.is_empty() {
        return Ok(Vec::new());
    }
    let offset = seed as usize % releases.len();
    releases.rotate_left(offset);
    let mut tracks = Vec::new();
    let mut last_error = None;
    for release in releases.into_iter().take(4) {
        let Some((artist, title)) = release.title.split_once(" - ") else {
            continue;
        };
        let url = format!("https://api.discogs.com/releases/{}", release.id);
        let listing = match client
            .get(&url)
            .call()
            .map_err(|error| error.to_string())
            .and_then(|response| {
                response
                    .into_json::<TrackList>()
                    .map_err(|error| error.to_string())
            }) {
            Ok(listing) => listing,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let info = ReleaseInfo {
            id: release.id.to_string(),
            title: title.into(),
            country: release.country,
            year: release
                .year
                .and_then(|year| year.parse().ok())
                .unwrap_or_default(),
            label: release
                .label
                .iter()
                .find(|label| label.eq_ignore_ascii_case(&filters.label))
                .or_else(|| release.label.first())
                .cloned()
                .unwrap_or_default(),
        };
        let mut eligible: Vec<_> = listing
            .tracklist
            .into_iter()
            .filter(|track| track.type_ == "track" && !track.title.is_empty())
            .collect();
        if !eligible.is_empty() {
            let offset = (seed as usize / 17) % eligible.len();
            eligible.rotate_left(offset);
        }
        for track in eligible.into_iter().take(2) {
            let artist = track.artists.first().map_or(artist, |artist| &artist.name);
            if !artist.eq_ignore_ascii_case("various") {
                tracks.push(CatalogTrack {
                    title: track.title,
                    artist: artist.into(),
                    release: info.clone(),
                });
            }
        }
    }
    if tracks.is_empty()
        && let Some(error) = last_error
    {
        return Err(format!("Could not load Discogs tracklists: {error}").into());
    }
    Ok(tracks)
}
