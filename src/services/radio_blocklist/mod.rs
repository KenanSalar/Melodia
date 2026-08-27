//! Station terms the directory may serve and this build will not show.
//!
//! Curation policy rather than a user setting, so there is no toggle for it and no
//! surface in the UI names it. The terms are read at build time from a source that
//! never enters the repo, hashed under a key from that same source, and only the
//! fingerprints reach the binary — see [`source`] for the format and `build.rs` for
//! where it is read from.
//!
//! **A build with no source blocks nothing**, which is what a contributor, a fork
//! and every PR check gets. Every predicate here short-circuits on the empty term
//! list, so that build pays one branch.
//!
//! Matching is exact on both sides of [`source::normalize`]'s fold, never substring:
//! `Some Station 100.5 FM` is a different term from `Some Station`. That is what
//! lets a fingerprint stand in for the value at all.

pub mod source;

use std::borrow::Cow;

use source::TermKind;

use crate::entities::radio::{DirectoryStation, Facet, FacetKind, NewRadioStation};

// `BLOCKED_KEY: [u8; 32]` and the sorted `BLOCKED_TERMS: [u64; N]`, written by
// `build.rs`. An empty array when there is no source.
include!(concat!(env!("OUT_DIR"), "/radio_blocklist_terms.rs"));

/// A key and the terms hashed under it.
pub struct Blocklist {
    key: [u8; 32],
    /// Sorted, so a lookup is a binary search over what is at most a few hundred
    /// entries and touches no allocation.
    terms: Cow<'static, [u64]>,
}

/// What this build was compiled with.
const BAKED: Blocklist = Blocklist {
    key: BLOCKED_KEY,
    terms: Cow::Borrowed(&BLOCKED_TERMS),
};

/// The fields a station is judged on, borrowed rather than owned.
///
/// A view rather than either concrete station type, so a row arriving from the
/// directory, from an imported file and from the hand-typed form all meet the same
/// predicate. Named fields rather than seven positional `&str`s for the reason
/// [`NewRadioStation`] gives for being a struct.
pub struct StationTerms<'a> {
    /// `None` for a station with no directory entry behind it.
    pub station_uuid: Option<&'a str>,
    pub name: &'a str,
    pub stream_url: &'a str,
    pub country_code: &'a str,
    pub language: &'a str,
    pub codec: &'a str,
    /// The directory's comma-separated spelling, split here rather than by the caller.
    pub tags: &'a str,
}

impl<'a> From<&'a DirectoryStation> for StationTerms<'a> {
    fn from(station: &'a DirectoryStation) -> Self {
        Self {
            station_uuid: Some(&station.station_uuid),
            name: &station.name,
            stream_url: &station.stream_url,
            country_code: &station.country_code,
            language: &station.language,
            codec: &station.codec,
            tags: &station.tags,
        }
    }
}

impl<'a> From<&'a NewRadioStation> for StationTerms<'a> {
    fn from(station: &'a NewRadioStation) -> Self {
        Self {
            station_uuid: station.station_uuid.as_deref(),
            name: &station.name,
            stream_url: &station.stream_url,
            country_code: &station.country_code,
            language: &station.language,
            codec: &station.codec,
            tags: &station.tags,
        }
    }
}

/// Whether this build refuses to show a station.
pub fn blocks<'a>(station: impl Into<StationTerms<'a>>) -> bool {
    BAKED.blocks_station(&station.into())
}

/// Whether this build refuses to offer a facet as a filter chip or a scope pill.
pub fn facet_is_blocked(kind: FacetKind, facet: &Facet) -> bool {
    BAKED.blocks_facet(kind, facet)
}

impl Blocklist {
    /// Build one from a parsed source, which is how a test gets a list that does not
    /// depend on whether the machine running it has one.
    pub fn from_terms(terms: source::Terms) -> Self {
        Self {
            key: terms.key,
            terms: Cow::Owned(terms.fingerprints),
        }
    }

    fn blocks_station(&self, station: &StationTerms<'_>) -> bool {
        if self.terms.is_empty() {
            return false;
        }
        if let Some(uuid) = station.station_uuid
            && self.holds(TermKind::Station, uuid)
        {
            return true;
        }
        self.holds(TermKind::Name, station.name)
            || self.holds(TermKind::Url, station.stream_url)
            || self.holds(TermKind::Country, station.country_code)
            || self.holds(TermKind::Language, station.language)
            || self.holds(TermKind::Codec, station.codec)
            || station.tags.split(',').any(|tag| self.holds(TermKind::Tag, tag))
    }

    /// **Countries match on the code and never on the name**, which is the one axis
    /// where the two differ: `countrycode` is the search endpoint's only code-keyed
    /// parameter, so the code is what a pick actually filters by. Matching the name
    /// here would hide the chip while leaving every station under it visible.
    fn blocks_facet(&self, kind: FacetKind, facet: &Facet) -> bool {
        if self.terms.is_empty() {
            return false;
        }
        match kind {
            FacetKind::Countries => {
                facet.code.as_deref().is_some_and(|code| self.holds(TermKind::Country, code))
            }
            FacetKind::Languages => self.holds(TermKind::Language, &facet.name),
            FacetKind::Tags => self.holds(TermKind::Tag, &facet.name),
            FacetKind::Codecs => self.holds(TermKind::Codec, &facet.name),
        }
    }

    /// A blank value is absent rather than a term: the directory serves `""` for a
    /// field it does not know about as readily as it omits one, and an empty term is
    /// refused at parse time, so hashing one could only ever waste the hash.
    fn holds(&self, kind: TermKind, value: &str) -> bool {
        if value.trim().is_empty() {
            return false;
        }
        self.terms.binary_search(&source::fingerprint(&self.key, kind, value)).is_ok()
    }
}
