//! Reading an optional string field the way a human would.

/// The field's text where there is any, `None` where it is absent or blank.
///
/// **Two crates ask this and they ask it of unrelated things** — a lyrics directory's answer,
/// which serves `""` about as readily as it omits a field, and a track's own artist tag, where a
/// scan can leave a run of spaces behind. Both mean "there is nothing here", so both read it
/// through one rule.
///
/// `entities::radio` keeps a stricter one of its own: it rejects the empty string without
/// trimming, so a directory value of `" "` still counts as present there.
#[must_use]
pub fn filled(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|text| !text.is_empty())
}
