//! The directory's JSON as it arrives, and its projection onto the entity types.
//!
//! Every shape here carries a container-level `#[serde(default)]`, because the
//! API adds and removes fields without notice and a station dropped over one
//! nobody reads is a station the user cannot find. Nullability is deliberately
//! not handled: the API does send `null`, but only for fields none of these
//! structs declares, and an undeclared field is already ignored.

use serde::Deserialize;

use melodia_core::entities::radio::{DirectoryStation, Facet};

/// One row of `/json/stations/search`.
///
/// The response carries roughly twice these fields — change stamps, an ISO
/// duplicate of every timestamp, geo coordinates, an extended-info flag. Only
/// what survives into [`DirectoryStation`] is listed.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiStation {
    pub stationuuid: String,
    pub name: String,
    /// The station's own URL, and the fallback where the directory has not
    /// resolved one.
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
    /// Project onto the entity, doing the tidying the directory does not.
    pub fn into_directory_station(self) -> DirectoryStation {
        DirectoryStation {
            station_uuid: self.stationuuid,
            // Names arrive padded often enough to matter: a leading tab sorts
            // ahead of every letter and paints as a gap down the card column.
            name: self.name.trim().to_owned(),
            stream_url: if self.url_resolved.is_empty() {
                self.url
            } else {
                self.url_resolved
            },
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

/// One entry of a facet list.
///
/// The four lists share a shape apart from their code field: `iso_3166_1` for
/// countries, `iso_639` for languages, neither for tags and codecs.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiFacet {
    pub name: String,
    pub stationcount: i64,
    pub iso_3166_1: Option<String>,
    pub iso_639: Option<String>,
}

impl ApiFacet {
    /// Project onto the entity, folding whichever code field this list carries.
    pub fn into_facet(self) -> Facet {
        Facet {
            code: self.iso_3166_1.or(self.iso_639).filter(|code| !code.is_empty()),
            name: self.name,
            station_count: self.stationcount,
        }
    }
}

/// The answer to `/json/vote/{uuid}`.
///
/// The only response here whose success is in the body rather than in the
/// status: an unknown station and a vote the server is rate-limiting both come
/// back `200 {"ok":false}`. `message` carries the server's own wording, which is
/// what a refusal has to say for itself.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ApiVote {
    pub ok: bool,
    pub message: String,
}

/// The distinct hostnames in a mirror list.
///
/// The list carries one entry per address family, so a single mirror appears
/// twice under one name. Picking from it undeduplicated is a coin flip between
/// two spellings of one host, which looks like a working random pick right up
/// until a second mirror exists.
///
/// The `ip` is dropped rather than kept as a fallback: the certificate is issued
/// for the hostname, so an IP-keyed request fails TLS verification. That field
/// is there for clients resolving names themselves, which is the work this
/// whole endpoint exists to avoid.
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

/// A hostname and nothing else, which is the shape `url_for` spells the scheme
/// and the separators around. One carrying either of its own would rewrite the
/// path rather than name a mirror.
fn is_bare_host(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', ':', '?', '#']) && !name.contains(char::is_whitespace)
}

/// Whether a station uuid is safe to interpolate into a request path.
///
/// The two uuid-keyed endpoints take theirs as a path segment rather than as a
/// query parameter, and a uuid does not only ever come from the directory: an
/// imported station list carries whatever `station_uuid` the file named, so a
/// value with a `/` or a `?` in it would reach past the endpoint it was meant
/// for. Shaped like what the directory issues rather than parsed as a UUID,
/// since it is the *path* this has to be safe in and nothing here cares whether
/// the version nibble is plausible.
pub fn is_path_safe_uuid(uuid: &str) -> bool {
    !uuid.is_empty() && uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `None` for a field the directory left blank, which it does far more often
/// than it omits one.
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
#[path = "tests/model_tests.rs"]
mod tests;
