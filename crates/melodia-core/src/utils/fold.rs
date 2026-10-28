//! Case- and accent-folding, for two strings a human would call the same. Two unrelated questions
//! fold here, each arguing its reason where it asks; sharing one implementation keeps a fix from
//! landing on half of them.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The ASCII half of [`push_folded`]'s rule, spelled once so a caller's own byte walk
/// (`ui::row_match`) can't answer a NUL differently from the long way round.
#[must_use]
pub const fn fold_ascii_byte(b: u8) -> u8 {
    if b == 0 { b' ' } else { b.to_ascii_lowercase() }
}

/// NFD, combining marks dropped, lowercased. `is_combining_mark` takes every `Mark`, kana voicing
/// and Indic spacing marks included, the safe end for comparing two spellings of a name. An
/// embedded NUL becomes a space, ID3v2.4 joining a multi-value frame's values with one.
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

#[cfg(test)]
#[path = "tests/fold_tests.rs"]
mod tests;
