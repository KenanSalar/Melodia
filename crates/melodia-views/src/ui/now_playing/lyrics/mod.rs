//! The lyrics panel's Rust half.
//!
//! **Every position is milliseconds in an `i32`**, as the `Player` global publishes and the rows
//! carry: it reaches past three weeks of one track and converts into `f64` without loss.

mod follow;
mod measure;
mod rows;
mod sheet;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::NowPlayingState;
use crate::ui::settings::lyrics_settings;
use melodia_app::state::AppState;
use melodia_app::tasks;
use melodia_core::entities::lyrics::LyricLine;
use melodia_ui::{AppWindow, LyricRow, Lyrics};

pub(super) use sheet::{release, wire_menu, wire_reseed};

/// Until the panel reports its own, within a second of mounting. Mid `up-next-width`'s clamp, so
/// the first draw is close.
const ASSUMED_WIDTH: f32 = 340.0;

#[derive(Clone, Copy)]
enum RowKind {
    Words,
    /// Carries where the gap ends, the row's own `at_ms` being where it starts, so the notes have
    /// both ends of the span they fill across.
    Interlude {
        until_ms: i32,
    },
}

/// One row as the panel draws it. **The texts are `SharedString`**, so publishing a row on every
/// re-measure is a refcount bump rather than a resident second copy of the sheet.
struct Row {
    kind: RowKind,
    /// `None` on an untimed sheet, the two never mixing within one sheet.
    at_ms: Option<i32>,
    text: SharedString,
    /// How the words sound, for a script that does not spell it. `None` for a Latin line.
    romanization: Option<SharedString>,
    /// The gloss a bilingual sheet carries under the words.
    translation: Option<SharedString>,
    /// Wrapped line count, and so this row's height in `Lyrics.line-height` units.
    lines: u8,
    /// The same for the romanization, in `Lyrics.romanization-line-height` units. Zero without
    /// one, and zero for every row while the toggle is off.
    romanization_lines: u8,
    /// The same for the gloss, in `Lyrics.translation-line-height` units. Zero without one.
    translation_lines: u8,
}

impl Row {
    /// The three counts are [`follow::republish`]'s to fill against the live width.
    fn words(line: &LyricLine, at_ms: Option<i32>) -> Self {
        Self {
            kind: RowKind::Words,
            at_ms,
            text: SharedString::from(line.text.as_str()),
            romanization: line.romanization.as_deref().map(SharedString::from),
            translation: line.translation.as_deref().map(SharedString::from),
            lines: 1,
            romanization_lines: 0,
            translation_lines: 0,
        }
    }

    /// The quiet between two lines, drawn as the notes. Blank text floors at one wrapped line, so
    /// the row is a one-line verse's height and [`Metrics::row_height`] needs no arm for it.
    fn interlude(from_ms: i32, until_ms: i32) -> Self {
        Self {
            kind: RowKind::Interlude { until_ms },
            at_ms: Some(from_ms),
            text: SharedString::new(),
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
    /// Read once: each is an `out` property over a literal, so the two halves can't drift apart
    /// within a session.
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

    /// Each part carries the gap above it, so no shape needs a branch. The gap between rows is the
    /// layout's `spacing`; the vertical padding is in, being what the hover fill covers.
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
    metrics: Metrics,
    width: Cell<f32>,
    /// Which way the romanization toggle stood when the model was last written, so a re-measure
    /// can tell a flip from a width that moved and publish only for the former.
    published_romanization: Cell<bool>,
    /// The last position the bridge published, and when it was seen. Together they interpolate a
    /// roughly one-second tick into something a highlight can follow.
    anchor_ms: Cell<i32>,
    anchor_at: Cell<Instant>,
    /// A clicked row, held until the clock catches up with it.
    pinned: Cell<Option<usize>>,
    pinned_at_ms: Cell<i32>,
    /// The track path the resident sheet belongs to, `None` once the rows are handed back. **This
    /// makes the reseed idempotent**, so the three edges reaching it cost one lookup between them.
    holding: RefCell<Option<String>>,
    /// Filled after [`install`] returns, needing the `NowPlayingState` this is a field of.
    reseed: RefCell<Option<super::Hook>>,
    model: Rc<VecModel<LyricRow>>,
}

impl LyricsUi {
    /// Looks a sheet up for what is playing, unless the panel already holds it.
    pub(super) fn kick(&self) {
        if let Some(reseed) = self.reseed.borrow().as_ref() {
            reseed();
        }
    }

    fn holds(&self, path: &str) -> bool {
        self.holding.borrow().as_deref() == Some(path)
    }
}

pub(super) fn install(ui: &AppWindow, state: &AppState) -> Rc<LyricsUi> {
    let model: Rc<VecModel<LyricRow>> = Rc::new(VecModel::default());
    let global = ui.global::<Lyrics>();
    global.set_rows(ModelRc::from(model.clone()));

    let ly = Rc::new(LyricsUi {
        rows: RefCell::new(Vec::new()),
        offsets: RefCell::new(Vec::new()),
        metrics: Metrics::from_global(&global),
        width: Cell::new(ASSUMED_WIDTH),
        published_romanization: Cell::new(state.lyrics_romanization_shown.get()),
        anchor_ms: Cell::new(0),
        anchor_at: Cell::new(Instant::now()),
        pinned: Cell::new(None),
        pinned_at_ms: Cell::new(0),
        holding: RefCell::new(None),
        reseed: RefCell::new(None),
        model,
    });

    wire_toggles(ui, state, &ly);
    wire_panel_input(ui, &ly);

    ly
}

/// The header's switch and the two menu rows the Settings card carries as well.
fn wire_toggles(ui: &AppWindow, state: &AppState, ly: &Rc<LyricsUi>) {
    let global = ui.global::<Lyrics>();

    // Off the shadows, the Settings card writing the same fields. Every handler is the card's, so
    // the two controls of a switch can't drift; what's left here is what each owes the panel.
    global.set_enabled(state.lyrics_enabled.get());
    {
        let persist = lyrics_settings::enabled_handler(ui, state);
        let state = state.clone();
        let ly_toggle = ly.clone();
        global.on_set_enabled(move |on| {
            persist(on);
            // On, a sheet is looked up now rather than at the next track; off, the reseed releases
            // what it holds.
            ly_toggle.kick();
            // The store stops growing here just as it does on a close, which skips its own prune
            // while lyrics are off.
            if !on {
                tasks::lyrics_cache::spawn(&tasks::TaskSpawner::from_state(&state), &state);
            }
        });
    }

    global.set_romanization_shown(state.lyrics_romanization_shown.get());
    {
        let persist = lyrics_settings::romanization_handler(ui, state);
        let weak = ui.as_weak();
        let ly_republish = ly.clone();
        global.on_set_romanization_shown(move |shown| {
            persist(shown);
            // Nothing is resolved again: the rows already carry the romanization, so the flip is a
            // re-measure of the sheet on screen against the heights it is now drawn at.
            if let Some(ui) = weak.upgrade() {
                follow::republish(&ui, &ly_republish);
            }
        });
    }

    global.set_online_enabled(state.lyrics_online_enabled.get());
    {
        let persist = lyrics_settings::online_handler(ui, state);
        let weak = ui.as_weak();
        let ly_online = ly.clone();
        global.on_set_online_enabled(move |on| {
            persist(on);
            let Some(ui) = weak.upgrade() else { return };
            // On, the claim goes too: `reseed` dedupes on the sheet it holds, here the empty answer
            // the switch just invalidated. Off re-runs only an empty panel, so it stops naming a
            // lookup that can no longer run while a sheet on screen is kept.
            if on || ly_online.rows.borrow().is_empty() {
                release(&ui, &ly_online);
                ly_online.kick();
            }
        });
    }
}

/// What the sheet reports back: the width it was laid out at, the pointer over it, the follow
/// tick, and a click asking to be taken to a line.
fn wire_panel_input(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    let global = ui.global::<Lyrics>();

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
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
