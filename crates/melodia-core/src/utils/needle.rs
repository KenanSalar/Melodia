//! A filter box's query, folded exactly once by its owner. An unfolded needle still matches, just
//! without accent parity, so [`Needle`] makes the fold a type rather than a comment, and answers
//! its shape once per walk rather than per row.

use std::cell::RefCell;

use crate::utils::fold::{fold, fold_ascii_byte, push_folded};

/// Widest `i32` in decimal: ten digits plus a sign.
const MAX_I32_DIGITS: usize = 11;

/// Back-to-front into `buf`, off the heap since years print per row per digit keystroke. Both
/// fallbacks are unreachable, standing in for an `as` truncation and an `unwrap`.
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

/// Built only by [`fold_needle`], which is what makes the single fold enforceable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Needle {
    /// Private: handing out a `&str` is how the unfolded spelling gets back in.
    text: String,
    /// Gates the allocation-free byte path.
    ascii: bool,
    /// A non-empty run of ASCII digits, the only shape that can name an integer field.
    digits: bool,
}

/// Not derived, so it is the same needle as `fold_needle("")` rather than one with `ascii` false.
impl Default for Needle {
    fn default() -> Self {
        fold_needle("")
    }
}

impl Needle {
    /// For a haystack folded ahead of time: `tracks.search_text` and the cached lists' packed key.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the needle can name a year, so a query can leave that column out.
    pub fn is_number(&self) -> bool {
        self.digits
    }

    /// A detail's fresh open, which inherits no filter from the one before.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Skips the fold on all-ASCII text, where a filter walk spends nearly all its time. An empty
    /// needle matches anything, so a walk can run unconditionally.
    pub fn contains(&self, haystack: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        // An ASCII haystack has no accent to fold, so the byte walk is the whole answer.
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

    /// An empty needle matches only an empty `haystack`.
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

    /// An empty needle matches anything.
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

    /// A substring of the decimal rather than the FTS side's prefix, every other field here
    /// matching mid-word. `None` matches nothing. Wrong for a field drawn from a short ladder of
    /// values ([`Self::equals_number`]).
    pub fn matches_number(&self, value: Option<i32>) -> bool {
        self.over_decimal(value, |decimal| decimal.contains(&self.text))
    }

    /// The whole number, for a short ladder of values sharing digits, where a substring would
    /// pick most of a bitrate column.
    pub fn equals_number(&self, value: Option<i32>) -> bool {
        self.over_decimal(value, |decimal| decimal == self.text)
    }

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
    /// One buffer per thread for the haystacks, folded per row on every keystroke. Not a field on
    /// [`Needle`], which the detail views hold behind a `Mutex` and a `RefCell` would make `!Sync`.
    static FOLD_SCRATCH: RefCell<String> = const { RefCell::new(String::new()) };
}

/// `matches` is a `str` comparison at every call site, so the borrow can't re-enter.
fn with_folded<R>(haystack: &str, matches: impl FnOnce(&str) -> R) -> R {
    FOLD_SCRATCH.with_borrow_mut(|scratch| {
        scratch.clear();
        push_folded(scratch, haystack);
        matches(scratch)
    })
}

/// Trims first, a trailing space emptying the list. The shape flags read the folded text: folding
/// only adds ASCII-ness (`Björk` → `bjork`), so reading the raw input would strand every accented
/// needle on the allocating arm.
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
