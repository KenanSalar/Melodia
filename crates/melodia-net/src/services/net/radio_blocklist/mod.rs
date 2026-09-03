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

use melodia_core::entities::radio::{DirectoryStation, Facet, FacetKind, NewRadioStation};

// `BLOCKED_KEY`, the sorted `BLOCKED_TERMS`, and the substring half —
// `BLOCKED_PATTERNS` with the `PATTERN_LENGTHS` that size their windows. Written by
// `build.rs`; empty arrays when there is no source.
include!(concat!(env!("OUT_DIR"), "/radio_blocklist_terms.rs"));

/// A key and the terms hashed under it.
pub struct Blocklist {
    key: [u8; 32],
    /// Sorted, so a lookup is a binary search over what is at most a few hundred
    /// entries and touches no allocation.
    terms: Cow<'static, [u64]>,
    /// Sorted for the same reason, and separate because they answer a different
    /// question — see [`Blocklist::pattern_hit`].
    patterns: Cow<'static, [u64]>,
    /// Ascending distinct pattern lengths, in characters.
    pattern_lengths: Cow<'static, [u32]>,
}

/// What this build was compiled with.
const BAKED: Blocklist = Blocklist {
    key: BLOCKED_KEY,
    terms: Cow::Borrowed(&BLOCKED_TERMS),
    patterns: Cow::Borrowed(&BLOCKED_PATTERNS),
    pattern_lengths: Cow::Borrowed(&PATTERN_LENGTHS),
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
            patterns: Cow::Owned(terms.patterns),
            pattern_lengths: Cow::Owned(terms.pattern_lengths),
        }
    }

    fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.patterns.is_empty()
    }

    fn blocks_station(&self, station: &StationTerms<'_>) -> bool {
        if self.is_empty() {
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
            || self.pattern_hit(TermKind::NameContains, station.name)
            || self.pattern_hit(TermKind::UrlContains, station.stream_url)
            // One pass over the tags for both questions, since splitting twice would
            // be the more expensive half of asking them.
            || station.tags.split(',').any(|tag| {
                self.holds(TermKind::Tag, tag) || self.pattern_hit(TermKind::TagContains, tag)
            })
    }

    /// Whether any window of `value` hashes to a blocked pattern.
    ///
    /// **The candidate is what gets hashed, not the pattern**, which is the whole
    /// trick: a fingerprint cannot be matched against a substring, so the substring
    /// is reconstructed from the candidate and hashed whole. The pattern text never
    /// ships, only its fingerprint and its length.
    ///
    /// Cost is one hash per window, and the windows are bounded by
    /// [`source::MIN_PATTERN_CHARS`] at the short end and by the candidate's own
    /// length at the long end. Directory values are short — a tag averages a handful
    /// of characters — so this stays a few hashes per value, and it runs on the fetch
    /// task rather than the UI thread.
    fn pattern_hit(&self, kind: TermKind, value: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let folded = source::normalize(value);
        if folded.is_empty() {
            return false;
        }

        // Byte offset of each character plus the end, so a window is a slice of the
        // fold rather than a string rebuilt per position.
        let mut bounds: Vec<usize> = Vec::with_capacity(folded.len() + 1);
        bounds.extend(folded.char_indices().map(|(offset, _)| offset));
        bounds.push(folded.len());
        let characters = bounds.len() - 1;

        for &length in self.pattern_lengths.iter() {
            let length = length as usize;
            // Ascending, so nothing after this one fits either.
            if length > characters {
                break;
            }
            for start in 0..=(characters - length) {
                let window = &folded[bounds[start]..bounds[start + length]];
                let hit = source::fingerprint_normalized(&self.key, kind, window);
                if self.patterns.binary_search(&hit).is_ok() {
                    return true;
                }
            }
        }
        false
    }

    /// **Countries match on the code and never on the name**, which is the one axis
    /// where the two differ: `countrycode` is the search endpoint's only code-keyed
    /// parameter, so the code is what a pick actually filters by. Matching the name
    /// here would hide the chip while leaving every station under it visible.
    fn blocks_facet(&self, kind: FacetKind, facet: &Facet) -> bool {
        if self.is_empty() {
            return false;
        }
        match kind {
            FacetKind::Countries => {
                facet.code.as_deref().is_some_and(|code| self.holds(TermKind::Country, code))
            }
            FacetKind::Languages => self.holds(TermKind::Language, &facet.name),
            // The one facet a pattern reaches, and the reason patterns exist: the tag
            // list carries a spelling per variant, so blocking a family one entry at a
            // time never finishes.
            FacetKind::Tags => {
                self.holds(TermKind::Tag, &facet.name)
                    || self.pattern_hit(TermKind::TagContains, &facet.name)
            }
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

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
