//! Folding a selection down to what it agrees on, and rendering the results.
//!
//! Pure, and the only part of the dialog that neither reads nor writes a Slint property.

/// Common value across the selection for a string field: `(value, disagrees)`.
/// All rows agree ⇒ that value; they differ ⇒ empty + the multi-value flag.
/// Borrows each row (`&str`) to compare and clones only the winner — vs cloning
/// every row's value just to test agreement.
pub(super) fn common_str<'a>(mut values: impl Iterator<Item = &'a str>) -> (String, bool) {
    let Some(first) = values.next() else {
        return (String::new(), false);
    };
    if values.any(|v| v != first) { (String::new(), true) } else { (first.to_owned(), false) }
}

/// [`common_str`] for a field compared as a whole value rather than as a string.
///
/// Which is what the multi-value fields need: comparing the lists themselves is what lets a genre
/// contain the separator its column happens to render with, and what keeps two credits that render
/// alike from folding together.
pub(super) fn common_value<'a, T>(mut values: impl Iterator<Item = &'a T>) -> (T, bool)
where
    T: Clone + Default + PartialEq + 'a,
{
    let Some(first) = values.next() else {
        return (T::default(), false);
    };
    if values.all(|other| other == first) { (first.clone(), false) } else { (T::default(), true) }
}

/// Common value across the selection for a formatted (numeric) field. Compares a
/// cheap `Copy + Eq` key so no per-row string is allocated to test agreement,
/// and formats only the single winning value. The key must collapse together
/// every value that *renders* identically (e.g. a `Some(0)` and a `None` year
/// both display empty ⇒ same key), so key equality matches display equality.
pub(super) fn common_by<T: Copy, K: Eq>(
    mut values: impl Iterator<Item = T>,
    key: impl Fn(T) -> K,
    fmt: impl Fn(T) -> String,
) -> (String, bool) {
    let Some(first) = values.next() else {
        return (String::new(), false);
    };
    let first_key = key(first);
    if values.any(|v| key(v) != first_key) { (String::new(), true) } else { (fmt(first), false) }
}

/// Display-collapsing key for an integer field: non-positive / absent all render
/// empty (see [`fmt_int`]), so map them to one `None` bucket.
pub(super) fn int_key(v: Option<i32>) -> Option<i32> {
    v.filter(|&n| n > 0)
}

/// Display-collapsing key for BPM: non-finite / non-positive render empty (see
/// [`fmt_bpm`]); the finite-positive values are keyed by their bit pattern so
/// equality is exact without a float `==` (which the pedantic gate rejects).
pub(super) fn bpm_key(v: Option<f64>) -> Option<u64> {
    v.filter(|b| b.is_finite() && *b > 0.0).map(f64::to_bits)
}

/// `Option<i32>` → display string; empty for absent / non-positive (0 is "unset"
/// for year / track / disc).
pub(super) fn fmt_int(v: Option<i32>) -> String {
    match v {
        Some(n) if n > 0 => n.to_string(),
        _ => String::new(),
    }
}

pub(super) fn fmt_bpm(v: Option<f64>) -> String {
    match v {
        Some(b) if b.is_finite() && b > 0.0 => {
            if b.fract().abs() < f64::EPSILON {
                format!("{b:.0}")
            } else {
                format!("{b}")
            }
        }
        _ => String::new(),
    }
}

/// Human byte size (integer math only — avoids the `cast_precision_loss` an
/// `i64 as f64` would trip under the pedantic gate).
pub(super) fn fmt_size(bytes: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = 1024 * KIB;
    const GIB: i64 = 1024 * MIB;
    let b = bytes.max(0);
    if b < KIB {
        format!("{b} B")
    } else if b < MIB {
        format!("{} KB", b / KIB)
    } else if b < GIB {
        format!("{}.{} MB", b / MIB, (b % MIB) * 10 / MIB)
    } else {
        format!("{}.{} GB", b / GIB, (b % GIB) * 10 / GIB)
    }
}

#[cfg(test)]
#[path = "tests/fold_tests.rs"]
mod tests;
