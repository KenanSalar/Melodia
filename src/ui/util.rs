//! Small shared helpers for the UI glue layer.
//!
//! These were previously copy-pasted across `albums.rs`, `browse.rs`, and
//! `tracks.rs` — identical definitions in three places. Hoisted here so
//! there's a single source of truth.

/// Saturating `i64 → i32` conversion. Slint's generated models use `i32`
/// for ids; DB ids are `i64`. Real ids never overflow, but the saturating
/// fallback keeps the conversion total instead of panicking.
pub fn clamp_i64_to_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

/// Lowercased sort key for an optional string; `None` sorts as the empty
/// string. Used by the in-memory track-list sorts in [`crate::ui::track_sort`].
pub fn opt_lc(s: Option<&str>) -> String {
    s.unwrap_or("").to_lowercase()
}

#[cfg(test)]
#[path = "tests/util_tests.rs"]
mod tests;
