use std::borrow::Cow;

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A stored station. Favorites, hand-typed URLs and history are one row at different points in
/// its life, told apart by its flags.
#[derive(Clone, Debug, PartialEq, FromRow)]
pub struct RadioStation {
    pub id: i64,
    /// radio-browser.info's id, or `None` for a station the user typed in.
    pub station_uuid: Option<String>,
    pub name: String,
    pub stream_url: String,
    /// The directory's, and rewritten in full on every re-import.
    pub homepage: Option<String>,
    /// The user's own, for a directory entry with none, in its own column so a re-import can't
    /// revert it. Read through [`Self::website`].
    pub local_homepage: Option<String>,
    /// Kept so a re-download can be retried after the stored file is swept.
    pub favicon_url: Option<String>,
    /// [`Self::local_homepage`]'s twin, read through [`Self::logo_source`].
    pub local_favicon_url: Option<String>,
    /// Read through [`Self::genre`].
    pub local_tags: Option<String>,
    /// Read through [`Self::country_name`]. A hand-typed station starts with none, a guess from
    /// the host being wrong more often than blank is.
    pub local_country: Option<String>,
    /// In the radio-logo store, apart because this file can't be re-derived, and still one of the
    /// sweep's six columns.
    pub artwork_path: Option<String>,
    /// The directory's free-form comma-separated tags, stored verbatim.
    pub tags: String,
    /// The full name a card shows, stored rather than derived so no ISO table ships.
    pub country: String,
    pub country_code: String,
    pub language: String,
    /// The directory's, naming the container: every Ogg mount is `OGG`.
    pub codec: String,
    /// What the stream held when it first played, `None` until then. Its own column so a re-import
    /// rewriting [`Self::codec`] can't take it; read through [`Self::format`].
    pub probed_codec: Option<String>,
    /// Advertised kbps, `0` on a large share of live stations, so never a divisor without a
    /// fallback.
    pub bitrate: i32,
    /// Stored because a favorite has left the directory behind, and a setting filters pages by it.
    pub hls: bool,
    pub is_favorite: bool,
    pub sort_key: String,
    pub date_added: String,
    pub last_played: Option<String>,
    pub play_count: i32,
}

/// Empty reads as absent, the directory serving `""` as readily as it omits a field. Not
/// [`crate::utils::text::filled`], which trims: [`RadioStation::can_override`] offers an editor
/// only where the directory said nothing, and a run of spaces is something.
fn filled(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// A slash: a card's meta line joins fields with ` · `, so a dot would read as a fourth field and
/// a space as one two-word name.
const FORMAT_SEPARATOR: &str = "/";

/// A station's format from what the directory filed it under and what an open measured, both kept
/// where they differ, the directory's first: it names the container, which is what its filter
/// matches, so `OGG` beside `VORBIS` says they are one station. Where one answer implies the other
/// (`UNKNOWN`, a blank, one containing the other) it stands alone, borrowed.
pub fn compose_format<'a>(directory: &'a str, probed: Option<&'a str>) -> Option<Cow<'a, str>> {
    let (named, measured) = (filled(Some(directory)), filled(probed));
    let (Some(directory), Some(probed)) = (named, measured) else {
        return named.or(measured).map(Cow::Borrowed);
    };
    if directory.eq_ignore_ascii_case(UNKNOWN_CODEC) {
        return Some(Cow::Borrowed(probed));
    }
    if contains_ignore_ascii_case(directory, probed) {
        return Some(Cow::Borrowed(directory));
    }
    if contains_ignore_ascii_case(probed, directory) {
        return Some(Cow::Borrowed(probed));
    }
    Some(Cow::Owned(format!("{directory}{FORMAT_SEPARATOR}{probed}")))
}

/// [`compose_format`] over a value already carrying a measurement, as a Now Playing surface holds.
/// Composing it a second time would name both codecs where a station that changed what it serves
/// holds one.
pub fn recompose_format<'a>(current: &'a str, probed: &'a str) -> Option<Cow<'a, str>> {
    let directory = current.split_once(FORMAT_SEPARATOR).map_or(current, |(named, _)| named);
    compose_format(directory, Some(probed))
}

/// An empty `needle` is refused, `windows(0)` panicking.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

impl RadioStation {
    /// The user's answer wins, written only where the directory had none. Every surface drawing or
    /// opening a station's site reads this, so a card and its launch agree.
    pub fn website(&self) -> Option<&str> {
        filled(self.local_homepage.as_deref()).or_else(|| filled(self.homepage.as_deref()))
    }

    /// Where this station's logo should be fetched from. [`Self::website`]'s rule.
    pub fn logo_source(&self) -> Option<&str> {
        filled(self.local_favicon_url.as_deref()).or_else(|| filled(self.favicon_url.as_deref()))
    }

    /// The genre a card shows. [`Self::website`]'s rule.
    pub fn genre(&self) -> Option<&str> {
        filled(self.local_tags.as_deref()).or_else(|| filled(Some(self.tags.as_str())))
    }

    /// The country a card shows. [`Self::website`]'s rule.
    pub fn country_name(&self) -> Option<&str> {
        filled(self.local_country.as_deref()).or_else(|| filled(Some(self.country.as_str())))
    }

    /// Unlike [`Self::website`]'s rule, keeps both answers where they differ ([`compose_format`]).
    pub fn format(&self) -> Option<Cow<'_, str>> {
        compose_format(&self.codec, self.probed_codec.as_deref())
    }

    /// `false` only over a directory value the user hasn't overridden, where an editor is one
    /// misclick from replacing something correct with a typo. A hand-typed station and a field
    /// already set stay open, or a typo would be permanent.
    fn can_override(&self, from_directory: Option<&str>, local: Option<&str>) -> bool {
        self.station_uuid.is_none() || filled(from_directory).is_none() || local.is_some()
    }

    pub fn can_set_website(&self) -> bool {
        self.can_override(self.homepage.as_deref(), self.local_homepage.as_deref())
    }

    pub fn can_set_logo(&self) -> bool {
        self.can_override(self.favicon_url.as_deref(), self.local_favicon_url.as_deref())
    }

    pub fn can_set_genre(&self) -> bool {
        self.can_override(Some(self.tags.as_str()), self.local_tags.as_deref())
    }

    pub fn can_set_country(&self) -> bool {
        self.can_override(Some(self.country.as_str()), self.local_country.as_deref())
    }

    /// What the user set, never the directory's, for an export: folding the directory's value in
    /// would re-import as an override nobody made.
    pub fn local_override(&self, field: OverrideField) -> Option<&str> {
        match field {
            OverrideField::Website => self.local_homepage.as_deref(),
            OverrideField::LogoUrl => self.local_favicon_url.as_deref(),
            OverrideField::Genre => self.local_tags.as_deref(),
            OverrideField::Country => self.local_country.as_deref(),
        }
    }

    /// Whether the card offers its pencil at all: it does while any one field is still the user's
    /// to fill, and goes away once the directory has answered for all of them.
    pub fn is_editable(&self) -> bool {
        self.can_set_website()
            || self.can_set_logo()
            || self.can_set_genre()
            || self.can_set_country()
    }

    /// Whether either local tab would still show this station. **A hand-typed one is listed by its
    /// favorite alone**: its card offers no heart to set (`can-favorite: station.uuid != ""`), so
    /// kept for its play stamp it could never get back into Favorites.
    pub fn is_listed(&self) -> bool {
        if self.station_uuid.as_deref().is_none_or(str::is_empty) {
            return self.is_favorite;
        }
        self.is_favorite || self.last_played.is_some()
    }

    /// The save input that recreates this row, for an export. `station_uuid` is the point, telling
    /// a directory station from a hand-typed lookalike. Nothing derived crosses, nor `artwork_path`
    /// (a file in this install's store), nor the `local_*` columns, which [`StationOverrides`]
    /// carries.
    pub fn to_new_station(&self) -> NewRadioStation {
        NewRadioStation {
            station_uuid: self.station_uuid.clone(),
            name: self.name.clone(),
            stream_url: self.stream_url.clone(),
            homepage: self.homepage.clone(),
            favicon_url: self.favicon_url.clone(),
            tags: self.tags.clone(),
            country: self.country.clone(),
            country_code: self.country_code.clone(),
            language: self.language.clone(),
            codec: self.codec.clone(),
            bitrate: self.bitrate,
            hls: self.hls,
        }
    }
}

/// The kept tab a removal was asked on, which decides what it takes: the favorite, or the plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeptList {
    Favorites,
    Recent,
}

/// What removing one station from a kept tab took, enough to reverse exactly that.
#[derive(Debug, PartialEq)]
pub enum StationCut {
    /// The favorite, the row staying for its plays.
    Unfavorited { id: i64, name: String },
    /// The plays, the row staying for its favorite.
    HistoryCleared { id: i64, name: String, last_played: String, play_count: i32 },
    /// The row as it stood before the removal, which left nothing listing it.
    Deleted(Box<RadioStation>),
}

impl StationCut {
    pub fn name(&self) -> &str {
        match self {
            Self::Unfavorited { name, .. } | Self::HistoryCleared { name, .. } => name,
            Self::Deleted(station) => &station.name,
        }
    }
}

/// An enum because `radio_files` walks the set, so one table names each field once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideField {
    Website,
    LogoUrl,
    Genre,
    Country,
}

/// The four fields a user may fill where the directory left a blank, a struct so four
/// `Option<String>`s can't swap places. `None` clears the column.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationOverrides {
    pub website: Option<String>,
    pub logo_url: Option<String>,
    pub genre: Option<String>,
    pub country: Option<String>,
}

impl StationOverrides {
    /// The slot one field reads into, for a caller filling them from a table.
    pub fn slot_mut(&mut self, field: OverrideField) -> &mut Option<String> {
        match field {
            OverrideField::Website => &mut self.website,
            OverrideField::LogoUrl => &mut self.logo_url,
            OverrideField::Genre => &mut self.genre,
            OverrideField::Country => &mut self.country,
        }
    }
}

/// What a save supplies; the id, `sort_key`, `date_added` and play stats are the table's own.
/// `#[serde(default)]` because an exported station list travels between builds both ways.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewRadioStation {
    pub station_uuid: Option<String>,
    pub name: String,
    pub stream_url: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub tags: String,
    pub country: String,
    pub country_code: String,
    pub language: String,
    pub codec: String,
    pub bitrate: i32,
    pub hls: bool,
}

/// What the station editor rewrites. Everything but `name` is the stream's account of itself, so a
/// moved URL replaces all of it rather than keeping the old mount's logo or tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationEdit {
    pub name: String,
    pub stream_url: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub tags: String,
    pub codec: String,
    pub bitrate: i32,
}

/// What an earlier session's attempt at one logo URL left behind.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredLogoAnswer {
    pub favicon_url: String,
    /// The stored file, or `None` where the URL answered with nothing.
    pub artwork_path: Option<String>,
    /// When this URL may be asked again. `None` on a hit.
    pub retry_after: Option<String>,
}

/// What the directory's checker writes for a stream it couldn't identify. Shared because
/// `library::radio` drops the facet and `ui::radio` draws it, and the two agreeing keeps a filter
/// from hiding a bucket the chip still offers.
pub const UNKNOWN_CODEC: &str = "UNKNOWN";

/// A station as the directory describes it, before it is anybody's row: it knows popularity and
/// reachability the table has no column for, and the table's id and stats mean nothing until a
/// station is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectoryStation {
    pub station_uuid: String,
    pub name: String,
    /// Already resolved past any `.pls`/`.m3u`, so playable as it stands.
    pub stream_url: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub tags: String,
    /// The full name; the code is what filters.
    pub country: String,
    pub country_code: String,
    /// The country subdivision, in the directory's own spelling.
    pub state: String,
    pub language: String,
    pub codec: String,
    /// As [`RadioStation::bitrate`], never a divisor.
    pub bitrate: i32,
    pub hls: bool,
    pub votes: i64,
    pub click_count: i64,
    /// Whether the directory's own last reachability check passed.
    pub last_check_ok: bool,
}

impl DirectoryStation {
    /// Playable, and distinguishable once it is a row: an empty uuid crosses as `Some("")`, which
    /// the `UNIQUE` column takes as a value, so every station missing one would upsert onto one row.
    pub fn is_usable(&self) -> bool {
        !self.station_uuid.is_empty() && !self.stream_url.is_empty()
    }

    /// Drops the popularity figures. The uuid crosses unconditionally, anything here having passed
    /// [`Self::is_usable`].
    pub fn to_new_station(&self) -> NewRadioStation {
        NewRadioStation {
            station_uuid: Some(self.station_uuid.clone()),
            name: self.name.clone(),
            stream_url: self.stream_url.clone(),
            homepage: self.homepage.clone(),
            favicon_url: self.favicon_url.clone(),
            tags: self.tags.clone(),
            country: self.country.clone(),
            country_code: self.country_code.clone(),
            language: self.language.clone(),
            codec: self.codec.clone(),
            bitrate: self.bitrate,
            hls: self.hls,
        }
    }
}

/// `has_more` travels with the page because rows are filtered on the way out, so a short page is
/// as often a thinned full one as the end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationPage {
    pub stations: Vec<DirectoryStation>,
    pub has_more: bool,
}

/// One entry of a directory facet list, and how many stations carry it.
#[derive(Clone, Debug, PartialEq)]
pub struct Facet {
    pub name: String,
    /// The ISO code where the list has one. Only countries filter by it, `countrycode` being the
    /// search endpoint's sole code-keyed parameter.
    pub code: Option<String>,
    pub station_count: i64,
}

/// Which facet list to fetch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FacetKind {
    Countries,
    Languages,
    Tags,
    Codecs,
}

/// How to order a directory search.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchOrder {
    Name,
    Votes,
    /// The directory's own answer to what people actually listen to, and what an
    /// empty query sorts by.
    #[default]
    ClickCount,
    Bitrate,
    Random,
}

/// Stations per page, and the fallback for a zero [`StationSearch::limit`], the API's own default
/// being the whole directory. Shared with the pager, whose offset advances by exactly the limit a
/// request carried, which `melodia-views` can't read from `melodia-net`.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// `Default` is the first screen with nothing typed: most-clicked first, one page, no filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationSearch {
    /// Substring match on the station name. Empty is no name filter.
    pub name: String,
    pub country_code: String,
    /// The name, not a code: the endpoint has no code-keyed language parameter.
    pub language: String,
    /// Every tag has to match, not any.
    pub tags: Vec<String>,
    pub codec: String,
    /// `0` is no floor, not a request for the many stations advertising zero.
    pub bitrate_min: u32,
    /// Direction is the order's own, so there is no flag for it here.
    pub order: SearchOrder,
    pub offset: u32,
    /// `0` takes the page size; never sent absent.
    pub limit: u32,
}

/// The size is read off the file, `store_image` re-encoding anything over its bounds.
#[derive(Debug, Clone)]
pub struct StoredLogo {
    pub path: String,
    pub bytes: u64,
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
