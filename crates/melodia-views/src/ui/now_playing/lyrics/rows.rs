//! What the panel draws a sheet as: one row per line, plus the gaps worth naming.
//!
//! A gap is a row of the panel's own making rather than anything the sheet holds, so the floors it
//! answers to are spelled here and not in the sheet: how long a rest has to be before it is worth
//! drawing is a question about the panel, and the sheet has already answered the only one it can.

use super::Row;
use melodia_core::entities::lyrics::Lyrics as Sheet;

/// The most lines the panel will draw from one sheet.
///
/// **This is what makes the un-virtualized `for` in the panel affordable, so it is a bound rather
/// than a guard against anything.** Nothing about a `.lrc` file or a lyrics tag is length-limited
/// and both come from outside, so without a cap the row count is whatever a malformed file says.
/// Far past any song — the longest sung lyrics run a few hundred lines — so a sheet reaching it is
/// not one, and losing its tail costs nothing a reader wanted.
pub(super) const MAX_ROWS: usize = 600;

/// How long a sheet has to leave the singer quiet *between two lines* before the panel draws the
/// break.
///
/// **A floor rather than a preference, and what it holds off is scroll churn**: the panel glides
/// off the line above onto the notes and off them onto the line below, so a shorter break is three
/// targets inside a breath and reads as the panel losing its place rather than as the song resting.
pub(super) const INTERLUDE_MS: i32 = 5_000;

/// The same for the run-in, which is lower because neither half of [`INTERLUDE_MS`]' argument
/// reaches it.
///
/// A gap with no line above it costs no scrolling: the panel mounts on it and leaves it once, which
/// is the one move it would have made anyway. And under its floor the run-in is the only gap that
/// leaves *nothing* lit, where a short break between two lines still leaves the line above it sung.
/// So all that is left to ask is whether the row stands long enough to read as deliberate rather
/// than as a flicker on mount.
pub(super) const INTRO_MS: i32 = 3_000;

/// The rows a sheet draws, with the notes wherever it leaves a gap worth naming.
///
/// **The run-in falls out of the same walk**, `sung_until` starting at the track rather than at the
/// first line: a sheet whose first stamp is a minute in is a minute of quiet with nothing above it,
/// which is what an interlude is. It is also the only gap that needs no blank stamp to be found, so
/// it is the one every sheet gets, and it answers to its own floor for the reasons [`INTRO_MS`]
/// argues.
///
/// **A line the sheet gave no end to closes nothing**, and the run to the next line reads as a long
/// line rather than a rest. That is the honest reading: only a blank stamp says the singing
/// stopped, and a sheet without one is not describing a pause it left out.
///
/// No trailing gap, deliberately. It would need the *last* line's end, which almost no sheet
/// states, so it would appear on a minority of tracks and paint notes over the singing on the rest.
pub(super) fn rows_for(sheet: &Sheet) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::with_capacity(sheet.lines.len());
    let mut sung_until = Some(0);

    for (index, line) in sheet.lines.iter().enumerate() {
        let at_ms = line.at_ms.map(millis);
        // The run-in is the one gap with no line above it, so it is the one that answers to
        // `INTRO_MS`.
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
