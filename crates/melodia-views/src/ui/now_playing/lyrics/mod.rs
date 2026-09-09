//! The lyrics panel's Rust half.
//!
//! Four modules under one door: [`measure`] estimates how wide a line is set, [`rows`] turns a
//! sheet into the rows the panel draws, [`sheet`] gets one and hands it back, and [`follow`] keeps
//! the sung line marked. What is left here is the state all four read and the wiring that installs
//! them.
//!
//! **Every position is milliseconds in an `i32`**, which is what the `Player` global already
//! publishes and what the panel's rows carry. It reaches past three weeks of one track, and it is
//! what lets [`follow`]'s interpolation convert into `f64` without losing a bit.

mod follow;
mod measure;
mod rows;
mod sheet;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::NowPlayingState;
use crate::ui::settings_bind;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::LyricLine;
use melodia_ui::{AppWindow, LyricRow, Lyrics, Settings};

pub(super) use sheet::{release, wire_menu, wire_reseed};

/// Width to estimate against before the panel has reported its own.
///
/// Only ever wrong for one tick: the panel reports on both its cadences, so a real number lands
/// within a second of it mounting. Sized to the middle of `up-next-width`'s clamp so the first
/// draw is close rather than merely defined.
const ASSUMED_WIDTH: f32 = 340.0;

/// What a row is.
///
/// A gap is a row of the panel's own making rather than anything the sheet holds, which is why it
/// is spelled here and not in the sheet: how long a rest has to be before it is worth drawing is a
/// question about the panel, and the sheet has already answered the only one it can.
#[derive(Clone, Copy)]
pub(super) enum RowKind {
    Words,
    /// Carries where the gap ends, the row's own `at_ms` being where it starts, so the notes have
    /// both ends of the span they fill across.
    Interlude {
        until_ms: i32,
    },
}

/// One row as the panel draws it. An interlude carries no text and none of the two lines under it.
struct Row {
    kind: RowKind,
    /// `None` on an untimed sheet, the two never mixing within one sheet.
    at_ms: Option<i32>,
    text: String,
    /// How the words sound, for a script that does not spell it. `None` for a Latin line.
    romanization: Option<String>,
    /// The gloss a bilingual sheet carries under the words, drawn at its own size beneath them.
    translation: Option<String>,
    /// Wrapped line count, and so this row's height in `Lyrics.line-height` units.
    lines: u8,
    /// The same for the romanization, in `Lyrics.romanization-line-height` units. Zero without
    /// one, and zero for every row while the toggle is off.
    romanization_lines: u8,
    /// The same for the gloss, in `Lyrics.translation-line-height` units. Zero without one.
    translation_lines: u8,
}

impl Row {
    /// A line of the sheet. The three counts are [`follow::republish`]'s to fill against the live
    /// width.
    fn words(line: &LyricLine, at_ms: Option<i32>) -> Self {
        Self {
            kind: RowKind::Words,
            at_ms,
            text: line.text.clone(),
            romanization: line.romanization.clone(),
            translation: line.translation.clone(),
            lines: 1,
            romanization_lines: 0,
            translation_lines: 0,
        }
    }

    /// The quiet between two lines, drawn as the notes.
    ///
    /// Blank text is what makes this cost no arithmetic anywhere else: `wrapped_lines` floors a
    /// blank run at one line, so the row comes out exactly as tall as a one-line verse and
    /// [`Metrics::row_height`] needs no arm for it.
    fn interlude(from_ms: i32, until_ms: i32) -> Self {
        Self {
            kind: RowKind::Interlude { until_ms },
            at_ms: Some(from_ms),
            text: String::new(),
            romanization: None,
            translation: None,
            lines: 1,
            romanization_lines: 0,
            translation_lines: 0,
        }
    }
}

/// The type scale the panel draws by, taken from the `Lyrics` global so both halves lay a sheet
/// out against one set of numbers.
#[derive(Clone, Copy)]
struct Metrics {
    font_size: f32,
    line_height: f32,
    romanization_font_size: f32,
    romanization_line_height: f32,
    romanization_gap: f32,
    translation_font_size: f32,
    translation_line_height: f32,
    translation_gap: f32,
    row_gap: f32,
    row_pad_y: f32,
}

impl Metrics {
    /// The scale as the `Lyrics` global declares it.
    ///
    /// Read once: every one of these is an `out` property with a literal behind it, so the panel
    /// and this cannot drift apart within a session however the sheet changes.
    fn from_global(global: &Lyrics) -> Self {
        Self {
            font_size: global.get_font_size(),
            line_height: global.get_line_height(),
            romanization_font_size: global.get_romanization_font_size(),
            romanization_line_height: global.get_romanization_line_height(),
            romanization_gap: global.get_romanization_gap(),
            translation_font_size: global.get_translation_font_size(),
            translation_line_height: global.get_translation_line_height(),
            translation_gap: global.get_translation_gap(),
            row_gap: global.get_row_gap(),
            row_pad_y: global.get_row_pad_y(),
        }
    }

    /// How tall a row's own box is: the words, and whichever of the two lines under them it has.
    ///
    /// Each part carries the gap above it, so a row of one, two or three comes out right with no
    /// branch per shape. The gap *between* rows is not in here — that is the layout's `spacing`,
    /// and it belongs to neither of the rows it separates. The vertical padding is, being what
    /// the hover fill covers.
    fn row_height(self, row: &Row) -> f32 {
        let mut height = 2.0 * self.row_pad_y + self.line_height * f32::from(row.lines);
        if row.romanization_lines > 0 {
            height += self.romanization_gap
                + self.romanization_line_height * f32::from(row.romanization_lines);
        }
        if row.translation_lines > 0 {
            height += self.translation_gap
                + self.translation_line_height * f32::from(row.translation_lines);
        }
        height
    }
}

/// What the panel is showing, and what is needed to follow it.
pub(crate) struct LyricsUi {
    rows: RefCell<Vec<Row>>,
    /// Cumulative top edge of each row in logical pixels, one entry per row.
    offsets: RefCell<Vec<f32>>,
    /// The sizes and line heights the panel draws by. Fixed for the session, unlike [`Self::width`]
    /// beside it, which follows the layout.
    metrics: Metrics,
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
    reseed: RefCell<Option<super::Seeder>>,
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

/// Install the panel's callbacks and seed the toggle.
pub(super) fn install(ui: &AppWindow, state: &AppState) -> Rc<LyricsUi> {
    let model: Rc<VecModel<LyricRow>> = Rc::new(VecModel::default());
    let global = ui.global::<Lyrics>();
    global.set_rows(ModelRc::from(model.clone()));

    let ly = Rc::new(LyricsUi {
        rows: RefCell::new(Vec::new()),
        offsets: RefCell::new(Vec::new()),
        metrics: Metrics::from_global(&global),
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
    let flags = settings_bind::read_or_default(state, "lyrics").lyrics;
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

    // Off the shadow rather than off `flags`, because the Settings card writes the same field and
    // may have moved it since this view last mounted.
    global.set_romanization_shown(state.lyrics_romanization_shown.get());
    {
        let persist = settings_bind::shadow_toggle(
            state,
            &state.lyrics_romanization_shown,
            "set_lyrics_romanization_shown",
            library::settings::set_lyrics_romanization_shown,
        );
        let weak = ui.as_weak();
        let ly_republish = ly.clone();
        global.on_set_romanization_shown(move |shown| {
            persist(shown);
            // Nothing is resolved again: the rows already carry the romanization, so the flip is a
            // re-measure of the sheet on screen against the heights it is now drawn at.
            if let Some(ui) = weak.upgrade() {
                follow::republish(&ui, &ly_republish);
                // **The Settings row is seeded once at boot and nothing re-reads it**, there being
                // no section gate on that page, so without this the card spends the rest of the
                // session showing what the flag was at launch.
                ui.global::<Settings>().set_lyrics_romanization_shown(shown);
            }
        });
    }

    // Off the shadow for the romanization row's reason, the Settings card owning the same field.
    global.set_online_enabled(state.lyrics_online_enabled.get());
    {
        let persist = settings_bind::shadow_toggle(
            state,
            &state.lyrics_online_enabled,
            "set_lyrics_online_enabled",
            library::settings::set_lyrics_online_enabled,
        );
        let weak = ui.as_weak();
        let ly_online = ly.clone();
        global.on_set_online_enabled(move |on| {
            persist(on);
            let Some(ui) = weak.upgrade() else { return };
            ui.global::<Settings>().set_lyrics_online_enabled(on);
            // **Switching it on is a reason to look this track up, and the claim has to go with
            // it**: `reseed` dedupes on the sheet it holds, which here is the empty answer the
            // switch just invalidated. Switching it off re-runs only where there is nothing to
            // lose, so an empty panel stops naming a lookup that can no longer run while a sheet
            // on screen is kept: the switch buys traffic rather than silence.
            if on || ly_online.rows.borrow().is_empty() {
                release(&ui, &ly_online);
                ly_online.kick();
            }
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
            follow::republish(&ui, &ly);
        });
    }

    {
        let weak = ui.as_weak();
        let ly = ly.clone();
        global.on_hover_at(move |y| {
            let Some(ui) = weak.upgrade() else { return };
            let found = follow::row_at(&ly.offsets.borrow(), &ly.rows.borrow(), ly.metrics, y);
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
            follow::follow(&ui, &ly);
        });
    }

    {
        let weak = ui.as_weak();
        let ly = ly.clone();
        global.on_seek_at(move |y| {
            let Some(ui) = weak.upgrade() else { return };
            follow::seek_at(&ui, &ly, y);
        });
    }

    ly
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
