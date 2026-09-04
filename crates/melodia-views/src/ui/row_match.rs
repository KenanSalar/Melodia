//! What a row is searchable by, and the fold every filter box runs.
//!
//! Every per-view `SearchBar` narrows an in-memory cache rather than re-querying, so none
//! goes near `tracks_fts` — a second answer to "what does this query match", and this
//! module is what keeps it the same answer. [`search_fields`] is the one definition of a
//! row's searchable text, and [`fold_needle`] / [`push_folded`] fold case *and* accents,
//! covering everything the `unicode61 remove_diacritics 2` tokenizer does. Years are the
//! one deliberate divergence: [`Needle::matches_number`] is a substring where FTS prefixes.
//!
//! **A needle must be folded exactly once, by its owner, and [`Needle`] is what makes that
//! a type rather than a comment.** The failure is silent in the worst way: the row side is
//! always folded, so an unfolded needle still matches, just without accent parity, on that
//! one surface. Carrying the needle also answers its *shape* once per walk rather than per
//! row — all-ASCII, and all-ASCII-digits — neither of which can change between rows.

use std::cell::RefCell;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use melodia_core::entities::track::{MostPlayedFavorite, TrackListRow};

/// The searchable text of a track row, in the order `tracks_fts` lists its columns. `year`
/// joins through [`any_field_matches`], being an integer.
///
/// `composer` and `file_name` are indexed by FTS and deliberately not carried here:
/// `TrackListRow` has no composer column, and a filename echoes the tags beside it, which
/// an unranked substring filter has no way to de-prioritize as bm25 does.
pub fn search_fields(r: &TrackListRow) -> [&str; 5] {
    [
        &r.title,
        r.artist.as_deref().unwrap_or_default(),
        r.album_artist.as_deref().unwrap_or_default(),
        r.album.as_deref().unwrap_or_default(),
        r.genre.as_deref().unwrap_or_default(),
    ]
}

/// [`search_fields`] for the Most Played card projection — same five, same order.
fn most_played_fields(t: &MostPlayedFavorite) -> [&str; 5] {
    [
        &t.title,
        t.artist.as_deref().unwrap_or_default(),
        t.album_artist.as_deref().unwrap_or_default(),
        t.album.as_deref().unwrap_or_default(),
        t.genre.as_deref().unwrap_or_default(),
    ]
}

/// Any field, or the year's decimal form, containing `needle`.
fn any_field_matches(fields: &[&str; 5], year: Option<i32>, needle: &Needle) -> bool {
    fields.iter().any(|f| needle.contains(f)) || needle.matches_number(year)
}

/// Folded substring match across a track row's five text fields and its year. An empty
/// needle matches every row, so an unfiltered list needs no branch.
pub fn track_matches(r: &TrackListRow, needle: &Needle) -> bool {
    any_field_matches(&search_fields(r), r.year, needle)
}

/// [`track_matches`] over a Most Played card. The card renders only title and artist, but
/// on Recently Played it sits directly above a track list fed by the same search bar, so a
/// narrower set would let one query show a genre's tracks in the list and none of its
/// cards above them.
pub fn most_played_matches(t: &MostPlayedFavorite, needle: &Needle) -> bool {
    any_field_matches(&most_played_fields(t), t.year, needle)
}

/// Widest `i32` in decimal: ten digits plus a sign.
const MAX_I32_DIGITS: usize = 11;

/// Write `value` into `buf` back-to-front and hand back the digits. Years are printed once
/// per row per digit-shaped keystroke, so this stays off the heap for the same reason
/// `player::playback::waveform::push_fixed` does. Both fallbacks are unreachable, and exist because
/// the alternatives are a silent `as` truncation and an `unwrap`.
fn write_decimal(buf: &mut [u8; MAX_I32_DIGITS], value: i32) -> &str {
    // `unsigned_abs` rather than `abs` — `-i32::MIN` overflows.
    let mut rest = value.unsigned_abs();
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + u8::try_from(rest % 10).unwrap_or(0);
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if value < 0 {
        at -= 1;
        buf[at] = b'-';
    }
    std::str::from_utf8(&buf[at..]).unwrap_or("")
}

/// A filter needle, folded once by [`fold_needle`] and carrying the two answers about its
/// own shape that every predicate below would otherwise re-derive per field, per row.
/// Construction is the only way in, which makes "folded exactly once by its owner"
/// enforceable rather than advisory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needle {
    /// Private: handing out a `&str` is how the unfolded spelling gets back in.
    text: String,
    /// What lets the substring check take the allocation-free byte path. Asked before the
    /// haystack, so a non-ASCII query short-circuits that scan.
    ascii: bool,
    /// A non-empty run of ASCII digits — the only shape that can name one of the integer
    /// fields, and so the gate keeping an ordinary text query off both number predicates.
    digits: bool,
}

/// Hand-written rather than derived, so `Needle::default()` and `fold_needle("")` are the
/// *same* needle. A derived `Default` leaves `ascii` false, a state [`fold_needle`] can
/// never produce, and the two would compare unequal while behaving identically.
impl Default for Needle {
    fn default() -> Self {
        fold_needle("")
    }
}

impl Needle {
    /// For the one caller matching against an already-folded haystack of its own: the
    /// packed `RowSearchKey` behind both cached track lists.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Reset to the empty needle — the four detail views' fresh-open, where the incoming
    /// entity inherits no filter from the one before it.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Case- and accent-insensitive substring check, skipping the fold entirely on
    /// all-ASCII text — where the filter walk spends nearly all its time, a large genre
    /// detail running this over thousands of rows per throttled keystroke. An empty needle
    /// matches anything, which is what lets a filter walk run unconditionally.
    pub fn contains(&self, haystack: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        // An ASCII haystack carries no accent to fold, so the byte walk is the whole
        // answer rather than an approximation.
        if self.ascii && haystack.is_ascii() {
            let h = haystack.as_bytes();
            let n = self.text.as_bytes();
            if n.len() > h.len() {
                return false;
            }
            return h
                .windows(n.len())
                .any(|w| w.iter().zip(n).all(|(a, b)| fold_ascii_byte(*a) == *b));
        }
        with_folded(haystack, |folded| folded.contains(&self.text))
    }

    /// Case- and accent-insensitive equality, on the same fold as [`Self::contains`].
    /// Unlike its sibling, an empty needle matches only an empty `haystack`.
    pub fn equals(&self, haystack: &str) -> bool {
        // Folding an ASCII string is byte-for-byte, so a length mismatch settles it. It
        // settles nothing in general: NFD decomposition changes length.
        if self.ascii && haystack.is_ascii() {
            let n = self.text.as_bytes();
            return haystack.len() == n.len()
                && haystack.bytes().zip(n).all(|(a, b)| fold_ascii_byte(a) == *b);
        }
        with_folded(haystack, |folded| folded == self.text)
    }

    /// Case- and accent-insensitive prefix check, on the same fold as [`Self::contains`].
    /// An empty needle matches anything, as there.
    pub fn starts_with(&self, haystack: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        if self.ascii && haystack.is_ascii() {
            let n = self.text.as_bytes();
            return haystack.len() >= n.len()
                && haystack.bytes().zip(n).all(|(a, b)| fold_ascii_byte(a) == *b);
        }
        with_folded(haystack, |folded| folded.starts_with(&self.text))
    }

    /// Substring match on an integer field in decimal, without formatting it onto the
    /// heap. Substring rather than the FTS side's prefix: every other field in these boxes
    /// matches mid-word (`hap` finds "Rhapsody"), so a prefix year is the odd one out.
    ///
    /// `None` is a row that has no such number and matches nothing — which is what lets a
    /// caller whose sentinel is `0` rather than absence hand over that decision here.
    ///
    /// Wrong for a field drawn from a short set of values — see [`Self::equals_number`].
    pub fn matches_number(&self, value: Option<i32>) -> bool {
        self.over_decimal(value, |decimal| decimal.contains(&self.text))
    }

    /// [`Self::matches_number`] on the whole number rather than a run inside it, for a
    /// field drawn from a short ladder of values that share digits. The substring rule
    /// that picks a decade out of the years picks most of a bitrate column here.
    pub fn equals_number(&self, value: Option<i32>) -> bool {
        self.over_decimal(value, |decimal| decimal == self.text)
    }

    /// The digit gate and the stack buffer both number predicates run on.
    fn over_decimal(&self, value: Option<i32>, matches: impl FnOnce(&str) -> bool) -> bool {
        if !self.digits {
            return false;
        }
        let Some(value) = value else {
            return false;
        };
        let mut buf = [0u8; MAX_I32_DIGITS];
        matches(write_decimal(&mut buf, value))
    }
}

/// The ASCII half of [`push_folded`]'s rule, so the byte walk above and the packed key
/// below can't answer a NUL differently.
const fn fold_ascii_byte(b: u8) -> u8 {
    if b == 0 { b' ' } else { b.to_ascii_lowercase() }
}

/// Append `s` to `out`, case- and accent-folded: NFD, combining marks dropped, lowercased.
/// Covers everything `tracks_fts`'s `unicode61 remove_diacritics 2` tokenizer folds — mode
/// 2 rather than `SQLite`'s legacy 1 is what reaches the two-mark characters of scripts
/// like Vietnamese.
///
/// Deliberately the *looser* of the two: `is_combining_mark` is `General_Category=Mark`, so
/// this also drops Indic spacing marks and the kana voicing marks where `SQLite`'s table is
/// Latin-scoped. In a substring filter that only widens a match, and under-folding is the
/// half that would show.
///
/// An embedded NUL becomes a space — not only for the packed `\0`-separated key, but
/// because ID3v2.4 joins a multi-value text frame that way. A needle typed into a text
/// input never carries one.
pub fn push_folded(out: &mut String, s: &str) {
    if s.is_ascii() {
        out.reserve(s.len());
        for &b in s.as_bytes() {
            out.push(fold_ascii_byte(b) as char);
        }
        return;
    }
    for ch in s.nfd().filter(|c| !is_combining_mark(*c)) {
        if ch == '\0' {
            out.push(' ');
        } else {
            out.extend(ch.to_lowercase());
        }
    }
}

/// [`push_folded`] into a fresh `String`.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_folded(&mut out, s);
    out
}

thread_local! {
    /// The buffer the three predicates fold a *haystack* into.
    ///
    /// A needle is folded once by its owner, but the row side is folded per candidate, and
    /// a filter box runs that walk over the whole list on every keystroke: a fresh
    /// `String` per row where one reused buffer does. Only the accented arm reaches here;
    /// an all-ASCII pair never leaves the byte walk.
    ///
    /// Thread-local rather than a field on [`Needle`], which the detail views keep behind a
    /// `Mutex` and which a `RefCell` would stop being `Sync`.
    static FOLD_SCRATCH: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Fold `haystack` into the thread's scratch and answer `matches` against it.
///
/// `matches` is a `str` comparison at every call site, so it cannot re-enter this and the
/// borrow is uncontended by construction.
fn with_folded<R>(haystack: &str, matches: impl FnOnce(&str) -> R) -> R {
    FOLD_SCRATCH.with_borrow_mut(|scratch| {
        scratch.clear();
        push_folded(scratch, haystack);
        matches(scratch)
    })
}

/// The one way to build a [`Needle`]. Trims first — an untrimmed trailing space empties
/// the list.
///
/// The two shape flags are settled on the **folded** text, not interchangeably with the
/// raw input: folding only ever *adds* ASCII-ness, `Björk` coming out as `bjork`, so a
/// query the raw check would call non-ASCII is exactly the one the fast path can serve.
/// Hoisting either onto `raw` stays correct and silently strands every accented needle on
/// the allocating arm of [`Needle::contains`].
pub fn fold_needle(raw: &str) -> Needle {
    let text = fold(raw.trim());
    Needle {
        ascii: text.is_ascii(),
        digits: !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()),
        text,
    }
}

#[cfg(test)]
#[path = "tests/row_match_tests.rs"]
mod tests;
