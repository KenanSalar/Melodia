//! Reading an optional string field the way a human would.

/// `None` where the field is absent or blank: a lyrics directory serves `""` as readily as it
/// omits a field, and a scan can leave a run of spaces in a tag. `entities::radio` keeps a
/// stricter rule of its own that doesn't trim.
#[must_use]
pub fn filled(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|text| !text.is_empty())
}

#[cfg(test)]
#[path = "tests/text_tests.rs"]
mod tests;
