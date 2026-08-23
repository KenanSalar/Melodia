use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A stored radio station.
///
/// Favorites, hand-typed URLs and play history are the same row at different
/// points in its life, so the flags below are what tell them apart rather than
/// the table a row sits in.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct RadioStation {
    pub id: i64,
    /// radio-browser.info's id, or `None` for a station the user typed in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_uuid: Option<String>,
    pub name: String,
    pub stream_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Where the logo came from, kept so a re-download can be retried after the
    /// stored file is swept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_url: Option<String>,
    /// The logo in the radio-logo store, which is its own directory precisely
    /// because this is the one artwork column whose file cannot be re-derived
    /// from the user's own library. Still one of the five the sweep reads
    /// through — the reference set spans every store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_path: Option<String>,
    /// The directory's free-form comma-separated tags, stored verbatim.
    pub tags: String,
    /// The country's full name, which is what a card shows. Stored beside
    /// [`Self::country_code`] rather than derived from it: the directory hands
    /// both over, and deriving would mean shipping an ISO table.
    pub country: String,
    pub country_code: String,
    pub language: String,
    pub codec: String,
    /// Advertised kbps, `0` where the directory does not know. A display hint
    /// only: it is zero on a large share of live stations, so nothing may divide
    /// by it without a fallback.
    pub bitrate: i32,
    /// Segmented stream. Unplayable until Symphonia grows an MPEG-TS demuxer,
    /// and stored because a favorited station has left the directory behind.
    pub hls: bool,
    pub is_favorite: bool,
    #[serde(default, skip_serializing)]
    pub sort_key: String,
    pub date_added: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    pub play_count: i32,
}

/// What a caller supplies when saving a station.
///
/// The rest of [`RadioStation`] is the table's own: the id, the derived
/// `sort_key`, `date_added`, and the play stats a save must never reset. A
/// struct rather than a dozen parameters because most of them are `String` and
/// a positional list of those is a bind-order bug waiting to happen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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

/// What the station editor rewrites.
///
/// A struct rather than seven positional parameters for [`NewRadioStation`]'s reason. Everything
/// but `name` is the stream's own account of itself, so a URL that moved replaces all of it at
/// once: a station repointed at a new mount that kept the old one's logo, homepage or tags is the
/// same wrong `keep_station` already refuses on the directory's side.
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

/// A station as the directory describes it, before it is anybody's row.
///
/// Separate from [`RadioStation`] rather than a half-filled one because the two
/// know different things. The directory knows how popular a station is and
/// whether its own last check reached it, none of which the table has a column
/// for; the table's id, stored logo, sort key and play stats mean nothing until
/// the user keeps the station. Keeping the wire shape out of `src/ui/` is the
/// other half of it: a callback names this and never `services::radio_browser`.
#[derive(Clone, Debug, PartialEq)]
pub struct DirectoryStation {
    pub station_uuid: String,
    pub name: String,
    /// Already followed past any `.pls`/`.m3u` indirection — the directory
    /// resolves its own stations, so this is playable as it stands.
    pub stream_url: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub tags: String,
    /// The country's full name. The code is what filters; this is what a card
    /// shows.
    pub country: String,
    pub country_code: String,
    /// The country subdivision, in the directory's own spelling.
    pub state: String,
    pub language: String,
    pub codec: String,
    /// Advertised kbps, `0` where the directory does not know. The same display
    /// hint as [`RadioStation::bitrate`], and zero on enough live stations that
    /// nothing may divide by it without a fallback.
    pub bitrate: i32,
    pub hls: bool,
    pub votes: i64,
    pub click_count: i64,
    /// Whether the directory's own last reachability check passed.
    pub last_check_ok: bool,
}

impl DirectoryStation {
    /// Whether the station is worth handing on: it can be played, and it can be
    /// told apart from every other row once it is one.
    ///
    /// Both fields default to empty under the wire structs' `#[serde(default)]`,
    /// and the uuid is the worse half. [`Self::to_new_station`] passes it as
    /// `Some("")`, which the `UNIQUE` column reads as a value rather than a
    /// gap, so every station missing one would upsert onto a single row.
    pub fn is_usable(&self) -> bool {
        !self.station_uuid.is_empty() && !self.stream_url.is_empty()
    }

    /// Project onto the save input, dropping the popularity figures the table
    /// does not keep.
    ///
    /// The uuid crosses unconditionally because anything reaching a caller
    /// passed [`Self::is_usable`]; the `None` case is a hand-typed station,
    /// which never comes through here.
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

/// One page of directory results, and whether asking again would return more.
///
/// The two travel together because `has_more` cannot be recovered from
/// `stations` once the page has been filtered: rows are dropped on the way out
/// of the client ([`DirectoryStation::is_usable`]) and again at the facade (the
/// HLS setting), so a short page is as often a filtered full one as a genuine
/// end. Read off the kept length, paging stops on the first page a filter
/// thinned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationPage {
    pub stations: Vec<DirectoryStation>,
    pub has_more: bool,
}

/// One entry of a directory facet list, and how many stations carry it.
#[derive(Clone, Debug, PartialEq)]
pub struct Facet {
    pub name: String,
    /// The ISO code where the list has one, which only the countries list can
    /// actually filter by: `countrycode` is the search endpoint's sole
    /// code-keyed parameter. Languages carry an `iso_639` and still filter by
    /// `name`, the way tags and codecs do with no code at all.
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

/// A directory query, filled in by the caller and turned into request parameters
/// by `services::radio_browser`.
///
/// `Default` is the first screen with nothing typed: most-clicked first, one
/// page, no filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StationSearch {
    /// Substring match on the station name. Empty is no name filter.
    pub name: String,
    pub country_code: String,
    /// The language's *name*, not its code, which is where this parts company
    /// with `country_code` above: the search endpoint has no code-keyed
    /// language parameter to pair a [`Facet::code`] with.
    pub language: String,
    /// Every tag has to match, not any.
    pub tags: Vec<String>,
    pub codec: String,
    /// Advertised kbps floor. `0` is no floor, which is not the same as asking
    /// for zero: a large share of live stations advertise exactly that.
    pub bitrate_min: u32,
    /// Direction is the order's own, so there is no flag for it here.
    pub order: SearchOrder,
    pub offset: u32,
    /// `0` takes the client's page size. Never sent absent — the API's own
    /// default is the entire directory.
    pub limit: u32,
}
