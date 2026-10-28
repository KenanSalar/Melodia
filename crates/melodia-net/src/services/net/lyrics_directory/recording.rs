//! Whether two sets of tags name the same recording, and what to ask the index for.
//!
//! Pure, and the whole of what the client decides for itself: the index will hand back another
//! artist's song of the same name, so **everything it returns is checked here**. What to *ask* is
//! loose, a `feat.` credit in a title being noise no uploader agrees on. What to *accept* is
//! strict, because a false accept paints another song's words over this one where a false reject
//! only costs the timings.

use std::collections::{BTreeMap, BTreeSet};

use melodia_core::entities::artist::ArtistCredit;
use melodia_core::utils::fold::fold;

/// Words that make a title a *different performance*, where "Remastered 2011" or "Explicit" is
/// the same one dressed differently. Duration catches most of these, but not a remix as long as
/// the original. `mix` is absent on purpose: "Original Mix" is how much of an electronic library
/// labels the plain track.
const VERSION_MARKERS: [&str; 9] = [
    "live",
    "acoustic",
    "unplugged",
    "instrumental",
    "karaoke",
    "demo",
    "remix",
    "cover",
    "reprise",
];

/// Separators between credited artists in an unstructured string, one list so the query and the
/// check agree. The spaced `x` is a common collaboration spelling, the spaces keeping it off
/// `Charli XCX`. **Not to be extended to `entities::artist::JOIN_PHRASES`**: a `" and "` would
/// cut `Florence and the Machine` down to `Florence`.
const ARTIST_SPLITS: [&str; 9] =
    [" feat ", " feat. ", " ft ", " ft. ", " with ", " & ", " x ", ",", ";"];

/// One recording, reduced to what two of them can be compared by.
pub(super) struct Recording {
    /// The title with every bracketed aside removed, folded to letters and digits.
    core_title: String,
    /// How often each version marker occurs, in brackets or not. A count rather than a set because
    /// a marker is also a word: a title containing `live` and its live take share a core.
    markers: BTreeMap<&'static str, usize>,
    artists: BTreeSet<String>,
    /// The names our own tags list, empty for every row. Apart from `artists` because the row side
    /// is always one split string, so a name the split would cut matches nothing there.
    named: BTreeSet<String>,
}

impl Recording {
    pub(super) fn new(title: &str, artist: &str) -> Self {
        let folded_title = fold(title);
        Self {
            markers: VERSION_MARKERS
                .into_iter()
                .map(|marker| (marker, word_count(&folded_title, marker)))
                .filter(|(_, count)| *count > 0)
                .collect(),
            core_title: squeeze(&strip_brackets(&folded_title)),
            artists: credits(artist),
            named: BTreeSet::new(),
        }
    }

    /// Our side, from the credit. The listed names sit beside the split rather than in it, so
    /// `Simon & Garfunkel` still matches a row split the same way, and a `" vs. "` credit nobody
    /// is filed under reaches its names.
    pub(super) fn from_credit(title: &str, credit: &ArtistCredit) -> Self {
        let mut recording = Self::new(title, credit.line().unwrap_or_default());
        recording.named = credit
            .artists()
            .iter()
            .map(|credited| squeeze(&fold(&credited.name)))
            .filter(|name| !name.is_empty())
            .collect();
        recording
    }

    /// Whether this side is comparable at all, asked before spending a request: a title that is
    /// all bracket, or a credit that is all separators, can equal nothing.
    pub(super) fn can_match(&self) -> bool {
        !self.core_title.is_empty() && (!self.artists.is_empty() || !self.named.is_empty())
    }

    /// The same words, the same version, and credits agreeing under either reading of ours.
    pub(super) fn matches(&self, other: &Self) -> bool {
        if !self.can_match() || !other.can_match() {
            return false;
        }
        if self.core_title != other.core_title || self.markers != other.markers {
            return false;
        }
        credits_agree(&self.artists, &other.artists) || credits_agree(&self.named, &other.artists)
    }
}

/// Either side a subset of the other, which is what a shorter credit on one of them means. An
/// empty left side is refused, `named` being empty for every row and the subset test trivially
/// true for it.
fn credits_agree(ours: &BTreeSet<String>, theirs: &BTreeSet<String>) -> bool {
    !ours.is_empty() && (ours.is_subset(theirs) || theirs.is_subset(ours))
}

/// The tag's title less a `feat.` credit and nothing else. A version marker stays, or a search for
/// a live take is answered by the studio cut that [`Recording::matches`] then rejects.
pub(super) fn query_title(title: &str) -> &str {
    // ASCII-folded so an index into it is a byte index into `title`: `to_lowercase` grows `İ` by
    // a byte and would push the cut past the marker.
    let lower = title.to_ascii_lowercase();
    let cut = [" feat ", " feat. ", " ft ", " ft. ", "(feat", "[feat", "(ft", "[ft"]
        .into_iter()
        .filter_map(|mark| lower.find(mark))
        .min();

    match cut {
        Some(at) => title[..at].trim_end(),
        None => title,
    }
}

/// A credit listing several artists already says who leads. A single one may still be several
/// run together, which [`query_artist`] guesses at.
pub(super) fn search_artist(credit: &ArtistCredit) -> &str {
    if credit.artists().len() > 1 {
        credit.primary_name()
    } else {
        query_artist(credit.line().unwrap_or_default())
    }
}

/// The first credited artist, which is how a row is filed when the file credits several.
fn query_artist(artist: &str) -> &str {
    // ASCII-folded for `query_title`'s reason.
    let lower = artist.to_ascii_lowercase();
    let cut =
        ARTIST_SPLITS.into_iter().chain(["/", "\0"]).filter_map(|split| lower.find(split)).min();

    match cut {
        Some(at) => artist[..at].trim_end(),
        None => artist,
    }
}

/// Everyone credited on one artist string. Split before folding, for the NUL: the fold makes it a
/// space, and past that `A<NUL>B` is one credit with a space in it. A loop because the split
/// borrows from the folded string.
fn credits(artist: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for part in artist.split(['/', '\0']) {
        let folded = fold(part);
        for credit in split_credits(&folded) {
            let name = squeeze(credit);
            if !name.is_empty() {
                names.insert(name);
            }
        }
    }
    names
}

/// Borrowed through every round; the caller owns what it keeps.
fn split_credits(text: &str) -> Vec<&str> {
    let mut parts = vec![text];
    let mut next = Vec::new();
    for split in ARTIST_SPLITS {
        next.clear();
        next.extend(parts.iter().flat_map(|part| part.split(split)));
        std::mem::swap(&mut parts, &mut next);
    }
    parts
}

/// As a whole word, so "cover" doesn't fire on "Undercover".
fn word_count(haystack: &str, word: &str) -> usize {
    haystack.split(|c: char| !c.is_alphanumeric()).filter(|part| *part == word).count()
}

/// Everything outside `()`, `[]` and `{}`. Nesting is counted rather than matched, an unclosed
/// bracket in a tag being far likelier than a nested one.
fn strip_brackets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0_usize;
    for ch in text.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Letters and digits, single-spaced, so punctuation can't make two spellings of a title unequal.
/// An apostrophe is dropped rather than spaced: spaced, `don't` could never equal `dont`.
fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !matches!(ch, '\'' | '\u{2019}' | '`' | '\u{02bc}') && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
#[path = "tests/recording_tests.rs"]
mod tests;
