//! A filter box's query, folded once.
//!
//! **A needle must be folded exactly once, by its owner, and [`Needle`] is what makes that a type
//! rather than a comment.** The failure is silent in the worst way: the row side is always folded,
//! so an unfolded needle still matches, just without accent parity, on that one surface. It lives
//! here rather than beside the filter boxes because the store matches it too, against the folded
//! column every track carries, and a bare `&str` crossing that boundary could be either spelling.
//!
//! Carrying the needle also answers its *shape* once per walk rather than per row (all-ASCII, and
//! all-ASCII-digits), neither of which can change between rows.

use std::cell::RefCell;

use crate::utils::fold::{fold, fold_ascii_byte, push_folded};

/// Widest `i32` in decimal: ten digits plus a sign.
const MAX_I32_DIGITS: usize = 11;

/// Write `value` into `buf` back-to-front and hand back the digits. Years are printed once
/// per row per digit-shaped keystroke, so this stays off the heap for the same reason
/// `player::playback::dsp::push_fixed` does. Both fallbacks are unreachable, and exist because
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
    /// The folded text, for a haystack folded ahead of time that a plain substring check then
    /// answers: the `tracks.search_text` column and the cached lists' packed key.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the needle can name a year at all, so a query can leave the year column out
    /// rather than ask [`Self::matches_number`]'s question of every row.
    pub fn is_number(&self) -> bool {
        self.digits
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
#[path = "tests/needle_tests.rs"]
mod tests;
