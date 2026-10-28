//! The directory's JSON as it arrives, and its projection onto the entity types.
//!
//! Every shape takes a container-level `#[serde(default)]`: the API adds and removes fields
//! unannounced, and a station dropped over one nobody reads is a station nobody can find.

use serde::Deserialize;

use melodia_core::entities::radio::{DirectoryStation, Facet};

/// One row of `/json/stations/search`, listing only the fields [`DirectoryStation`] keeps.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiStation {
    pub stationuuid: String,
    pub name: String,
    /// The fallback where the directory has not resolved one.
    pub url: String,
    /// Already followed past any `.pls`/`.m3u` indirection.
    pub url_resolved: String,
    pub homepage: String,
    pub favicon: String,
    pub tags: String,
    pub country: String,
    pub countrycode: String,
    pub state: String,
    pub language: String,
    pub codec: String,
    pub bitrate: i32,
    /// `0` or `1`, not a JSON bool.
    pub hls: i32,
    pub votes: i64,
    pub clickcount: i64,
    /// `0` or `1`, not a JSON bool.
    pub lastcheckok: i32,
}

impl ApiStation {
    pub fn into_directory_station(self) -> DirectoryStation {
        DirectoryStation {
            station_uuid: self.stationuuid,
            // A leading tab sorts ahead of every letter and paints as a gap down the cards.
            name: self.name.trim().to_owned(),
            stream_url: if self.url_resolved.is_empty() { self.url } else { self.url_resolved },
            homepage: non_empty(self.homepage),
            favicon_url: non_empty(self.favicon),
            tags: self.tags,
            country: self.country,
            country_code: self.countrycode,
            state: self.state,
            language: self.language,
            codec: self.codec,
            bitrate: self.bitrate,
            hls: self.hls != 0,
            votes: self.votes,
            click_count: self.clickcount,
            last_check_ok: self.lastcheckok != 0,
        }
    }
}

/// One entry of `/json/servers`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiServer {
    pub ip: String,
    pub name: String,
}

/// One entry of a facet list. The code field is `iso_3166_1` for countries, `iso_639` for
/// languages, and absent for tags and codecs.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiFacet {
    pub name: String,
    pub stationcount: i64,
    pub iso_3166_1: Option<String>,
    pub iso_639: Option<String>,
}

impl ApiFacet {
    pub fn into_facet(self) -> Facet {
        Facet {
            code: self.iso_3166_1.or(self.iso_639).filter(|code| !code.is_empty()),
            name: self.name,
            station_count: self.stationcount,
        }
    }
}

/// The answer to `/json/vote/{uuid}`, the one response whose success is in the body; `message` is
/// the server's own wording for a refusal.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiVote {
    pub ok: bool,
    pub message: String,
}

/// The distinct hostnames in a mirror list, which names each mirror once per address family. The
/// `ip` is dropped: the certificate is for the name, so an IP-keyed request fails TLS.
pub fn hosts(servers: &[ApiServer]) -> Vec<String> {
    let mut names: Vec<String> = servers
        .iter()
        .map(|server| server.name.trim())
        .filter(|name| is_bare_host(name))
        .map(str::to_owned)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// A host and nothing else: one carrying a scheme or separator of its own would rewrite the path
/// `url_for` builds.
fn is_bare_host(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', ':', '?', '#']) && !name.contains(char::is_whitespace)
}

/// Whether a uuid is safe as a request path segment. Not only the directory issues them: an
/// imported station list carries whatever the file named, and a `/` or `?` would reach past the
/// endpoint. Shaped like what the directory issues rather than parsed as a UUID.
pub fn is_path_safe_uuid(uuid: &str) -> bool {
    !uuid.is_empty() && uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The directory leaves a field blank far more often than it omits one.
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
#[path = "tests/model_tests.rs"]
mod tests;
