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
use std::sync::Arc;
use std::time::Instant;

use async_compat::Compat;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use super::NowPlayingState;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::{LyricLine, Lyrics as Sheet, LyricsOutcome, LyricsSource};
use melodia_core::entities::track::TrackSummary;
use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::{AppWindow, LyricRow, Lyrics, LyricsState, Player};

/// Width to estimate against before the panel has reported its own.
///
/// Only ever wrong for one tick: the panel reports on both its cadences, so a real number lands
/// within a second of it mounting. Sized to the middle of `up-next-width`'s clamp so the first
/// draw is close rather than merely defined.
const ASSUMED_WIDTH: f32 = 340.0;

/// The space, the `i l t r f` family and the thin punctuation, which are around half of what a
/// Latin line is made of. See [`char_ems`] for what the four buckets are measured against.
const NARROW_EMS: f32 = 0.32;

/// `m w M W @ %`, the only characters Vazirmatn sets near an em.
const WIDE_EMS: f32 = 0.90;

/// A capital, which runs a fifth wider than the lowercase it sits in.
const UPPERCASE_EMS: f32 = 0.68;

/// Everything else drawn on a proportional em, letters of any script included.
const PROPORTIONAL_EMS: f32 = 0.57;

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

/// How long a sheet has to leave the singer quiet *between two lines* before the panel draws the
/// break.
///
/// **A floor rather than a preference, and what it holds off is scroll churn**: the panel glides
/// off the line above onto the notes and off them onto the line below, so a shorter break is three
/// targets inside a breath and reads as the panel losing its place rather than as the song resting.
const INTERLUDE_MS: i32 = 5_000;

/// The same for the run-in, which is lower because neither half of [`INTERLUDE_MS`]' argument
/// reaches it.
///
/// A gap with no line above it costs no scrolling: the panel mounts on it and leaves it once, which
/// is the one move it would have made anyway. And under its floor the run-in is the only gap that
/// leaves *nothing* lit, where a short break between two lines still leaves the line above it sung.
/// So all that is left to ask is whether the row stands long enough to read as deliberate rather
/// than as a flicker on mount.
const INTRO_MS: i32 = 3_000;

/// What a row is.
///
/// A gap is a row of the panel's own making rather than anything the sheet holds, which is why it
/// is spelled here and not in the sheet: how long a rest has to be before it is worth drawing is a
/// question about the panel, and the sheet has already answered the only one it can.
#[derive(Clone, Copy)]
enum RowKind {
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
    /// A line of the sheet. The three counts are [`republish`]'s to fill against the live width.
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

/// How wide a Hangul syllable that carries no final consonant is drawn: two ems, not one.
///
/// **Slint 1.16 sets those as two loose jamo rather than one block**, so `안` comes out square and
/// `아` comes out twice as wide as it is written. Charging both a square em under-measured a
/// Korean line by half, which is a wrap the panel never allowed room for: the words rode up over
/// the row above and the tail of the line was dropped. Retires when a Slint release shapes Hangul
/// through the composed glyph, and costs an early wrap in the meantime.
const OPEN_HANGUL_EMS: f32 = 2.0;

/// Whether the character is a Hangul syllable written without a final consonant.
///
/// The block is laid out initial-major, so a syllable's own index is a multiple of the number of
/// finals exactly when it has none.
fn is_open_hangul_syllable(ch: char) -> bool {
    /// The Hangul syllables block.
    const SYLLABLES: core::ops::RangeInclusive<u32> = 0xAC00..=0xD7A3;
    /// How many syllables share one initial and vowel, the final being what separates them.
    const FINALS: u32 = 28;

    let cp = u32::from(ch);
    SYLLABLES.contains(&cp) && (cp - SYLLABLES.start()).is_multiple_of(FINALS)
}

/// How wide one character is set, in ems.
///
/// **The two error directions are not symmetric.** Over-shooting wraps early and costs a blank
/// half-row; under-shooting elides the tail of a line, which loses words, so each bucket sits a
/// few percent above the widest character in it. What a bucket may not do is sit above the whole
/// alphabet, which the single averaged width it replaced did: set at the digit width, ordinary
/// prose measured a sixth wide and got a second row's worth of blank space under it.
fn char_ems(ch: char) -> f32 {
    if is_open_hangul_syllable(ch) {
        return OPEN_HANGUL_EMS;
    }
    if is_full_width(ch) {
        return 1.0;
    }
    match ch {
        'm' | 'w' | 'M' | 'W' | '@' | '%' => WIDE_EMS,
        ' ' | '!' | '"' | '\'' | '(' | ')' | ',' | '.' | ':' | ';' | 'I' | '[' | ']' | '`'
        | 'f' | 'i' | 'j' | 'l' | 'r' | 't' | '{' | '|' | '}' => NARROW_EMS,
        _ if ch.is_ascii_uppercase() => UPPERCASE_EMS,
        _ => PROPORTIONAL_EMS,
    }
}

/// How wide a line is set, in ems.
fn em_width(text: &str) -> f32 {
    text.chars().map(char_ems).sum()
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
            romanization_font_size: global.get_romanization_font_size(),
            romanization_line_height: global.get_romanization_line_height(),
            romanization_gap: global.get_romanization_gap(),
            translation_font_size: global.get_translation_font_size(),
            translation_line_height: global.get_translation_line_height(),
            translation_gap: global.get_translation_gap(),
            row_gap: global.get_row_gap(),
            row_pad_y: global.get_row_pad_y(),
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

    // Off the shadow rather than off `flags`, because the Settings card writes the same field and
    // may have moved it since this view last mounted.
    global.set_romanization_shown(state.lyrics_romanization_shown.get());
    {
        let state = state.clone();
        let weak = ui.as_weak();
        let ly_republish = ly.clone();
        global.on_set_romanization_shown(move |shown| {
            // Synchronously, before the write is spawned: the Settings card reads this cell to
            // seed its own row, and a disk write is not ordered against a sibling reading it.
            state.lyrics_romanization_shown.set(shown);
            state.persist_blocking("set_lyrics_romanization_shown", move |s| {
                library::settings::set_lyrics_romanization_shown(s, shown)
            });
            // Nothing is resolved again: the rows already carry the romanization, so the flip is a
            // re-measure of the sheet on screen against the heights it is now drawn at.
            if let Some(ui) = weak.upgrade() {
                republish(&ui, &ly_republish);
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
async fn fetch(state: &AppState, track: &TrackSummary) -> LyricsOutcome {
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
/// paint, and the artwork's already-applied guard has no way to know that. The three are a track
/// change, the view re-opening and the 3-dot toggle.
///
/// Idempotent by [`LyricsUi::holds`], so the edges may overlap freely: whichever gets there first
/// pays, and the claim is taken before the first `.await` rather than after the last.
fn reseed(weak: &Weak<AppWindow>, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let Some(ui) = weak.upgrade() else { return };
    let ly = &np_state.lyrics;

    let track = np_state.current_source.borrow().as_ref().and_then(|s| s.track.clone());
    // **Both terms, and `open` is the one that is not obvious.** `shown` is the persisted panel
    // preference rather than "the panel is mounted", so on its own it answers `true` for a closed
    // view, and the square miniplayer keeps the source-change path running behind one. That would
    // spend a request per track on a panel nobody can see.
    let wanted = np_state.open.get() && ui.global::<Lyrics>().get_shown();
    let Some(track) = track.filter(|_| wanted) else {
        // Closed, switched off, or a station, which has no words to look up. Either way the
        // previous song's sheet must not sit under it.
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

/// Wire the menu's two actions, which need the same `NowPlayingState` the reseed hook does.
///
/// **Both act on whatever is playing, read at click time rather than captured.** The menu is a
/// popup over a view that outlives any one track, so a handle taken at wire time names the wrong
/// song by the second verse.
pub(super) fn wire_menu(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let global = ui.global::<Lyrics>();

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let np_state = np_state.clone();
        global.on_refresh(move || {
            let Some(track) = playing_track(&np_state) else {
                return;
            };
            let Some(ui) = weak.upgrade() else { return };

            // Given up before the lookup, not after: the claim is what stops the reseed below
            // deduping this against the sheet it is meant to replace.
            release(&ui, &np_state.lyrics);

            let weak = weak.clone();
            let state = state.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                if let Err(e) = library::lyrics::forget(&state, &track.file_path).await {
                    log::warn!("lyrics refresh: {}", melodia_core::error::describe(&e));
                }
                if weak.upgrade().is_some() {
                    np_state.lyrics.kick();
                }
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing lyrics refresh spawn_local: {e}");
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let np_state = np_state.clone();
        global.on_save_to_tag(move || {
            let Some(track) = playing_track(&np_state) else {
                return;
            };
            let state = state.clone();
            let weak = weak.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                let found = library::lyrics::resident_text(&state, &track.file_path).await;
                let text = match found {
                    Ok(Some(text)) => text,
                    // The row is only offered against a sheet on screen, so nothing here is the
                    // store having lost the copy it was resolved from.
                    Ok(None) => {
                        log::debug!("lyrics save: nothing resident for {}", track.id);
                        return;
                    }
                    Err(e) => return report_save_failure(&e),
                };
                if let Err(e) = library::lyrics::write_to_tag(&state, track.id, &text).await {
                    return report_save_failure(&e);
                }
                toast::notify(ToastKind::LyricsSaved, track.title.clone());
                // The tag now holds what the panel is showing, so the row that offered this has
                // nothing left to do — and the sheet is the file's own from here.
                let Some(ui) = weak.upgrade() else { return };
                ui.global::<Lyrics>().set_can_save_to_tag(false);
                np_state.lyrics.holding.borrow_mut().take();
                np_state.lyrics.kick();
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing lyrics save spawn_local: {e}");
            }
        });
    }
}

/// Say a save did not happen, where it was asked for by name.
///
/// **A toast rather than a log line**, the radio vote's rule: this runs only because somebody
/// pressed a control that says it writes to their file, and a control that quietly does nothing is
/// worse than one that says why.
fn report_save_failure(e: &melodia_core::error::AppError) {
    let reason = melodia_core::error::describe(e);
    log::warn!("lyrics save: {reason}");
    toast::notify(ToastKind::OperationFailed, reason);
}

/// The track on the deck, or `None` on a station or an empty queue.
fn playing_track(np_state: &Rc<NowPlayingState>) -> Option<Arc<TrackSummary>> {
    np_state.current_source.borrow().as_ref().and_then(|s| s.track.clone())
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
fn apply(
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
            // Every source but the file's own tag is worth offering to write into it. A sidecar
            // counts: it is the user's file, but it is not the one that travels with the track.
            global.set_can_save_to_tag(sheet.source != LyricsSource::Tag);
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
        // **The claim above is still recorded, deliberately.** Nothing retries on its own, so
        // without it every tick that reaches `reseed` would start another lookup against a service
        // that has just told us to stop. Refresh is the retry, and it drops the claim itself.
        LyricsOutcome::Unavailable => {
            clear(ui, ly);
            global.set_state(LyricsState::Unavailable);
        }
    }
}

/// Say a lookup is out, so the panel is not claiming "not found" while it is still looking.
///
/// **Claims the track before the `.await`, not after it.** The lookup can reach a file and then a
/// socket, and a reseed landing in that window would otherwise see a sheet for the *previous*
/// track and start a second one for the same song.
fn mark_loading(ui: &AppWindow, ly: &Rc<LyricsUi>, track_path: &str) {
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
    global.set_can_save_to_tag(false);
    global.set_active_index(-1);
    global.set_active_offset(0.0);
    global.set_active_height(0.0);
}

/// Keep the sheet's text and stamps. Heights are not kept, following the width instead.
fn take_sheet(ly: &Rc<LyricsUi>, sheet: &Sheet) {
    *ly.rows.borrow_mut() = rows_for(sheet);
}

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
fn rows_for(sheet: &Sheet) -> Vec<Row> {
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
fn millis(at_ms: i64) -> i32 {
    i32::try_from(at_ms).unwrap_or(i32::MAX)
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
    // **Read here rather than filtered at the source**, so the toggle and the offset table are one
    // answer: a row whose romanization is suppressed has to be shorter by exactly what the panel
    // stops drawing, and both halves read this same pass's decision.
    let romanization_shown = ui.global::<Lyrics>().get_romanization_shown();

    let mut rows = ly.rows.borrow_mut();
    let mut offsets = Vec::with_capacity(rows.len());
    let mut published = Vec::with_capacity(rows.len());
    let mut top = 0.0_f32;

    for row in rows.iter_mut() {
        row.lines = wrapped_lines(&row.text, width, metrics.font_size);
        // Neither of these goes through `wrapped_lines`' floor of one, which is for a blank line a
        // plain sheet spaces its verses with. A row without one has no slot for it at all.
        row.romanization_lines = row
            .romanization
            .as_deref()
            .filter(|_| romanization_shown)
            .map_or(0, |sound| wrapped_lines(sound, width, metrics.romanization_font_size));
        row.translation_lines = row
            .translation
            .as_deref()
            .map_or(0, |gloss| wrapped_lines(gloss, width, metrics.translation_font_size));

        offsets.push(top);
        // The layout's own `spacing`, which sits between rows rather than inside one.
        top += metrics.row_height(row) + metrics.row_gap;
        published.push(LyricRow {
            text: SharedString::from(row.text.as_str()),
            romanization: row
                .romanization
                .as_deref()
                .filter(|_| romanization_shown)
                .map(SharedString::from)
                .unwrap_or_default(),
            translation: row.translation.as_deref().map(SharedString::from).unwrap_or_default(),
            at_ms: row.at_ms.unwrap_or(-1),
            is_interlude: matches!(row.kind, RowKind::Interlude { .. }),
            line_count: i32::from(row.lines),
            romanization_line_count: i32::from(row.romanization_lines),
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
    // The seek's own instant, so a click landing on a gap starts its notes empty rather than
    // wherever the song happened to be when the pointer went down.
    write_active(ui, ly, Some(index), f64::from(at_ms));
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

/// The last row whose stamp has passed, by binary search over the rows.
///
/// **A sheet is timed or it is not**, which the parser guarantees and the first row is enough to
/// ask: on a timed one every row carries a stamp, so the search is the whole slice and needs no
/// index of the stamped ones to walk. Worth the ask rather than the allocation, the panel calling
/// this on a 33 ms tick.
fn sung_at(rows: &[Row], position_ms: f64) -> Option<usize> {
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
    global.set_active_height(ly.metrics.get().row_height(row));
    global.set_interlude_progress(interlude_progress(row, position_ms));
}

/// How far through a gap the song is, for the notes to fill against. Zero on a row of words, which
/// have nothing to fill.
fn interlude_progress(row: &Row, position_ms: f64) -> f32 {
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

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
