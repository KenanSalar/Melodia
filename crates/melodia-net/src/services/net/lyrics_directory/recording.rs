//! Whether two sets of tags name the same recording, and what to ask the index for.
//!
//! Pure, and the whole of what the client decides for itself. The signature endpoint does its own
//! normalizing server-side and either answers or does not; the index is a keyword search that will
//! hand back a different artist's song of the same name, so **everything it returns has to be
//! checked here before it is believed**.
//!
//! **Two questions, deliberately answered differently.** What to *ask* is loose — a `feat.` credit
//! in a title is noise no uploader agrees on, and a query carrying it finds nothing. What to
//! *accept* is strict, and strict in one direction: a false accept paints another song's words
//! over this one, where a false reject falls back to the signature's own answer and costs only the
//! timings.

use std::collections::BTreeSet;

use melodia_core::utils::fold::fold;

/// The words that make a title a *different recording* rather than the same one dressed
/// differently.
///
/// **The distinction is the whole point.** "Remastered 2011", "Explicit", "Bonus Track" and
/// "NCS Release" are editorial noise on one performance, so they are stripped and ignored; a live
/// take, a remix or an acoustic version is a different performance, whose sung line falls in a
/// different place. Duration catches most of those, and not all: a remix can run the same length
/// as the original.
///
/// Kept to the markers that always mean a re-performance. `mix` is deliberately absent — "Original
/// Mix" is how half an electronic library labels the plain track, and rejecting on it would throw
/// away exactly the matches this file exists to find.
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

/// What separates one credited artist from the next, in a tag or in somebody else's row.
///
/// Spelled once because the query and the check must agree: a splitter that trims the query to the
/// first artist while the check compares the whole credit rejects every row the trimming found.
/// The spaced `x` earns its place on that count — it is how half of one dance library credits a
/// collaboration, and the spaces are what keep it off `Charli XCX`.
const ARTIST_SPLITS: [&str; 9] = [
    " feat ", " feat. ", " ft ", " ft. ", " with ", " & ", " x ", ",", ";",
];

/// One recording, reduced to what two of them can be compared by.
pub(super) struct Recording {
    /// The title with every bracketed aside removed, folded to letters and digits.
    core_title: String,
    /// Which version markers the title carried, whatever brackets they sat in.
    markers: BTreeSet<&'static str>,
    /// Every credited artist, folded and split apart.
    artists: BTreeSet<String>,
}

impl Recording {
    pub(super) fn new(title: &str, artist: &str) -> Self {
        let folded_title = fold(title);
        Self {
            markers: VERSION_MARKERS
                .into_iter()
                .filter(|marker| contains_word(&folded_title, marker))
                .collect(),
            core_title: squeeze(&strip_brackets(&folded_title)),
            // **Split before folding, for the NUL.** The fold turns one into a space so it reads
            // as a separator rather than as a glyph, which is right for a substring filter and
            // wrong here: past it, `J+1<NUL>DrDisrespect` is one credit with a space in it, and
            // a file crediting only the second of them stops matching its own row.
            artists: artist
                .split(['/', '\0'])
                .map(fold)
                .flat_map(|part| split_credits(&part))
                .map(|name| squeeze(&name))
                .filter(|name| !name.is_empty())
                .collect(),
        }
    }

    /// Whether this side is comparable at all: the half of [`Self::matches`] that asks about one
    /// recording rather than about a pair.
    ///
    /// A title that is nothing but a bracketed aside leaves no core title, and a credit that is
    /// nothing but separators leaves no artists. Neither can equal anything, which is why the
    /// caller checks it before spending a request rather than after reading the answer.
    pub(super) fn can_match(&self) -> bool {
        !self.core_title.is_empty() && !self.artists.is_empty()
    }

    /// Whether `other` is this recording under somebody else's tags.
    ///
    /// The title has to be the same words and the same version; the credits have to be one side's
    /// subset of the other's, which is what a shorter credit on one of them means — a row filed
    /// under "Dominic Strike, Euphoria" is this track where the file says only the first of them,
    /// and a file crediting a guest the row omits is the same case mirrored.
    pub(super) fn matches(&self, other: &Self) -> bool {
        if !self.can_match() || !other.can_match() {
            return false;
        }
        if self.core_title != other.core_title || self.markers != other.markers {
            return false;
        }
        let (ours, theirs) = (&self.artists, &other.artists);
        ours.is_subset(theirs) || theirs.is_subset(ours)
    }
}

/// The title to search the index with: the tag's own, less a `feat.` credit.
///
/// **Less that and nothing else.** A version marker stays, so a search for a live take is not
/// answered by the studio cut — and [`Recording::matches`] would reject that answer anyway, which
/// is a request spent to learn nothing. Brackets stay too: the index tolerates them, and dropping
/// them is what would need proving rather than assuming.
pub(super) fn query_title(title: &str) -> &str {
    // **ASCII-folded, so the index it yields is a byte index into `title` as well.** Every marker
    // below is ASCII, while `to_lowercase` is the full Unicode mapping and expands `İ` from two
    // bytes to three: past one of those, the cut lands to the right of where it was found, taking
    // the marker with it and eventually landing inside a character.
    let lower = title.to_ascii_lowercase();
    let cut = [
        " feat ", " feat. ", " ft ", " ft. ", "(feat", "[feat", "(ft", "[ft",
    ]
    .into_iter()
    .filter_map(|mark| lower.find(mark))
    .min();

    match cut {
        Some(at) => title[..at].trim_end(),
        None => title,
    }
}

/// The first credited artist, which is how a row is filed when the file credits several.
pub(super) fn query_artist(artist: &str) -> &str {
    // ASCII-folded for [`query_title`]'s reason, and it bites sooner here: a one-byte marker like
    // `,` leaves nothing for the drift to land harmlessly inside.
    let lower = artist.to_ascii_lowercase();
    let cut =
        ARTIST_SPLITS.into_iter().chain(["/", "\0"]).filter_map(|split| lower.find(split)).min();

    match cut {
        Some(at) => artist[..at].trim_end(),
        None => artist,
    }
}

/// The credits in one folded artist string.
fn split_credits(text: &str) -> Vec<String> {
    let mut parts = vec![text.to_owned()];
    for split in ARTIST_SPLITS {
        parts = parts.iter().flat_map(|part| part.split(split)).map(str::to_owned).collect();
    }
    parts
}

/// Whether `haystack` carries `word` as a word rather than inside a longer one, so "cover" does
/// not fire on "Undercover".
fn contains_word(haystack: &str, word: &str) -> bool {
    haystack.split(|c: char| !c.is_alphanumeric()).any(|part| part == word)
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

/// Letters and digits only, single-spaced — so a hyphen or a doubled space cannot make two
/// spellings of one title unequal.
///
/// **An apostrophe is dropped rather than spaced**, and the two are not interchangeable: spacing
/// it turns `don't` into two words that `dont` can never equal, which is the commonest way one
/// tagger's title differs from another's. Every other mark separates.
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
