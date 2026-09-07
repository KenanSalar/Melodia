//! The lyrics panel's Rust half: what it draws, and which line is being sung.
//!
//! **The panel owns no state of its own beyond its scroll position.** Rows, the sung index and the
//! offset that index sits at are all written from here, because following the song needs a
//! cumulative offset table and Slint cannot hand one back: a `for` loop exposes no per-item
//! element whose `y` could be read. So this decides how tall each row is drawn and the panel
//! obeys, which is what makes the table and the layout agree by construction rather than by luck.
//!
//! **Every position here is milliseconds in an `i32`**, which is what the `Player` global already
//! publishes and what the panel's rows carry. It reaches past three weeks of one track, and it is
//! what lets the interpolation below convert into `f64` without losing a bit.
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
//! instead is that a sheet is small and, via [`MAX_ROWS`], bounded.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use async_compat::Compat;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use super::NowPlayingState;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::{Lyrics as Sheet, LyricsOutcome};
use melodia_core::entities::track::TrackSummary;
use melodia_ui::{AppWindow, LyricRow, Lyrics, LyricsState, Player};

/// Width to estimate against before the panel has reported its own.
///
/// Only ever wrong for one tick: the panel reports on both its cadences, so a real number lands
/// within a second of it mounting. Sized to the middle of `up-next-width`'s clamp so the first
/// draw is close rather than merely defined.
const ASSUMED_WIDTH: f32 = 340.0;

/// How wide one proportional character is drawn, in ems.
///
/// The wrap estimator's only fuzzy term, and it is the trade `ui::chips::estimated_chip_width`
/// makes: a little over half an em, where Vazirmatn's digits sit near 0.55. **The two error
/// directions are not symmetric.** Over-shooting wraps early and costs a blank half-row;
/// under-shooting elides the tail of a line, which loses words. So it is generous on purpose.
const CHAR_EMS: f32 = 0.54;

/// Where a lyric line stops being a line and starts being a paragraph. Past this the panel would
/// scroll more than it shows.
const MAX_WRAPPED_LINES: u8 = 3;

/// The most lines the panel will draw from one sheet.
///
/// **This is what makes the un-virtualized `for` in the panel affordable, so it is a bound rather
/// than a guard against anything.** Nothing about a `.lrc` file or a lyrics tag is length-limited
/// and both come from outside, so without a cap the row count is whatever a malformed file says.
/// Far past any song — the longest sung lyrics run a few hundred lines — so a sheet reaching it is
/// not one, and losing its tail costs nothing a reader wanted.
const MAX_ROWS: usize = 600;

/// How long a clicked line keeps the highlight before the clock is assumed to have gone elsewhere.
///
/// The position channel reports about once a second, so for up to that long after a seek it still
/// says the *old* line is being sung; without a pin the panel would glide back to it and return.
const PIN_HOLDS_FOR_MS: f64 = 2_000.0;

/// One row as the panel draws it.
struct Row {
    /// `None` on an untimed sheet, the two never mixing within one sheet.
    at_ms: Option<i32>,
    text: String,
    /// The gloss a bilingual sheet carries under the words, drawn at its own size beneath them.
    translation: Option<String>,
    /// Wrapped line count, and so this row's height in `Lyrics.line-height` units.
    lines: u8,
    /// The same for the gloss, in `Lyrics.translation-line-height` units. Zero without one.
    translation_lines: u8,
}

/// The type scale the panel draws by, taken from the `Lyrics` global so both halves lay a sheet
/// out against one set of numbers.
#[derive(Clone, Copy)]
struct Metrics {
    font_size: f32,
    line_height: f32,
    translation_font_size: f32,
    translation_line_height: f32,
    translation_gap: f32,
    row_gap: f32,
}

impl Metrics {
    /// How tall a row's own box is: the words, and the gloss under them where there is one.
    ///
    /// The gap *between* rows is not in here — that is the layout's `spacing`, and it belongs to
    /// neither of the rows it separates.
    fn row_height(self, row: &Row) -> f32 {
        let words = self.line_height * f32::from(row.lines);
        if row.translation_lines == 0 {
            return words;
        }
        words
            + self.translation_gap
            + self.translation_line_height * f32::from(row.translation_lines)
    }
}

/// What the panel is showing, and what is needed to follow it.
pub(crate) struct LyricsUi {
    rows: RefCell<Vec<Row>>,
    /// Cumulative top edge of each row in logical pixels, one entry per row.
    offsets: RefCell<Vec<f32>>,
    /// The sizes and line heights the panel draws by.
    metrics: Cell<Metrics>,
    /// Panel width last reported, in logical pixels.
    width: Cell<f32>,
    /// The last position the bridge published, and when it was seen. Together they interpolate a
    /// roughly one-second tick into something a highlight can follow.
    anchor_ms: Cell<i32>,
    anchor_at: Cell<Instant>,
    /// A clicked row, held until the clock catches up with it.
    pinned: Cell<Option<usize>>,
    pinned_at_ms: Cell<i32>,
    /// The track path the resident sheet belongs to, and `None` whenever the rows have been
    /// handed back. **This is what makes the reseed idempotent**, so the three unrelated edges
    /// that reach it cost one lookup between them rather than one each.
    holding: RefCell<Option<String>>,
    /// Filled after [`install`] returns, because looking a sheet up needs the `NowPlayingState`
    /// this is a field of. `NowPlayingState`'s own two seeders, one layer down.
    reseed: RefCell<Option<Box<dyn Fn()>>>,
    model: Rc<VecModel<LyricRow>>,
}

impl LyricsUi {
    /// Look a sheet up for whatever is playing, unless the panel already holds it.
    ///
    /// A no-op before [`install`]'s caller has wired the hook.
    pub(super) fn kick(&self) {
        if let Some(reseed) = self.reseed.borrow().as_ref() {
            reseed();
        }
    }

    /// Whether the resident sheet is already this track's.
    fn holds(&self, path: &str) -> bool {
        self.holding.borrow().as_deref() == Some(path)
    }
}

/// Whether a character is drawn on a square em rather than a proportional one.
///
/// Unicode's East Asian Wide and Fullwidth ranges, trimmed to what a lyric sheet reaches. Worth
/// the ranges rather than one averaged width: Hangul and CJK are half of what this panel is for,
/// and counting them proportionally under-estimates a Korean line by nearly half, which is enough
/// to elide words the panel had the room for.
fn is_full_width(ch: char) -> bool {
    matches!(u32::from(ch),
        0x1100..=0x115F         // Hangul jamo
        | 0x2E80..=0x303E       // CJK radicals and punctuation
        | 0x3041..=0x33FF       // kana, Hangul compatibility jamo, CJK compatibility
        | 0x3400..=0x4DBF       // CJK extension A
        | 0x4E00..=0x9FFF       // CJK unified ideographs
        | 0xA960..=0xA97F       // Hangul jamo extended-A
        | 0xAC00..=0xD7A3       // Hangul syllables
        | 0xF900..=0xFAFF       // CJK compatibility ideographs
        | 0xFE30..=0xFE6F       // CJK compatibility and small forms
        | 0xFF01..=0xFF60       // fullwidth forms
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F     // emoji, drawn square
        | 0x20000..=0x3FFFD     // CJK extensions B and beyond
    )
}

/// How wide a line is set, in ems.
fn em_width(text: &str) -> f32 {
    text.chars().map(|ch| if is_full_width(ch) { 1.0 } else { CHAR_EMS }).sum()
}

/// How many wrapped lines a run of text takes at the current width and size.
///
/// An estimate, and deliberately so: measuring would mean asking the layout, which is the thing
/// that cannot answer. Being wrong costs nothing structural, because the panel draws whatever comes
/// back and the offset table is built from the same number.
fn wrapped_lines(text: &str, width: f32, font_size: f32) -> u8 {
    if text.trim().is_empty() || width <= 0.0 || font_size <= 0.0 {
        return 1;
    }
    let per_line = (width / font_size).max(1.0);
    let needed = em_width(text) / per_line;

    // Bucketed rather than rounded, so the count comes out of comparisons and never a cast.
    if needed <= 1.0 {
        1
    } else if needed <= 2.0 {
        2
    } else {
        MAX_WRAPPED_LINES
    }
}

/// Install the panel's callbacks and seed the toggle.
pub(super) fn install(ui: &AppWindow, state: &AppState) -> Rc<LyricsUi> {
    let model: Rc<VecModel<LyricRow>> = Rc::new(VecModel::default());
    let global = ui.global::<Lyrics>();
    global.set_rows(ModelRc::from(model.clone()));

    let ly = Rc::new(LyricsUi {
        rows: RefCell::new(Vec::new()),
        offsets: RefCell::new(Vec::new()),
        metrics: Cell::new(Metrics {
            font_size: global.get_font_size(),
            line_height: global.get_line_height(),
            translation_font_size: global.get_translation_font_size(),
            translation_line_height: global.get_translation_line_height(),
            translation_gap: global.get_translation_gap(),
            row_gap: global.get_row_gap(),
        }),
        width: Cell::new(ASSUMED_WIDTH),
        anchor_ms: Cell::new(0),
        anchor_at: Cell::new(Instant::now()),
        pinned: Cell::new(None),
        pinned_at_ms: Cell::new(0),
        holding: RefCell::new(None),
        reseed: RefCell::new(None),
        model,
    });

    // Seeded off `settings.json` the way the visualizer's own toggle is. The menu row has already
    // flipped the property by the time the callback runs, so the property half is done.
    let flags = crate::ui::settings_bind::read_or_default(state, "lyrics").lyrics;
    global.set_shown(flags.lyrics_panel_shown);
    {
        let state = state.clone();
        let ly_toggle = ly.clone();
        global.on_set_shown(move |shown| {
            state.persist_blocking("set_lyrics_panel_shown", move |s| {
                library::settings::set_lyrics_panel_shown(s, shown)
            });
            // **Switching the panel on is a reason to look a sheet up**, and until this line it
            // was not one: the fetch hung off a track change alone, so turning it on mid-song
            // showed an empty panel until the next track — or until a restart, which is the
            // *seed* path and does run.
            ly_toggle.kick();
        });
    }

    {
        let weak = ui.as_weak();
        let ly = ly.clone();
        global.on_report_width(move |width| {
            // A pixel either way changes no wrap, and this arrives on every tick.
            if (ly.width.get() - width).abs() < 1.0 {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            ly.width.set(width);
            republish(&ui, &ly);
        });
    }

    {
        let weak = ui.as_weak();
        let ly = ly.clone();
        global.on_hover_at(move |y| {
            let Some(ui) = weak.upgrade() else { return };
            let found = row_at(&ly.offsets.borrow(), &ly.rows.borrow(), ly.metrics.get(), y);
            let index = found.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
            // `Property::set` is value-compared, so the rows are only dirtied when the pointer
            // crosses from one line to the next rather than on every motion event.
            ui.global::<Lyrics>().set_hover_index(index);
        });
    }

    {
        let weak = ui.as_weak();
        let ly = ly.clone();
        global.on_tick(move || {
            let Some(ui) = weak.upgrade() else { return };
            follow(&ui, &ly);
        });
    }

    {
        let weak = ui.as_weak();
        let ly = ly.clone();
        global.on_seek_at(move |y| {
            let Some(ui) = weak.upgrade() else { return };
            seek_at(&ui, &ly, y);
        });
    }

    ly
}

/// The sheet for `track`, or `None` where the panel should be left as it is.
///
/// A failure is logged and reads as "nothing found": a sidecar in some old codepage or a directory
/// that is down are both, to a reader, a panel with no words in it, and neither is worth a toast.
pub(super) async fn fetch(state: &AppState, track: &TrackSummary) -> LyricsOutcome {
    match library::lyrics::for_track(state, track).await {
        Ok(outcome) => outcome,
        Err(e) => {
            log::debug!(
                "ui::now_playing lyrics for {}: {}",
                track.id,
                melodia_core::error::describe(&e)
            );
            LyricsOutcome::Absent
        }
    }
}

/// Bring the panel in line with whatever is playing, looking a sheet up if it has to.
///
/// **Three unrelated edges reach this, and dropping any one of them shows an empty panel.** The
/// sheet is handed back on every close, which is the trade this feature makes against holding a
/// resident copy for the life of the process — so a re-open of the *same* track has nothing to
/// paint, and the artwork's already-applied guard has no way to know that. A track change is
/// `apply_source_change`'s; the other two are the view re-opening and the 3-dot toggle, and both
/// arrive here.
///
/// Idempotent by [`LyricsUi::holds`], so the edges may overlap freely: whichever gets there first
/// pays, and the claim is taken before the first `.await` rather than after the last.
fn reseed(weak: &Weak<AppWindow>, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let Some(ui) = weak.upgrade() else { return };
    let ly = &np_state.lyrics;

    let track = np_state.current_source.borrow().as_ref().and_then(|s| s.track.clone());
    let shown = ui.global::<Lyrics>().get_shown();
    let Some(track) = track.filter(|_| shown) else {
        // Switched off, or a station, which has no words to look up. Either way the previous
        // song's sheet must not sit under it.
        if ly.holding.borrow().is_some() {
            release(&ui, ly);
        }
        return;
    };
    if ly.holds(&track.file_path) {
        return;
    }

    mark_loading(&ui, ly, &track.file_path);
    let weak = weak.clone();
    let state = state.clone();
    let np_state = np_state.clone();
    let res = slint::spawn_local(Compat::new(async move {
        let outcome = fetch(&state, &track).await;
        let Some(ui) = weak.upgrade() else { return };
        // The song may have moved under the lookup; the claim taken above is what says so, and it
        // is the same test the two other edges dedupe on.
        if !np_state.lyrics.holds(&track.file_path) {
            return;
        }
        apply(&ui, &np_state.lyrics, &track.file_path, &outcome, state.lyrics_online_enabled.get());
    }));
    if let Err(e) = res {
        log::warn!("ui::now_playing lyrics reseed task spawn_local: {e}");
    }
}

/// Wire the reseed hook, once the `NowPlayingState` this panel's state is a field of exists.
pub(super) fn wire_reseed(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let weak_ui = ui.as_weak();
    let state = state.clone();
    let weak_np = Rc::downgrade(np_state);
    *np_state.lyrics.reseed.borrow_mut() = Some(Box::new(move || {
        let Some(np_state) = weak_np.upgrade() else {
            return;
        };
        reseed(&weak_ui, &state, &np_state);
    }));
}

/// Write an outcome into the panel.
///
/// `online_enabled` comes from the caller rather than being read here, because it decides only
/// which of two sentences an empty panel shows and the caller is the half holding `AppState`.
pub(super) fn apply(
    ui: &AppWindow,
    ly: &Rc<LyricsUi>,
    track_path: &str,
    outcome: &LyricsOutcome,
    online_enabled: bool,
) {
    ly.pinned.set(None);
    let global = ui.global::<Lyrics>();

    // Recorded whichever of the three this came to, so a re-open that finds the same track already
    // answered does not pay for a second lookup — including the two that draw no rows, which are
    // answers rather than absences.
    *ly.holding.borrow_mut() = Some(track_path.to_owned());

    match outcome {
        LyricsOutcome::Sheet(sheet) => {
            take_sheet(ly, sheet);
            republish(ui, ly);
            global.set_synced(sheet.is_synced());
            global.set_state(LyricsState::Ready);
        }
        LyricsOutcome::Instrumental => {
            clear(ui, ly);
            global.set_state(LyricsState::Instrumental);
        }
        LyricsOutcome::Absent => {
            clear(ui, ly);
            // The one state that names a setting, being the only one a reader can act on here.
            global.set_state(if online_enabled {
                LyricsState::Missing
            } else {
                LyricsState::Off
            });
        }
    }
}

/// Say a lookup is out, so the panel is not claiming "not found" while it is still looking.
///
/// **Claims the track before the `.await`, not after it.** The lookup can reach a file and then a
/// socket, and a reseed landing in that window would otherwise see a sheet for the *previous*
/// track and start a second one for the same song.
pub(super) fn mark_loading(ui: &AppWindow, ly: &Rc<LyricsUi>, track_path: &str) {
    clear(ui, ly);
    *ly.holding.borrow_mut() = Some(track_path.to_owned());
    ui.global::<Lyrics>().set_state(LyricsState::Loading);
}

/// Hand back the rows and the table, on the view's own teardown.
///
/// Gives up the claim with them, so the next open looks the sheet up again rather than believing
/// it is still on screen. That belief is what left a re-opened view blank until a restart.
pub(super) fn release(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    ly.holding.borrow_mut().take();
    clear(ui, ly);
    ui.global::<Lyrics>().set_state(LyricsState::Idle);
}

fn clear(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    ly.rows.borrow_mut().clear();
    ly.offsets.borrow_mut().clear();
    ly.pinned.set(None);
    ly.model.set_vec(Vec::new());

    let global = ui.global::<Lyrics>();
    global.set_synced(false);
    global.set_active_index(-1);
    global.set_active_offset(0.0);
    global.set_active_height(0.0);
}

/// Keep the sheet's text and stamps. Heights are not kept, following the width instead.
fn take_sheet(ly: &Rc<LyricsUi>, sheet: &Sheet) {
    if sheet.lines.len() > MAX_ROWS {
        log::debug!("lyrics: sheet of {} lines truncated to {MAX_ROWS}", sheet.lines.len());
    }
    *ly.rows.borrow_mut() = sheet
        .lines
        .iter()
        .take(MAX_ROWS)
        .map(|line| Row {
            at_ms: line.at_ms.map(|at| i32::try_from(at).unwrap_or(i32::MAX)),
            text: line.text.clone(),
            translation: line.translation.clone(),
            lines: 1,
            translation_lines: 0,
        })
        .collect();
}

/// Re-measure every row against the current width, rebuilding the model and the offset table.
///
/// **Ends by restating the sung line**, which is an offset into the table this just moved, and
/// which on a fresh sheet has not been answered at all. The panel's tick is the other caller of
/// [`follow`] and it cannot cover either case: a sheet is fetched again on every mount of that
/// branch, so a paused panel would sit on a `-1` until the transport moved.
fn republish(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    let width = ly.width.get();
    let metrics = ly.metrics.get();

    let mut rows = ly.rows.borrow_mut();
    let mut offsets = Vec::with_capacity(rows.len());
    let mut published = Vec::with_capacity(rows.len());
    let mut top = 0.0_f32;

    for row in rows.iter_mut() {
        row.lines = wrapped_lines(&row.text, width, metrics.font_size);
        // Not through `wrapped_lines`, whose floor of one is for a blank line a plain sheet spaces
        // its verses with. A row with no gloss has no second slot at all.
        row.translation_lines = row
            .translation
            .as_deref()
            .map_or(0, |gloss| wrapped_lines(gloss, width, metrics.translation_font_size));

        offsets.push(top);
        // The layout's own `spacing`, which sits between rows rather than inside one.
        top += metrics.row_height(row) + metrics.row_gap;
        published.push(LyricRow {
            text: SharedString::from(row.text.as_str()),
            translation: row.translation.as_deref().map(SharedString::from).unwrap_or_default(),
            at_ms: row.at_ms.unwrap_or(-1),
            line_count: i32::from(row.lines),
            translation_line_count: i32::from(row.translation_lines),
        });
    }
    drop(rows);

    *ly.offsets.borrow_mut() = offsets;
    ly.model.set_vec(published);
    follow(ui, ly);
}

/// Move the sung line on, interpolating between the position channel's roughly one-second ticks.
fn follow(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    let player = ui.global::<Player>();
    let reported = player.get_position_ms();
    let vm = player.get_vm();

    // A changed reading re-anchors the clock; an unchanged one is interpolated from the last.
    // **A paused player re-anchors on every pass**, so the interpolation cannot run on past the
    // position it is holding at: the clock is the only thing that says the song stopped, the
    // reported position simply stops changing.
    if reported != ly.anchor_ms.get() || !vm.is_playing {
        ly.anchor_ms.set(reported);
        ly.anchor_at.set(Instant::now());
    }
    let speed = f64::from(vm.playback_speed).max(0.0);
    let advanced = ly.anchor_at.get().elapsed().as_secs_f64() * 1000.0 * speed;
    let position = f64::from(ly.anchor_ms.get()) + advanced;

    let sung = sung_at(&ly.rows.borrow(), position);

    // The pin stands until the clock reaches the clicked line, or until it has had long enough
    // that something else must have moved the player.
    if let Some(pinned) = ly.pinned.get() {
        let stale = (position - f64::from(ly.pinned_at_ms.get())).abs() > PIN_HOLDS_FOR_MS;
        if sung != Some(pinned) && !stale {
            // Restated rather than left alone, so a table rebuilt under the pin still centres it.
            write_active(ui, ly, Some(pinned));
            return;
        }
        ly.pinned.set(None);
    }

    write_active(ui, ly, sung);
}

/// Seek to the line drawn at a point down the sheet.
///
/// The panel hands over a coordinate rather than a stamp, so the pin below can be set against the
/// *row* that was clicked: a stamp repeated by a chorus names two of them, and pinning the first
/// leaves the panel gliding back up the sheet from a seek into the second.
fn seek_at(ui: &AppWindow, ly: &Rc<LyricsUi>, y: f32) {
    let rows = ly.rows.borrow();
    let Some(index) = row_at(&ly.offsets.borrow(), &rows, ly.metrics.get(), y) else {
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
    write_active(ui, ly, Some(index));
    ui.global::<Player>().invoke_seek(at_ms);
}

/// The row drawn at a point down the sheet.
///
/// Binary search over the same offsets the follow uses, so a click and the highlight cannot
/// disagree about where a line sits.
fn row_at(offsets: &[f32], rows: &[Row], metrics: Metrics, y: f32) -> Option<usize> {
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

/// The last row whose stamp has passed, by binary search over the stamped rows.
///
/// A sheet is timed or it is not, so on a timed one every row is stamped and the search is over
/// the whole slice; the filter is what keeps it correct if that ever stops being true.
fn sung_at(rows: &[Row], position_ms: f64) -> Option<usize> {
    let stamped: Vec<usize> =
        rows.iter().enumerate().filter(|(_, row)| row.at_ms.is_some()).map(|(i, _)| i).collect();
    let passed = stamped.partition_point(|&i| f64::from(rows[i].at_ms.unwrap_or(0)) <= position_ms);
    passed.checked_sub(1).map(|slot| stamped[slot])
}

/// Publish which row is sung and where it sits, so the panel can centre it.
fn write_active(ui: &AppWindow, ly: &Rc<LyricsUi>, index: Option<usize>) {
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
    global.set_active_height(ly.metrics.get().row_height(row));
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
