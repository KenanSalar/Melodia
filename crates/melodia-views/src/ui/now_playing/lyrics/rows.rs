//! What the panel draws a sheet as: one row per line, plus the gaps worth naming. A gap is the
//! panel's own row, so its floors are spelled here: how long a rest must be to draw is the panel's
//! question, not the sheet's.

use super::Row;
use melodia_core::entities::lyrics::Lyrics as Sheet;

/// **What makes the panel's un-virtualized `for` affordable.** Neither a `.lrc` nor a tag is
/// length-limited, and the longest sung lyrics run a few hundred lines, so a sheet reaching this
/// isn't one.
pub(super) const MAX_ROWS: usize = 600;

/// How long a quiet between two lines has to be before the panel draws it. **A floor against scroll
/// churn**: the panel glides onto the notes and off again, so a shorter break is three targets
/// inside a breath and reads as the panel losing its place.
pub(super) const INTERLUDE_MS: i32 = 5_000;

/// Lower for the run-in: with no line above it costs no scrolling, so it only has to stand long
/// enough not to read as a flicker on mount.
pub(super) const INTRO_MS: i32 = 3_000;

/// The rows a sheet draws, with the notes wherever it leaves a gap worth naming. The run-in falls
/// out of the same walk, `sung_until` starting at the track.
///
/// A line the sheet gave no end to closes nothing: only a blank stamp says the singing stopped. No
/// trailing gap: it needs the last line's end, which almost no sheet states, and would paint notes
/// over the singing everywhere else.
pub(super) fn rows_for(sheet: &Sheet) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::with_capacity(sheet.lines.len());
    let mut sung_until = Some(0);

    for (index, line) in sheet.lines.iter().enumerate() {
        let at_ms = line.at_ms.map(millis);
        let floor = if index == 0 { INTRO_MS } else { INTERLUDE_MS };
        if let (Some(from), Some(at)) = (sung_until, at_ms)
            && at - from >= floor
        {
            rows.push(Row::interlude(from, at));
        }
        rows.push(Row::words(line, at_ms));
        sung_until = line.end_ms.map(millis);

        if rows.len() >= MAX_ROWS {
            log::debug!(
                "lyrics: sheet of {} lines truncated at {MAX_ROWS} rows",
                sheet.lines.len()
            );
            rows.truncate(MAX_ROWS);
            break;
        }
    }
    rows
}

/// A stamp as the panel carries it, saturating at the reach of an `i32`.
pub(super) fn millis(at_ms: i64) -> i32 {
    i32::try_from(at_ms).unwrap_or(i32::MAX)
}
