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
    /// The directory's, and rewritten in full on every re-import.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// The user's own, for a directory entry that carries no homepage — roughly
    /// one in fifteen, and nothing can be derived from a stream URL that is
    /// usually a shared host. Its own column so each has exactly one writer:
    /// folded into [`Self::homepage`] it would either be blanked by the next
    /// re-import or block the directory from ever correcting a site that moved.
    /// Read through [`Self::website`], never directly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_homepage: Option<String>,
    /// Where the logo came from, kept so a re-download can be retried after the
    /// stored file is swept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_url: Option<String>,
    /// The user's own logo URL, for the third of the directory that ships none.
    /// [`Self::local_homepage`]'s twin in every respect — read through
    /// [`Self::logo_source`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_favicon_url: Option<String>,
    /// The user's own genre, read through [`Self::genre`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_tags: Option<String>,
    /// The user's own country, read through [`Self::country_name`]. A hand-typed
    /// station has none of its own: a stream announces no country, and guessing
    /// one from the host would be wrong more often than blank is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_country: Option<String>,
    /// The logo in the radio-logo store, which is its own directory precisely
    /// because this is the one artwork column whose file cannot be re-derived
    /// from the user's own library. Still one of the six the sweep reads
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
    /// Segmented stream, played through `player::source::hls`. Stored because a
    /// favorited station has left the directory behind, and because the one
    /// setting still keyed to it filters directory pages by it.
    pub hls: bool,
    pub is_favorite: bool,
    #[serde(default, skip_serializing)]
    pub sort_key: String,
    pub date_added: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    pub play_count: i32,
}

/// A value that is empty rather than absent reads as absent: the directory serves `""` about as
/// readily as it omits a field, and a caller asking "is there one" means the same by both.
fn filled(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

impl RadioStation {
    /// The site this station links out to, whoever supplied it.
    ///
    /// **The user's answer wins**, since it is only ever written where they went and found what
    /// the directory had no entry for. Every surface that draws or opens a station website reads
    /// this rather than either column, so a card and the browser launch behind it cannot disagree.
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

    /// Whether one field is the user's to fill in.
    ///
    /// **The one case that answers `false` is a directory value the user has not overridden.**
    /// That value is not theirs to overwrite and an editor over it is one misclick from replacing
    /// something correct with a typo, which is the whole reason these are gated rather than always
    /// offered.
    ///
    /// The two cases it answers `true` for are both the user's own: a hand-typed station has no
    /// directory behind it to disagree with, and anything they already set has to stay
    /// correctable — closing the field the moment it holds a value would make a typo permanent and
    /// would take back the "leave it blank to remove" the dialog offers.
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

    /// The user's own answer for one field, and never the directory's.
    ///
    /// **Deliberately not [`Self::website`]'s ladder.** That one resolves what a *surface* should
    /// draw; this one answers what the user set, which is what an export writes back. Folding the
    /// directory's value in would spell one station out of both halves and re-import it as an
    /// override of something nobody overrode.
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

    /// This row as the save input that would recreate it, for an export that has to survive as
    /// the station it is rather than as a name and a URL.
    ///
    /// **`station_uuid` is the field the whole projection is for**: it is what separates a station
    /// the directory owns from a hand-typed lookalike, and everything the card gates on that
    /// follows from it. Nothing derived crosses — no id, no `sort_key`, no play stats — and
    /// deliberately **not `artwork_path`**, which names a file in this install's logo store and
    /// means nothing anywhere else; the logo is re-fetched from `favicon_url` wherever the row
    /// lands. The four `local_*` columns stay out too, being [`StationOverrides`]'s to carry.
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

/// Which of the four a caller means.
///
/// An enum rather than four named call sites because the set is *walked* rather than read one at
/// a time: `radio_files` had the four spelled once for its reader and again for its writer, so
/// what its own doc asks for — "a fifth is a row in the table rather than a fifth arm to keep
/// parallel" — was two rows in two tables. One table needs one name per field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideField {
    Website,
    LogoUrl,
    Genre,
    Country,
}

/// The four fields a user may fill in where the directory left a blank.
///
/// A struct rather than four parameters for [`RadioStation`]'s own reason: they are all
/// `Option<String>` and a positional list of those is a bind-order bug waiting to happen. `None`
/// clears the column, which is how a value is removed again.
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

/// What a caller supplies when saving a station.
///
/// The rest of [`RadioStation`] is the table's own: the id, the derived
/// `sort_key`, `date_added`, and the play stats a save must never reset. A
/// struct rather than a dozen parameters because most of them are `String` and
/// a positional list of those is a bind-order bug waiting to happen.
///
/// **`#[serde(default)]` because this shape travels between builds.**
/// `radio_files` writes it into an exported station list, so a file from a build
/// carrying one more column still has to import here, and one written here still
/// has to import there. Unknown fields are already ignored; this is the other
/// direction.
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

/// What an earlier session's attempt at one logo URL left behind.
///
/// Here rather than beside the query that reads it, for the reason every other row type is: it
/// crosses `library::radio`'s public signature and is read in `src/ui/radio`, and a boundary type
/// spelled `queries::radio::…` at those call sites is the UI naming the database layer.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredLogoAnswer {
    pub favicon_url: String,
    /// The stored file, or `None` where the URL answered with nothing.
    pub artwork_path: Option<String>,
    /// When this URL may be asked again. `None` on a hit.
    pub retry_after: Option<String>,
}

/// What the directory's checker writes into [`DirectoryStation::codec`] when it could not identify
/// a stream at all.
///
/// Here rather than beside either reader because both layers name it and neither may name the
/// other: `library::radio` drops the facet, `ui::radio` draws what is left of it under a word a
/// user would recognise, and the two agreeing is what keeps a filter from hiding a bucket the chip
/// still offers.
pub const UNKNOWN_CODEC: &str = "UNKNOWN";

/// A station as the directory describes it, before it is anybody's row.
///
/// Separate from [`RadioStation`] rather than a half-filled one because the two
/// know different things. The directory knows how popular a station is and
/// whether its own last check reached it, none of which the table has a column
/// for; the table's id, stored logo, sort key and play stats mean nothing until
/// the user keeps the station. Keeping the wire shape out of `src/ui/` is the
/// other half of it: a callback names this and never `services::net::radio_browser`.
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
/// segmented filter), so a short page is as often a filtered full one as a genuine
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
/// by `services::net::radio_browser`.
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

/// One station logo that landed: where it is, and what it cost the store.
///
/// The size is read back off the file rather than taken from the bytes handed to `store_image` —
/// that re-encodes anything over its own bounds, so the two differ exactly where the number
/// matters most.
#[derive(Debug, Clone)]
pub struct StoredLogo {
    pub path: String,
    pub bytes: u64,
}
