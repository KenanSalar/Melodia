//! Text read forgivingly: an optional field that may be blank, and bytes that are mostly UTF-8.

/// `None` where the field is absent or blank: a lyrics directory serves `""` as readily as it
/// omits a field, and a scan can leave a run of spaces in a tag. `entities::radio` keeps a
/// stricter rule of its own that doesn't trim.
#[must_use]
pub fn filled(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|text| !text.is_empty())
}

/// std's unstable `String::from_utf8_lossy_owned` (rust-lang/rust#129436): valid UTF-8 moves into
/// the `String`, where `String::from_utf8_lossy(&bytes).into_owned()` copies it.
#[must_use]
pub fn from_utf8_lossy_owned(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

#[cfg(test)]
#[path = "tests/text_tests.rs"]
mod tests;
