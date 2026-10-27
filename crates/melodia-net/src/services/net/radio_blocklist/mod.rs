//! Station terms the directory may serve and this build won't show. Read at build time from a
//! source outside the repo; only keyed fingerprints reach the binary ([`source`], `build.rs`).
//!
//! **A build with no source blocks nothing**, which is what a contributor, a fork and every PR
//! check gets, and every predicate short-circuits on the empty list.

pub mod source;

use std::borrow::Cow;

use source::TermKind;

use melodia_core::entities::radio::{DirectoryStation, Facet, FacetKind, NewRadioStation};

// Written by `build.rs`; empty arrays when there is no source.
include!(concat!(env!("OUT_DIR"), "/radio_blocklist_terms.rs"));

pub struct Blocklist {
    key: [u8; 32],
    /// Sorted, for a binary search that allocates nothing.
    terms: Cow<'static, [u64]>,
    /// Sorted too, and apart because [`Blocklist::pattern_hit`] asks a different question.
    patterns: Cow<'static, [u64]>,
    /// Ascending distinct pattern lengths, in characters.
    pattern_lengths: Cow<'static, [u32]>,
}

const BAKED: Blocklist = Blocklist {
    key: BLOCKED_KEY,
    terms: Cow::Borrowed(&BLOCKED_TERMS),
    patterns: Cow::Borrowed(&BLOCKED_PATTERNS),
    pattern_lengths: Cow::Borrowed(&PATTERN_LENGTHS),
};

/// The fields a station is judged on, so a directory row, an imported file and the hand-typed
/// form all meet one predicate.
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
    /// So a test gets a list that doesn't depend on whether its machine has one.
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
            // One split for both tag questions.
            || station.tags.split(',').any(|tag| {
                self.holds(TermKind::Tag, tag) || self.pattern_hit(TermKind::TagContains, tag)
            })
    }

    /// Whether any window of `value` hashes to a blocked pattern. The candidate's windows are
    /// hashed rather than the pattern, so only its fingerprint and length ship.
    fn pattern_hit(&self, kind: TermKind, value: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let folded = source::normalize(value);
        if folded.is_empty() {
            return false;
        }

        // Character boundaries, so a window is a slice of the fold.
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

    /// Countries match on the code, never the name: the code is what a pick filters by, so
    /// matching the name would hide the chip and leave its stations visible.
    fn blocks_facet(&self, kind: FacetKind, facet: &Facet) -> bool {
        if self.is_empty() {
            return false;
        }
        match kind {
            FacetKind::Countries => {
                facet.code.as_deref().is_some_and(|code| self.holds(TermKind::Country, code))
            }
            FacetKind::Languages => self.holds(TermKind::Language, &facet.name),
            FacetKind::Tags => {
                self.holds(TermKind::Tag, &facet.name)
                    || self.pattern_hit(TermKind::TagContains, &facet.name)
            }
            FacetKind::Codecs => self.holds(TermKind::Codec, &facet.name),
        }
    }

    /// A blank value is absent rather than a term, empty terms being refused at parse time.
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
