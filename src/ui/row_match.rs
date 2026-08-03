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
//! [`year_matches`] is a substring where the FTS side is a prefix, so `98`
//! narrows a filter box to the 1980s and 1998 while `98*` matches no year
//! at all. That's the field falling in with its neighbours rather than with
//! the index — see its own doc.
//!
//! **A needle must be folded exactly once, by its owner.** Some views store
//! a folded shadow at write time and some fold at read time; either is
//! fine, and folding twice is harmless (the fold is idempotent). Skipping
//! it is not: the row side is always folded, so an unfolded needle silently
//! loses accent parity on that one surface with everything still building.
//!
//! The predicates are shaped by what each surface actually holds.
//! [`track_matches`] takes a full `TrackListRow`; [`most_played_matches`]
//! takes the narrower card projection but searches the identical field set,
//! which is what stops Recently Played's strip and the recency list under it
//! narrowing differently on one query. [`field_contains`] alone serves the
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
fn any_field_matches(fields: &[&str; 5], year: Option<i32>, needle: &str) -> bool {
    fields.iter().any(|f| field_contains(f, needle)) || year_matches(year, needle)
}

/// Folded substring match across a track row's title, artist, album artist,
/// album, genre and year. `needle` must come from [`fold_needle`]; an empty
/// one matches every row, so an unfiltered list needs no branch here.
pub fn track_matches(r: &TrackListRow, needle: &str) -> bool {
    any_field_matches(&search_fields(r), r.year, needle)
}

/// [`track_matches`] over a Most Played card. The card renders only title
/// and artist, and on Recently Played it sits directly above a track list
/// fed by the same search bar — matching a narrower set there would let one
/// query show a genre's tracks in the list and none of its cards above them.
/// Favorites' copy is a tab rather than a neighbour, so it runs the same
/// rule for the plainer reason: one query, one meaning, every page.
pub fn most_played_matches(t: &MostPlayedFavorite, needle: &str) -> bool {
    any_field_matches(&most_played_fields(t), t.year, needle)
}

/// Substring match on a track's year in decimal, without formatting it onto
/// the heap. A needle carrying anything but ASCII digits cannot name a year,
/// which is what keeps the ordinary text query off this path entirely.
///
/// Substring rather than prefix, unlike the FTS side: every other field in
/// these boxes matches mid-word (`hap` finds "Rhapsody", which a per-token
/// prefix would not), so a prefix-only year would be the odd one out.
pub fn year_matches(year: Option<i32>, needle: &str) -> bool {
    let Some(year) = year else {
        return false;
    };
    if needle.is_empty() || !needle.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut buf = [0u8; MAX_I32_DIGITS];
    write_decimal(&mut buf, year).contains(needle)
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

/// Case- and accent-insensitive substring check that skips the allocating
/// path on all-ASCII text — which is where the filter walk spends nearly
/// all of its time. A "Rock" genre detail can hold thousands of rows and
/// this runs over every one of them per throttled keystroke.
///
/// `needle` must come from [`fold_needle`]. An empty needle matches
/// anything — that's what lets every filter walk run unconditionally rather
/// than keep its own empty-search-bar fast path.
pub fn field_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    // An ASCII haystack carries no accent to fold, so the byte walk is the
    // whole answer rather than a first approximation of it.
    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        if n.len() > h.len() {
            return false;
        }
        return h
            .windows(n.len())
            .any(|w| w.iter().zip(n).all(|(a, b)| fold_ascii_byte(*a) == *b));
    }
    fold(haystack).contains(needle)
}

/// Case- and accent-insensitive equality, on the same fold as
/// [`field_contains`].
///
/// Unlike its sibling, an empty `needle` matches only an empty `haystack` —
/// "equals nothing" is a real question with a real answer, where "contains
/// nothing" is what lets a filter walk run with an empty search bar.
///
/// `needle` must come from [`fold_needle`].
pub fn field_equals(haystack: &str, needle: &str) -> bool {
    // Folding an ASCII string is byte-for-byte, so a length mismatch settles
    // it without touching the contents. It settles nothing in the general
    // case: NFD decomposition changes length.
    if haystack.is_ascii() && needle.is_ascii() {
        let n = needle.as_bytes();
        return haystack.len() == n.len()
            && haystack
                .bytes()
                .zip(n)
                .all(|(a, b)| fold_ascii_byte(a) == *b);
    }
    fold(haystack) == needle
}

/// Case- and accent-insensitive prefix check, on the same fold as
/// [`field_contains`]. An empty `needle` matches anything, as there.
///
/// `needle` must come from [`fold_needle`].
pub fn field_starts_with(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let n = needle.as_bytes();
        return haystack.len() >= n.len()
            && haystack
                .bytes()
                .zip(n)
                .all(|(a, b)| fold_ascii_byte(a) == *b);
    }
    fold(haystack).starts_with(needle)
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

/// The form a filter needle has to arrive in before it reaches any
/// predicate here. Trims first — a leading or trailing space is never part
/// of what the user meant, and an untrimmed one empties the list.
pub fn fold_needle(raw: &str) -> String {
    fold(raw.trim())
}

#[cfg(test)]
#[path = "tests/row_match_tests.rs"]
mod tests;
