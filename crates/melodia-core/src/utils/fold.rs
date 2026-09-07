//! Case- and accent-folding, for comparing two strings a human would call the same.
//!
//! **What it does, not why any caller wants it.** Two unrelated questions fold text here — whether
//! a typed needle appears in a row, and whether a lyrics directory's row names the recording that
//! is playing — and they answer to different authorities. Each argues its own reason where it asks;
//! what they share is one implementation, so a fix to the mechanics cannot land on half of them.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The ASCII half of [`push_folded`]'s rule, spelled once so the general path here and a
/// caller's own byte walk cannot answer a NUL differently. `ui::row_match` has such a walk: an
/// all-ASCII needle over an all-ASCII row never leaves the bytes, and folding through this is
/// what keeps that shortcut equal to the long way round.
#[must_use]
pub const fn fold_ascii_byte(b: u8) -> u8 {
    if b == 0 { b' ' } else { b.to_ascii_lowercase() }
}

/// Append `s` to `out`, case- and accent-folded: NFD, combining marks dropped, lowercased.
///
/// `is_combining_mark` is `General_Category=Mark`, so this reaches the Indic spacing marks and the
/// kana voicing marks as well as the Latin diacritics — the loose end of what "accent" can mean,
/// and the safe end for a caller comparing two spellings of one name.
///
/// **An embedded NUL becomes a space.** ID3v2.4 joins the values of a multi-value text frame that
/// way, so a tag's artist field routinely carries one where a person would have typed a comma —
/// and so does a row written from such a tag by somebody else's tagger.
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
#[must_use]
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_folded(&mut out, s);
    out
}
