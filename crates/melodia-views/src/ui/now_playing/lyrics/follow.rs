//! Following the song down the sheet: which line is being sung, and where it sits.
//!
//! **The panel owns no state of its own beyond its scroll position.** The sung index and the
//! offset it sits at are written from here, because following the song needs a cumulative offset
//! table and Slint cannot hand one back: a `for` loop exposes no per-item element whose `y` could
//! be read. So this decides how tall each row is drawn and the panel obeys, which is what makes
//! the table and the layout agree by construction rather than by luck.
//!
//! **The panel does not virtualize, and a `ListView` is what it cannot use rather than what it has
//! not got around to.** Slint's listview repeater does measure real per-row heights, so uneven
//! rows are not the objection they read as — but it *owns* the scroller's geometry: it writes
//! `viewport-y` itself on every layout pass, and publishes `viewport-height` as an average row
//! height times the row count, re-derived from whichever rows happen to be mounted. This panel's
//! whole design is the opposite of that: it centres the sung line by writing `viewport-y` from an
//! exact offset table, and the overlay scrollbar gauges extent off `viewport-height`. Under a
//! `ListView` the first would be overwritten every frame and the second would drift as the sheet
//! scrolled through rows a gloss makes nearly twice as tall. What makes the plain `for` right
//! instead is that a sheet is small and, via [`super::rows::MAX_ROWS`], bounded.

use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, Model};

use super::measure::wrapped_lines;
use super::{LyricsUi, Metrics, Row, RowKind};
use melodia_ui::{AppWindow, LyricRow, Lyrics, Player};

/// How long a clicked line keeps the highlight before the clock is assumed to have gone elsewhere.
///
/// The position channel reports about once a second, so for up to that long after a seek it still
/// says the *old* line is being sung; without a pin the panel would glide back to it and return.
const PIN_HOLDS_FOR_MS: f64 = 2_000.0;

/// Re-measure every row against the current width, rebuilding the model and the offset table.
///
/// **Most calls have nothing to publish, and that is what the early return is for.** The panel
/// reports its width on every tick, so this runs 30 times a second for as long as a resize drag
/// lasts — while [`wrapped_lines`] buckets to one, two or three, so almost every one of those
/// widths lays the sheet out exactly as the last did. Past the guard the rows are written
/// *through* rather than replaced: `set_vec` resets the model, which clears the repeater's
/// instances and rebuilds every row's item tree, and on an un-virtualized `for` that is the whole
/// sheet.
///
/// **Ends by restating the sung line**, which is an offset into the table this just moved, and
/// which on a fresh sheet has not been answered at all. The panel's tick is the other caller of
/// [`follow`] and it cannot cover either case: a sheet is fetched again on every mount of that
/// branch, so a paused panel would sit on a `-1` until the transport moved.
pub(super) fn republish(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    let width = ly.width.get();
    let metrics = ly.metrics;
    // **Read here rather than filtered at the source**, so the toggle and the offset table are one
    // answer: a row whose romanization is suppressed has to be shorter by exactly what the panel
    // stops drawing, and both halves read this same pass's decision.
    let romanization_shown = ui.global::<Lyrics>().get_romanization_shown();
    // A flip changes what every row *draws*, not only how tall it is, so the text goes out again
    // with the counts.
    let toggle_moved = ly.published_romanization.replace(romanization_shown) != romanization_shown;

    let mut rows = ly.rows.borrow_mut();
    // A length the model does not share is a different sheet, which has to be replaced rather
    // than written through. It is also what catches a fresh sheet whose measured counts happen to
    // match the ones `Row::words` seeds.
    let replace = ly.model.row_count() != rows.len();
    let mut heights_moved = false;

    for row in rows.iter_mut() {
        let lines = wrapped_lines(&row.text, width, metrics.font_size);
        // Neither of these goes through `wrapped_lines`' floor of one, which is for a blank line a
        // plain sheet spaces its verses with. A row without one has no slot for it at all.
        let romanization_lines = row
            .romanization
            .as_deref()
            .filter(|_| romanization_shown)
            .map_or(0, |sound| wrapped_lines(sound, width, metrics.romanization_font_size));
        let translation_lines = row
            .translation
            .as_deref()
            .map_or(0, |gloss| wrapped_lines(gloss, width, metrics.translation_font_size));

        heights_moved |= (lines, romanization_lines, translation_lines)
            != (row.lines, row.romanization_lines, row.translation_lines);
        row.lines = lines;
        row.romanization_lines = romanization_lines;
        row.translation_lines = translation_lines;
    }

    if !replace && !toggle_moved && !heights_moved {
        drop(rows);
        follow(ui, ly);
        return;
    }

    // Off the counts the pass above wrote, so the table and the drawn rows cannot disagree.
    let mut offsets = Vec::with_capacity(rows.len());
    let mut top = 0.0_f32;
    for row in rows.iter() {
        offsets.push(top);
        // The layout's own `spacing`, which sits between rows rather than inside one.
        top += metrics.row_height(row) + metrics.row_gap;
    }

    let published: Vec<LyricRow> =
        rows.iter().map(|row| published_row(row, romanization_shown)).collect();
    drop(rows);

    *ly.offsets.borrow_mut() = offsets;
    if replace {
        ly.model.set_vec(published);
    } else {
        for (index, row) in published.into_iter().enumerate() {
            ly.model.set_row_data(index, row);
        }
    }
    follow(ui, ly);
}

/// A row as the panel draws it, with the romanization suppressed where the toggle says so.
fn published_row(row: &Row, romanization_shown: bool) -> LyricRow {
    LyricRow {
        text: row.text.clone(),
        romanization: row
            .romanization
            .as_ref()
            .filter(|_| romanization_shown)
            .cloned()
            .unwrap_or_default(),
        translation: row.translation.clone().unwrap_or_default(),
        at_ms: row.at_ms.unwrap_or(-1),
        is_interlude: matches!(row.kind, RowKind::Interlude { .. }),
        line_count: i32::from(row.lines),
        romanization_line_count: i32::from(row.romanization_lines),
        translation_line_count: i32::from(row.translation_lines),
    }
}

/// Move the sung line on, interpolating between the position channel's roughly one-second ticks.
pub(super) fn follow(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    let player = ui.global::<Player>();
    let reported = player.get_position_ms();
    // The two projections rather than `get_vm()`, which clones the whole view model — sixteen
    // strings and two images — for these two numbers, on every tick.
    let is_playing = player.get_vm_is_playing();

    // A changed reading re-anchors the clock; an unchanged one is interpolated from the last.
    // **A paused player re-anchors on every pass**, so the interpolation cannot run on past the
    // position it is holding at: the clock is the only thing that says the song stopped, the
    // reported position simply stops changing.
    if reported != ly.anchor_ms.get() || !is_playing {
        ly.anchor_ms.set(reported);
        ly.anchor_at.set(Instant::now());
    }
    let speed = f64::from(player.get_vm_playback_speed()).max(0.0);
    let advanced = ly.anchor_at.get().elapsed().as_secs_f64() * 1000.0 * speed;
    let position = f64::from(ly.anchor_ms.get()) + advanced;

    let sung = sung_at(&ly.rows.borrow(), position);

    // The pin stands until the clock reaches the clicked line, or until it has had long enough
    // that something else must have moved the player.
    if let Some(pinned) = ly.pinned.get() {
        let stale = (position - f64::from(ly.pinned_at_ms.get())).abs() > PIN_HOLDS_FOR_MS;
        if sung != Some(pinned) && !stale {
            // Restated rather than left alone, so a table rebuilt under the pin still centres it.
            write_active(ui, ly, Some(pinned), position);
            return;
        }
        ly.pinned.set(None);
    }

    write_active(ui, ly, sung, position);
}

/// Seek to the line drawn at a point down the sheet.
///
/// The panel hands over a coordinate rather than a stamp, so the pin below can be set against the
/// *row* that was clicked: a stamp repeated by a chorus names two of them, and pinning the first
/// leaves the panel gliding back up the sheet from a seek into the second.
pub(super) fn seek_at(ui: &AppWindow, ly: &Rc<LyricsUi>, y: f32) {
    let rows = ly.rows.borrow();
    let Some(index) = row_at(&ly.offsets.borrow(), &rows, ly.metrics, y) else {
        return;
    };
    let Some(at_ms) = rows.get(index).and_then(|row| row.at_ms) else {
        return;
    };
    drop(rows);

    // Pinned before the seek: the clock reports the old line for up to a tick, and the panel would
    // otherwise glide back to it and then return.
    ly.pinned.set(Some(index));
    ly.pinned_at_ms.set(at_ms);
    // The seek's own instant, so a click landing on a gap starts its notes empty rather than
    // wherever the song happened to be when the pointer went down.
    write_active(ui, ly, Some(index), f64::from(at_ms));
    ui.global::<Player>().invoke_seek(at_ms);
}

/// The row drawn at a point down the sheet.
///
/// Binary search over the same offsets the follow uses, so a click and the highlight cannot
/// disagree about where a line sits.
pub(super) fn row_at(offsets: &[f32], rows: &[Row], metrics: Metrics, y: f32) -> Option<usize> {
    if y < 0.0 {
        return None;
    }
    let index = offsets.partition_point(|top| *top <= y).checked_sub(1)?;
    let row = rows.get(index)?;

    // **The gap between two rows belongs to the one above**, so a click landing a few pixels wide
    // of a line still seeks it — the gaps are wide enough to be missed into. Below the *last* row
    // there is no line to have meant, and the panel's trailing whitespace is most of what gets
    // clicked by accident.
    let past_the_sheet =
        index + 1 == rows.len() && y > offsets.get(index)? + metrics.row_height(row);
    (!past_the_sheet).then_some(index)
}

/// The last row whose stamp has passed, by binary search over the rows.
///
/// **A sheet is timed or it is not**, which the parser guarantees and the first row is enough to
/// ask: on a timed one every row carries a stamp, so the search is the whole slice and needs no
/// index of the stamped ones to walk. Worth the ask rather than the allocation, the panel calling
/// this on a 33 ms tick.
pub(super) fn sung_at(rows: &[Row], position_ms: f64) -> Option<usize> {
    if rows.first().is_none_or(|row| row.at_ms.is_none()) {
        return None;
    }
    let passed =
        rows.partition_point(|row| row.at_ms.is_some_and(|at| f64::from(at) <= position_ms));
    passed.checked_sub(1)
}

/// Publish which row is sung and where it sits, so the panel can centre it.
fn write_active(ui: &AppWindow, ly: &Rc<LyricsUi>, index: Option<usize>, position_ms: f64) {
    let global = ui.global::<Lyrics>();
    let Some(index) = index else {
        global.set_active_index(-1);
        return;
    };
    let offsets = ly.offsets.borrow();
    let rows = ly.rows.borrow();
    let (Some(top), Some(row)) = (offsets.get(index), rows.get(index)) else {
        return;
    };

    global.set_active_index(i32::try_from(index).unwrap_or(i32::MAX));
    global.set_active_offset(*top);
    // The whole row, gloss included, so the pair is centred together rather than the words alone.
    global.set_active_height(ly.metrics.row_height(row));
    global.set_interlude_progress(interlude_progress(row, position_ms));
}

/// How far through a gap the song is, for the notes to fill against. Zero on a row of words, which
/// have nothing to fill.
pub(super) fn interlude_progress(row: &Row, position_ms: f64) -> f32 {
    let RowKind::Interlude { until_ms } = row.kind else {
        return 0.0;
    };
    let from = f64::from(row.at_ms.unwrap_or(0));
    let span = f64::from(until_ms) - from;
    // A gap that ends where it starts is one the stamps disagree about; full is the answer that
    // leaves nothing filling on screen.
    if span <= 0.0 {
        return 1.0;
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the clamp puts the ratio inside f32's range"
    )]
    let progress = ((position_ms - from) / span).clamp(0.0, 1.0) as f32;
    progress
}
