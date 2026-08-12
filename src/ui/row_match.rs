//! What a row is searchable by, and the fold every filter box runs.
//!
//! Every per-view `SearchBar` in the app narrows an in-memory cache rather
//! than re-querying, so none of them goes near `tracks_fts`. That is a
//! second answer to "what does this query match", and it used to be a
//! *narrower* one — title, artist and album only, compared literally. This
//! module makes it the same answer:
//!
//! * [`search_fields`] is the one definition of a track row's searchable
//!   text, ordered like the FTS column list it mirrors. Adding a seventh
//!   field is an edit here plus a migration, not an edit per view.
//! * [`fold_needle`] / [`push_folded`] fold case *and* accents, covering
//!   everything the `unicode61 remove_diacritics 2` tokenizer pinned by
//!   migration `20260802000001` folds — so `bjork` reaches Björk and `be`
//!   reaches Bế Tắc in a filter box exactly as it does in the Search view.
//!   They fold a little more besides; [`push_folded`] says what, and why
//!   that is the safe direction to err in.
//!
//! Years are the one place the two answers still differ on purpose:
//! [`Needle::matches_year`] is a substring where the FTS side is a prefix, so `98`
//! narrows a filter box to the 1980s and 1998 while `98*` matches no year
//! at all. That's the field falling in with its neighbours rather than with
//! the index — see its own doc.
//!
//! **A needle must be folded exactly once, by its owner — and [`Needle`] is
//! what makes that a type rather than a comment.** It used to be a comment, and
//! the failure it warned about is silent in the worst way: the row side is
//! always folded, so an unfolded needle still matches, just without accent
//! parity, on that one surface, with everything still building. Now the only
//! way to get one is [`fold_needle`], and the predicates take nothing else.
//!
//! Carrying the needle rather than a `&str` also lets its *shape* be answered
//! once per walk instead of once per row. Two questions were being re-asked per
//! field and per row of the largest lists in the app: whether the needle is
//! all-ASCII (which decides the allocation-free byte path) and whether it is all
//! ASCII digits (which is the only shape that can name a year). Neither can
//! change between rows.
//!
//! The predicates are shaped by what each surface actually holds.
//! [`track_matches`] takes a full `TrackListRow`; [`most_played_matches`]
//! takes the narrower card projection but searches the identical field set,
//! which is what stops Recently Played's strip and the recency list under it
//! narrowing differently on one query. [`Needle::contains`] alone serves the
//! surfaces matching a single name — the Favorite Artists grid, Artist
//! Detail's Albums strip, the four entity grids, and the Settings page.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::entities::track::{MostPlayedFavorite, TrackListRow};

/// The searchable text of a track row, in the order `tracks_fts` lists its
/// columns. `year` is absent because it is an integer — it joins the match
/// through [`any_field_matches`]. Absent optional fields come through as
/// `""`, which no non-empty needle can match.
///
/// `composer` and `file_name` are indexed by FTS but not carried here:
/// `TrackListRow` has no composer column, and a filename echoes the tags
/// beside it (which is why bm25 weights it 0.5 — an unranked substring
/// filter has no way to de-prioritize it).
pub fn search_fields(r: &TrackListRow) -> [&str; 5] {
    [
        &r.title,
        r.artist.as_deref().unwrap_or_default(),
        r.album_artist.as_deref().unwrap_or_default(),
        r.album.as_deref().unwrap_or_default(),
        r.genre.as_deref().unwrap_or_default(),
    ]
}

/// [`search_fields`] for the Most Played card projection — same five
/// fields, same order.
fn most_played_fields(t: &MostPlayedFavorite) -> [&str; 5] {
    [
        &t.title,
        t.artist.as_deref().unwrap_or_default(),
        t.album_artist.as_deref().unwrap_or_default(),
        t.album.as_deref().unwrap_or_default(),
        t.genre.as_deref().unwrap_or_default(),
    ]
}

/// The match rule both row shapes run: any field, or the year's decimal
/// form, containing `needle`.
fn any_field_matches(fields: &[&str; 5], year: Option<i32>, needle: &Needle) -> bool {
    fields.iter().any(|f| needle.contains(f)) || needle.matches_year(year)
}

/// Folded substring match across a track row's title, artist, album artist,
/// album, genre and year. An empty needle matches every row, so an unfiltered
/// list needs no branch here.
pub fn track_matches(r: &TrackListRow, needle: &Needle) -> bool {
    any_field_matches(&search_fields(r), r.year, needle)
}

/// [`track_matches`] over a Most Played card. The card renders only title
/// and artist, and on Recently Played it sits directly above a track list
/// fed by the same search bar — matching a narrower set there would let one
/// query show a genre's tracks in the list and none of its cards above them.
/// Favorites' copy is a tab rather than a neighbour, so it runs the same
/// rule for the plainer reason: one query, one meaning, every page.
pub fn most_played_matches(t: &MostPlayedFavorite, needle: &Needle) -> bool {
    any_field_matches(&most_played_fields(t), t.year, needle)
}

/// Widest `i32` in decimal: ten digits plus a sign.
const MAX_I32_DIGITS: usize = 11;

/// Write `value` into `buf` back-to-front and hand back the digits. Years
/// are printed once per row per digit-shaped keystroke, so this stays off
/// the heap for the same reason `player::waveform::push_fixed` does.
///
/// Both fallbacks below are unreachable — `rest % 10` is a digit and the
/// bytes written are ASCII — and exist only because the alternatives are a
/// silent `as` truncation and an `unwrap`.
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

/// A filter needle, folded once by [`fold_needle`] and carrying the two answers
/// about its own shape that every predicate below would otherwise re-derive per
/// field, per row.
///
/// Construction is the only way in, which is what makes "folded exactly once by
/// its owner" enforceable rather than advisory — an unfolded needle is now
/// unspellable rather than merely wrong. Views that keep a shadow store this;
/// views that fold at read time hold it for the length of one walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needle {
    /// The folded text. Private: handing out a `&str` is how the unfolded
    /// spelling gets back in.
    text: String,
    /// Whether [`Self::text`] is all-ASCII, which is what lets the substring
    /// check take the allocation-free byte path. The *haystack* still has to be
    /// asked — but asking the needle first short-circuits that scan entirely for
    /// a non-ASCII query, where it can only ever come back useless.
    ascii: bool,
    /// Whether [`Self::text`] is a non-empty run of ASCII digits — the only
    /// shape that can name a year, and so the gate that keeps an ordinary text
    /// query off [`Self::matches_year`] entirely.
    digits: bool,
}

/// Hand-written rather than derived, so that `Needle::default()` and
/// `fold_needle("")` are the *same* needle. A derived `Default` leaves `ascii`
/// false, which is a state [`fold_needle`] can never produce (folding an empty
/// string gives ASCII), and the two would then compare unequal while behaving
/// identically — a difference nothing would surface until something started
/// keying on it.
impl Default for Needle {
    fn default() -> Self {
        fold_needle("")
    }
}

impl Needle {
    /// The folded text, for the one caller that matches against an
    /// already-folded haystack of its own: the packed `RowSearchKey` behind
    /// both cached track lists (`ui::track_list_cache`).
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Reset to the empty needle, which matches everything — the four detail
    /// views' fresh-open, where the incoming entity inherits no filter from the
    /// one before it.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// An empty needle — every filter walk's "no filter" case, and what makes
    /// running one unconditionally cheap.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Case- and accent-insensitive substring check that skips the allocating
    /// path on all-ASCII text — which is where the filter walk spends nearly
    /// all of its time. A "Rock" genre detail can hold thousands of rows and
    /// this runs over every one of them per throttled keystroke.
    ///
    /// An empty needle matches anything — that's what lets every filter walk run
    /// unconditionally rather than keep its own empty-search-bar fast path.
    pub fn contains(&self, haystack: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        // An ASCII haystack carries no accent to fold, so the byte walk is the
        // whole answer rather than a first approximation of it. Needle first:
        // it is one already-known bool, where `haystack.is_ascii()` is a scan of
        // the field.
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
        fold(haystack).contains(&self.text)
    }

    /// Case- and accent-insensitive equality, on the same fold as
    /// [`Self::contains`].
    ///
    /// Unlike its sibling, an empty needle matches only an empty `haystack` —
    /// "equals nothing" is a real question with a real answer, where "contains
    /// nothing" is what lets a filter walk run with an empty search bar.
    pub fn equals(&self, haystack: &str) -> bool {
        // Folding an ASCII string is byte-for-byte, so a length mismatch settles
        // it without touching the contents. It settles nothing in the general
        // case: NFD decomposition changes length.
        if self.ascii && haystack.is_ascii() {
            let n = self.text.as_bytes();
            return haystack.len() == n.len()
                && haystack
                    .bytes()
                    .zip(n)
                    .all(|(a, b)| fold_ascii_byte(a) == *b);
        }
        fold(haystack) == self.text
    }

    /// Case- and accent-insensitive prefix check, on the same fold as
    /// [`Self::contains`]. An empty needle matches anything, as there.
    pub fn starts_with(&self, haystack: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        if self.ascii && haystack.is_ascii() {
            let n = self.text.as_bytes();
            return haystack.len() >= n.len()
                && haystack
                    .bytes()
                    .zip(n)
                    .all(|(a, b)| fold_ascii_byte(a) == *b);
        }
        fold(haystack).starts_with(&self.text)
    }

    /// Substring match on a track's year in decimal, without formatting it onto
    /// the heap. A needle carrying anything but ASCII digits cannot name a year,
    /// and [`Self::digits`] is that question answered once for the whole walk
    /// rather than re-scanned per row.
    ///
    /// Substring rather than prefix, unlike the FTS side: every other field in
    /// these boxes matches mid-word (`hap` finds "Rhapsody", which a per-token
    /// prefix would not), so a prefix-only year would be the odd one out.
    pub fn matches_year(&self, year: Option<i32>) -> bool {
        if !self.digits {
            return false;
        }
        let Some(year) = year else {
            return false;
        };
        let mut buf = [0u8; MAX_I32_DIGITS];
        write_decimal(&mut buf, year).contains(&self.text)
    }
}

/// The ASCII half of [`push_folded`]'s rule, in one place so the byte walk
/// above and the packed key below can't answer a NUL differently.
const fn fold_ascii_byte(b: u8) -> u8 {
    if b == 0 { b' ' } else { b.to_ascii_lowercase() }
}

/// Append `s` to `out`, case- and accent-folded: decomposed to NFD, combining
/// marks dropped, lowercased. Covers everything the `unicode61
/// remove_diacritics 2` tokenizer `tracks_fts` uses folds, so a spelling that
/// finds a track in the Search view finds it in a filter box too — mode 2
/// rather than `SQLite`'s legacy default 1 is what reaches the two-mark
/// characters of scripts like Vietnamese.
///
/// It is deliberately the *looser* of the two, not an exact mirror.
/// `is_combining_mark` is `General_Category=Mark`, so this also drops the
/// spacing marks of Indic scripts and the kana voicing marks, where
/// `SQLite`'s table is Latin-scoped. In a substring filter that only ever
/// widens a match — no row a query names can be hidden by it — and
/// under-folding is the half that would actually show.
///
/// An embedded NUL becomes a space, which is not only about the Tracks view
/// packing its fields into one `\0`-separated string: ID3v2.4 joins a
/// multi-value text frame the same way, so a tag really can carry one. A
/// needle typed into a text input never can, so the mapping only ever opens
/// a match up.
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

/// The one way to build a [`Needle`], and so the form a filter needle has to
/// arrive in before it reaches any predicate here. Trims first — a leading or
/// trailing space is never part of what the user meant, and an untrimmed one
/// empties the list.
///
/// The two shape flags are settled here, on the **folded** text, and that is not
/// interchangeable with asking the raw input. Folding only ever *adds*
/// ASCII-ness: `Björk` decomposes to `o` + a combining diaeresis, the mark is
/// dropped, and what comes out is `bjork` — so a query the raw check would call
/// non-ASCII is exactly the one the fast path can serve. Hoisting either flag
/// onto `raw` stays correct and silently strands every accented needle on the
/// allocating arm of [`Needle::contains`], which is the cost this type exists to
/// remove.
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
