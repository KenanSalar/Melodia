//! The lyrics panel's Rust half: what it draws, and which line is being sung.
//!
//! **The panel owns no state of its own beyond its scroll position.** Rows, the sung index and the
//! offset that index sits at are all written from here, because following the song needs a
//! cumulative offset table and Slint cannot hand one back: its own `ListView` assumes uniform rows
//! and a `for` loop exposes no per-item element whose `y` could be read. So this decides how tall
//! each row is drawn and the panel obeys, which is what makes the table and the layout agree by
//! construction rather than by luck.
//!
//! **Every position here is milliseconds in an `i32`**, which is what the `Player` global already
//! publishes and what the panel's rows carry. It reaches past three weeks of one track, and it is
//! what lets the interpolation below convert into `f64` without losing a bit.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::{Lyrics as Sheet, LyricsOutcome};
use melodia_ui::{AppWindow, LyricRow, Lyrics, LyricsState, Player};

/// Width to estimate against before the panel has reported its own.
///
/// Only ever wrong for one tick: the panel reports on both its cadences, so a real number lands
/// within a second of it mounting. Sized to the middle of `up-next-width`'s clamp so the first
/// draw is close rather than merely defined.
const ASSUMED_WIDTH: f32 = 340.0;

/// Rendered width of one character at the panel's font size.
///
/// The wrap estimator's only fuzzy term, and it is the trade `ui::chips::estimated_chip_width`
/// makes: a little over half an em, where Vazirmatn's digits sit near 0.55. **The two error
/// directions are not symmetric.** Over-shooting wraps early and costs a blank half-row;
/// under-shooting elides the tail of a line, which loses words. So it is generous on purpose.
const CHAR_WIDTH: f32 = 7.5;

/// Where a lyric line stops being a line and starts being a paragraph. Past this the panel would
/// scroll more than it shows.
const MAX_WRAPPED_LINES: u8 = 3;

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
    /// Wrapped line count, and so this row's height in `Lyrics.line-height` units.
    lines: u8,
}

/// What the panel is showing, and what is needed to follow it.
pub(crate) struct LyricsUi {
    rows: RefCell<Vec<Row>>,
    /// Cumulative top edge of each row in logical pixels, one entry per row.
    offsets: RefCell<Vec<f32>>,
    /// Height of one wrapped line, read from the global the panel also lays out by.
    line_height: Cell<f32>,
    /// Panel width last reported, in logical pixels.
    width: Cell<f32>,
    /// The last position the bridge published, and when it was seen. Together they interpolate a
    /// roughly one-second tick into something a highlight can follow.
    anchor_ms: Cell<i32>,
    anchor_at: Cell<Instant>,
    /// A clicked row, held until the clock catches up with it.
    pinned: Cell<Option<usize>>,
    pinned_at_ms: Cell<i32>,
    model: Rc<VecModel<LyricRow>>,
}

/// How many wrapped lines a row takes at the current width.
///
/// An estimate, and deliberately so: measuring would mean asking the layout, which is the thing
/// that cannot answer. Being wrong costs nothing structural, because the panel draws whatever comes
/// back and the offset table is built from the same number.
fn wrapped_lines(text: &str, width: f32) -> u8 {
    if text.trim().is_empty() || width <= 0.0 {
        return 1;
    }
    // Saturating at `u16` is ample for a lyric line, and `f32::from` on it avoids the precision
    // lint a direct cast would earn.
    let chars = f32::from(u16::try_from(text.chars().count()).unwrap_or(u16::MAX));
    let per_line = (width / CHAR_WIDTH).max(1.0);
    let needed = chars / per_line;

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
        line_height: Cell::new(global.get_line_height()),
        width: Cell::new(ASSUMED_WIDTH),
        anchor_ms: Cell::new(0),
        anchor_at: Cell::new(Instant::now()),
        pinned: Cell::new(None),
        pinned_at_ms: Cell::new(0),
        model,
    });

    // Seeded off `settings.json` the way the visualizer's own toggle is. The menu row has already
    // flipped the property by the time the callback runs, so this only persists.
    let flags = crate::ui::settings_bind::read_or_default(state, "lyrics").lyrics;
    global.set_shown(flags.lyrics_panel_shown);
    {
        let state = state.clone();
        global.on_set_shown(move |shown| {
            state.persist_blocking("set_lyrics_panel_shown", move |s| {
                library::settings::set_lyrics_panel_shown(s, shown)
            });
        });
    }

    {
        let ly = ly.clone();
        global.on_report_width(move |width| {
            // A pixel either way changes no wrap, and this arrives on every tick.
            if (ly.width.get() - width).abs() < 1.0 {
                return;
            }
            ly.width.set(width);
            republish(&ly);
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
        global.on_seek_to(move |at_ms| {
            let Some(ui) = weak.upgrade() else { return };
            // Pinned before the seek: the clock reports the old line for up to a tick, and the
            // panel would otherwise glide back to it and then return.
            let found = ly.rows.borrow().iter().position(|row| row.at_ms == Some(at_ms));
            if let Some(index) = found {
                ly.pinned.set(Some(index));
                ly.pinned_at_ms.set(at_ms);
                write_active(&ui, &ly, Some(index));
            }
            ui.global::<Player>().invoke_seek(at_ms);
        });
    }

    ly
}

/// Write an outcome into the panel.
///
/// `online_enabled` comes from the caller rather than being read here, because it decides only
/// which of two sentences an empty panel shows and the caller is the half holding `AppState`.
pub(super) fn apply(
    ui: &AppWindow,
    ly: &Rc<LyricsUi>,
    outcome: &LyricsOutcome,
    online_enabled: bool,
) {
    ly.pinned.set(None);
    let global = ui.global::<Lyrics>();

    match outcome {
        LyricsOutcome::Sheet(sheet) => {
            take_sheet(ly, sheet);
            republish(ly);
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
pub(super) fn mark_loading(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    clear(ui, ly);
    ui.global::<Lyrics>().set_state(LyricsState::Loading);
}

/// Hand back the rows and the table, on the view's own teardown.
pub(super) fn release(ui: &AppWindow, ly: &Rc<LyricsUi>) {
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
    *ly.rows.borrow_mut() = sheet
        .lines
        .iter()
        .map(|line| Row {
            at_ms: line.at_ms.map(|at| i32::try_from(at).unwrap_or(i32::MAX)),
            text: line.text.clone(),
            lines: 1,
        })
        .collect();
}

/// Re-measure every row against the current width, rebuilding the model and the offset table.
fn republish(ly: &Rc<LyricsUi>) {
    let width = ly.width.get();
    let line_height = ly.line_height.get();

    let mut rows = ly.rows.borrow_mut();
    let mut offsets = Vec::with_capacity(rows.len());
    let mut published = Vec::with_capacity(rows.len());
    let mut top = 0.0_f32;

    for row in rows.iter_mut() {
        row.lines = wrapped_lines(&row.text, width);
        offsets.push(top);
        top += line_height * f32::from(row.lines);
        published.push(LyricRow {
            text: SharedString::from(row.text.as_str()),
            at_ms: row.at_ms.unwrap_or(-1),
            line_count: i32::from(row.lines),
        });
    }
    drop(rows);

    *ly.offsets.borrow_mut() = offsets;
    ly.model.set_vec(published);
}

/// Move the sung line on, interpolating between the position channel's roughly one-second ticks.
fn follow(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    let player = ui.global::<Player>();
    let reported = player.get_position_ms();

    // A changed reading re-anchors the clock; an unchanged one is interpolated from the last.
    if reported != ly.anchor_ms.get() {
        ly.anchor_ms.set(reported);
        ly.anchor_at.set(Instant::now());
    }
    let speed = f64::from(player.get_vm().playback_speed).max(0.0);
    let advanced = ly.anchor_at.get().elapsed().as_secs_f64() * 1000.0 * speed;
    let position = f64::from(ly.anchor_ms.get()) + advanced;

    let sung = sung_at(&ly.rows.borrow(), position);

    // The pin stands until the clock reaches the clicked line, or until it has had long enough
    // that something else must have moved the player.
    if let Some(pinned) = ly.pinned.get() {
        let stale = (position - f64::from(ly.pinned_at_ms.get())).abs() > PIN_HOLDS_FOR_MS;
        if sung != Some(pinned) && !stale {
            return;
        }
        ly.pinned.set(None);
    }

    write_active(ui, ly, sung);
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
    global.set_active_height(ly.line_height.get() * f32::from(row.lines));
}
