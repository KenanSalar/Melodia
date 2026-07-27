//! Wire the Now-Playing audio visualizer to Rust.
//!
//! Seeds the `Visualizer` global from `settings.json` at startup and owns the
//! four callbacks. The per-frame analysis lives in [`frame`]; this module is
//! wiring, and the tick handler here is the dispatcher between the two styles.
//!
//! The contract spanning this directory, the `.slint` strip and both style
//! pickers is `.claude/rules/visualizer.md`. It isn't a `CLAUDE.md` beside this
//! code because most of what it governs lives under `ui/`.
//!
//! A 2048-point real FFT is sub-millisecond, so the render loop stays on the UI
//! thread rather than paying for a third thread and a second shared cell. Most
//! of what keeps that cheap composes for free from the mount tree: the strip
//! only mounts while the visualizer is enabled, and the Now-Playing view only
//! mounts while it's open, so a closed view costs nothing at all.
//!
//! # Arming the tap
//!
//! The mount tree can't gate the *producer* — the audio thread would happily
//! keep filling the ring for a view nobody has open, and for a window hidden to
//! tray. So the persisted setting and the tap's arm state are deliberately two
//! different things: `set-enabled` (the Settings → Playback toggle) only
//! persists, and this module is the sole writer of the arm state, from three
//! places on the UI thread:
//!
//! - `set-active` — the mount/unmount boundary, mirrored out of `AppWindow`.
//! - `tick` — the steady state, which folds in pause and window visibility.
//! - `window-hidden` — the one case the tick can't cover, because the same
//!   signal stops the Timer it runs off.
//!
//! Between them the tap is armed while something is drawing it, with one benign
//! exception: a pause landing on an already-settled drawing stops the Timer the
//! same way a hide does, and nothing disarms the tap. It costs nothing, which is
//! why it isn't worth a fourth writer — a paused deck pulls no samples, so an
//! armed tap writes nothing, and what the rings hold is the silence that settled
//! the drawing in the first place, so the history the resume doesn't drop is
//! silence too.
//!
//! The style has no say in any of it: every style reads the same ring.
//!
//! # Off-screen windows
//!
//! Nor can the mount tree gate a window that is still *open* but not being
//! shown: Slint `Timer`s fire off the event loop, not the render loop, and the
//! loop deliberately stays alive through a close-to-tray hide. Two signals cover
//! that, and the tick keeps them apart because they carry different weight:
//!
//! - [`tray_bridge::is_window_visible`] — the OS said so, and something will say
//!   so again when the window comes back. Safe to stop the Timer on, which the
//!   `window-shown` half of its `running` gate does.
//! - [`pulse::frames`] — nobody said anything, but the window has stopped being
//!   painted. Inferred, so it silences the expensive half and slows the Timer
//!   down (`dormant`) rather than stopping it.

mod frame;
mod pulse;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::library;
use crate::player::spectrum::{FFT_SIZE, NUM_BANDS, SpectrumAnalyzer};
use crate::player::visualizer::RING_CAP;
use crate::player::waveform::{self, MAX_COLUMNS, WaveformAnalyzer};
use crate::state::AppState;
use crate::ui::settings_bind::read_or_default;
use crate::ui::tray_bridge;
use crate::{AppWindow, Visualizer};

const STYLE_BARS: &str = "bars";
const STYLE_WAVEFORM: &str = "waveform";
const STYLE_MIRRORED: &str = "mirrored";

/// Ticks without a painted frame before the tick concludes nobody is looking.
///
/// Counted in ticks rather than measured against a clock, and that is the whole
/// point: the Timer stops once a paused player's drawing has settled, so a clock
/// would come back from any long pause reading "no frames in minutes" — when all
/// that happened is that nobody was looking. A tick that didn't happen accrues
/// nothing.
///
/// Sixty is ~1 s of bars or ~2 s of trace, both deliberately generous. A stall
/// that long on a window that really is on screen is already a severe hitch, and
/// standing down late costs nothing — where a false trip re-arms the tap, which
/// drops the rings' history and leaves the drawing ramping back in from silence.
const FRAME_STALL_TICKS: u32 = 60;

/// The two flags a strip at rest carries, shared by the pair that has to agree
/// on them: [`publish_resting`] writes them to the global, and [`Analyzers::new`]
/// seeds its shadows to the same values so a fresh session doesn't re-publish
/// what is already there.
///
/// Shared rather than spelled out twice and pinned by a test — the way
/// [`DEFAULT_VIZ_STYLE`](crate::services::settings::DEFAULT_VIZ_STYLE) and
/// [`STYLES`] are — because one of the two sites is a write into a Slint global,
/// which no test here can reach. Drift is silent and one-directional: if the
/// global is left holding one value while the shadows say the other, the tick's
/// `idle != session.was_idle` gate skips the publish and the global keeps the
/// stale flag for the whole session.
const RESTING_IDLE: bool = true;
const RESTING_DORMANT: bool = false;

/// One drawing session: the buffers the per-frame analysis must not rebuild, and
/// the shadows tracking what it last published.
///
/// Built on the first tick after the strip mounts and dropped when it leaves, so
/// a user who never opens Now Playing never pays for the FFT plans — and so each
/// session's shadows start at the resting values `set-active` publishes on the
/// way out, without anyone having to reset them one by one.
struct Analyzers {
    spectrum: SpectrumAnalyzer,
    wave: WaveformAnalyzer,
    /// The trace's path commands. Sized once for the widest trace — two vertices
    /// a column, ~17 bytes a vertex — so the per-frame rebuild only ever writes
    /// into capacity it already has.
    path: String,
    /// Shadows `Visualizer.idle` / `Visualizer.dormant`, so an unchanged value
    /// doesn't dirty the property sixty times a second.
    was_idle: bool,
    was_dormant: bool,
    frames: FrameWatch,
}

impl Analyzers {
    fn new() -> Self {
        Self {
            spectrum: SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS),
            // A window wider than the ring would only be padded with silence, so
            // the ring's capacity is the analyzer's too.
            wave: WaveformAnalyzer::new(RING_CAP, MAX_COLUMNS),
            path: String::with_capacity(MAX_COLUMNS * 2 * 20),
            was_idle: RESTING_IDLE,
            was_dormant: RESTING_DORMANT,
            frames: FrameWatch::new(),
        }
    }
}

/// How long the painted-frame count has stood still, in ticks.
struct FrameWatch {
    last: u64,
    stalled_ticks: u32,
}

impl FrameWatch {
    /// Seeded from the count as it stands, so a session always opens believing it
    /// is being drawn.
    fn new() -> Self {
        Self {
            last: pulse::frames().unwrap_or_default(),
            stalled_ticks: 0,
        }
    }

    /// Whether the window has painted recently enough to be worth drawing for.
    ///
    /// Every tick dirties a property, so a window being shown paints on every one
    /// of them and a window that isn't leaves the count where it was. `None` means
    /// nothing is counting: no evidence either way, and an inference with none
    /// behind it must not be what blanks the strip.
    fn painting(&mut self, frames: Option<u64>) -> bool {
        let Some(frames) = frames else {
            return true;
        };
        if frames == self.last {
            self.stalled_ticks = self.stalled_ticks.saturating_add(1);
        } else {
            self.last = frames;
            self.stalled_ticks = 0;
        }
        self.stalled_ticks < FRAME_STALL_TICKS
    }
}

/// Style keys in picker order.
///
/// Two things mirror this **by position** and neither can be checked by the
/// compiler: the `viz-style-names` `@tr` array in
/// `ui/components/now-playing/flyout-presets.slint` — rendered by both the
/// Settings chips and the Now-Playing view's flyout — and the branch in
/// `visualizer-strip.slint` that mounts on the key. `tests/visualizer_tests.rs`
/// pins both against this array.
///
/// A third mirror is [`DEFAULT_VIZ_STYLE`], which has to be the key at index 0 —
/// where both of the fallbacks below land. It is spelled out separately rather
/// than substituted in here on purpose: the test that pins the two together can
/// only fail if they are two literals.
const STYLES: [&str; 3] = [STYLE_BARS, STYLE_MIRRORED, STYLE_WAVEFORM];

/// Picker index for a persisted key. An unrecognized key — a hand-edited file,
/// or one written by a newer build — falls back to the default style rather
/// than leaving the strip blank.
fn style_index(key: &str) -> usize {
    STYLES.iter().position(|&style| style == key).unwrap_or(0)
}

/// Picker index for the index a picker sent. Out of range can only come from a
/// picker that has drifted out of step with [`STYLES`], and it answers the same
/// way an unknown key does.
fn style_index_from_i32(index: i32) -> usize {
    usize::try_from(index)
        .ok()
        .filter(|&i| i < STYLES.len())
        .unwrap_or(0)
}

/// Whether an index selects the waveform. The remaining two styles are the same
/// bars under a different anchor, and Slint resolves that from the key.
fn is_waveform(index: usize) -> bool {
    STYLES.get(index).copied() == Some(STYLE_WAVEFORM)
}

/// Publish both halves of the style: the key the strip mounts on and the index
/// the pickers bind to.
fn publish_style(global: &Visualizer, index: usize) {
    global.set_style(SharedString::from(STYLES[index]));
    global.set_style_idx(i32::try_from(index).unwrap_or_default());
}

/// Publish the drawing a strip nobody is watching mounts on: every band at rest
/// and the flat figure a decayed trace settles to, with the two flags that say
/// so.
///
/// Called at boot and again whenever a session ends, and the second call is the
/// load-bearing one. The strip's Timer runs on
/// `(playing && window-shown) || !idle`, so a strip remounting over a *paused*
/// player never fires a tick — and whatever the last session left in these two
/// properties would sit there until playback resumed. Closing Now Playing
/// destroys the view, so unlike a hide-to-tray there is no decay pass to reach
/// rest on its own.
///
/// Both halves of the drawing are built by the same code the live path uses, so
/// neither can drift from what a decay actually lands on; the tests pin that.
/// The property writes themselves are not covered — reaching a `Visualizer`
/// global means instantiating `AppWindow`, and with `renderer-software` off
/// that needs a hand-stubbed `Platform`/`WindowAdapter`/`Renderer` for the whole
/// component tree.
fn publish_resting(global: &Visualizer, model: &VecModel<f32>) {
    rest_bars(model);
    global.set_wave_path(SharedString::from(resting_wave_path().as_str()));
    global.set_idle(RESTING_IDLE);
    global.set_dormant(RESTING_DORMANT);
}

/// Put every band back at the level the model was seeded with. The strip floors
/// each one at a visible dot, so rest for the bars is the seed, not a height.
fn rest_bars(model: &VecModel<f32>) {
    for band in 0..model.row_count() {
        model.set_row_data(band, 0.0);
    }
}

/// The trace's resting figure, through its real writer rather than as a literal
/// so it is exactly what a decay settles to. Two columns is the narrowest input
/// that still describes a span.
fn resting_wave_path() -> String {
    let mut path = String::new();
    waveform::write_path_commands(&[waveform::Column::default(); 2], &mut path);
    path
}

pub fn install_visualizer(ui: &AppWindow, state: &AppState) {
    // `enabled` only decides whether the strip mounts — the backend tap stays
    // disarmed until `set-active` says a mounted strip is on screen, so an
    // unreadable file can't leave the two out of step.
    let flags = read_or_default(state, "visualizer").visualizer;
    let selected = style_index(&flags.viz_style);

    // Backing model for the `bars` `[float]` global property. Kept here (cloned
    // into the tick handler) so every frame mutates the same model the strip
    // reads, rather than replacing the property with a fresh one.
    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(vec![0.0; NUM_BANDS]));

    // The active style, read every tick and written by `set-style`. A shadow
    // rather than a read back off the global: it keeps a string compare out of
    // the 16 ms tick, and both ends live on the UI thread.
    let style: Rc<Cell<usize>> = Rc::new(Cell::new(selected));

    // The session's buffers, shared between the tick that builds and uses them
    // and the `set-active` that drops them. Both run on the UI thread, and the
    // tick's borrow never outlives one frame.
    let analyzers: Rc<RefCell<Option<Analyzers>>> = Rc::new(RefCell::new(None));

    let viz_global = ui.global::<Visualizer>();
    viz_global.set_enabled(flags.viz_enabled);
    publish_style(&viz_global, selected);
    viz_global.set_bars(ModelRc::from(model.clone()));

    // The bars would come up at rest on their own — the model is seeded with a
    // level per band and each floors at a dot — but the trace has nothing to
    // fall back on, and its Timer doesn't run until something plays, so a
    // Now-Playing view opened on a freshly started app would show an empty
    // strip. The same call is what a session ends on; see `publish_resting`.
    publish_resting(&viz_global, &model);

    // tick — dispatch one frame. The session's analyzers (the FFT plans and
    // their buffers, the Hann tables, the bin→band map, the smoothing state, the
    // sample window and the trace) are built on the first frame and reused
    // across ticks; nothing here allocates except the one `SharedString` the
    // waveform path has to be handed to Slint as.
    {
        let viz = state.rodio.visualizer();
        let model = model.clone();
        let style = style.clone();
        let analyzers = analyzers.clone();
        let weak = ui.as_weak();

        viz_global.on_tick(move |playing, strip_width| {
            let mut slot = analyzers.borrow_mut();
            // The one construction site, so no mount ordering can leave the tick
            // without buffers however `set-active` and the strip interleave.
            let session = slot.get_or_insert_with(Analyzers::new);

            // A pause leaves the last window of audio sitting in the ring, so
            // re-analysing it would freeze the drawing on a stale shape instead
            // of letting it fall. A window that isn't on screen has nothing to
            // draw for at all — see the module docs for why that takes two
            // signals rather than one. Computed rather than returned early on:
            // the decay path below still has to run, so `idle` stays truthful
            // and the Timer can still stop.
            let painting = session.frames.painting(pulse::frames());
            let analyzing = playing && painting && tray_bridge::is_window_visible();

            // The steady-state writer of the tap's arm state, so pause, minimise
            // and hide-to-tray all silence the producer as well as the analysis.
            // Arming *discards* whatever the rings still hold — samples that may
            // predate the pause by minutes — so the drawing ramps back in from
            // silence over one window rather than resuming on a stale shape.
            viz.set_enabled(analyzing);

            // Not `sample_rate()`: the tap sits under rodio's speed stage, so
            // the analysis rate has to fold the speed back in. Zero is the "draw
            // nothing new" signal both styles decay on.
            let rate = if analyzing { viz.analysis_rate() } else { 0 };

            let waveform = is_waveform(style.get());
            let idle = if waveform {
                frame::waveform(&viz, &mut session.wave, &mut session.path, rate, strip_width)
            } else {
                frame::bars(&viz, &mut session.spectrum, &model, rate)
            };
            // Settled, and nothing arriving to unsettle it: the tick is only
            // still here to watch for frames, which it can do far more slowly.
            let dormant = idle && !painting;

            // The bars' levels ride their own model, which the loop above has
            // already written; only the trace and the two flags come through
            // here. Both flags drive the strip's Timer, so they can't be
            // skipped — but they change twice a track, not sixty times a second.
            if waveform || idle != session.was_idle || dormant != session.was_dormant {
                if let Some(ui) = weak.upgrade() {
                    let global = ui.global::<Visualizer>();
                    // Only the mounted style's property is written — the other
                    // one's consumer isn't in the tree to read it.
                    if waveform {
                        global.set_wave_path(SharedString::from(session.path.as_str()));
                    }
                    global.set_idle(idle);
                    global.set_dormant(dormant);
                }
                session.was_idle = idle;
                session.was_dormant = dormant;
            }
        });
    }

    // set-active — the mount/unmount boundary. Closing Now Playing (or turning
    // the setting off with it open) unmounts the strip, which stops `tick`, so
    // something outside the strip has to disarm the tap on the way out. On the
    // way in it arms optimistically and the next tick refines it.
    //
    // The session's buffers leave with it. They are the whole of the feature's
    // resident footprint — two FFT plans with their windows, spectra and
    // scratch, the trace's window and its path string — and a strip nobody is
    // drawing has no use for any of it.
    {
        let viz = state.rodio.visualizer();
        let analyzers = analyzers.clone();
        let model = model.clone();
        let weak = ui.as_weak();
        viz_global.on_set_active(move |active| {
            viz.set_enabled(active && tray_bridge::is_window_visible());
            if active {
                // Where the frame counter starts. It isn't free — see `pulse` —
                // so it waits here for a strip that will actually read it.
                if let Some(ui) = weak.upgrade() {
                    pulse::install(&ui);
                }
                return;
            }
            *analyzers.borrow_mut() = None;
            // The strip mounts on whatever the last session left in these, and
            // nothing here will overwrite them: a strip remounting over a paused
            // player never ticks. So hand back the same rest a fresh `Analyzers`
            // is shadowing — the drawing as well as the two flags, or the next
            // open comes up on the frame this one ended on.
            if let Some(ui) = weak.upgrade() {
                publish_resting(&ui.global::<Visualizer>(), &model);
            }
        });
    }

    // window-hidden — the third writer of the arm state, and the reason the tick
    // isn't enough on its own. `window-shown` gates the strip's Timer, so a hide
    // that lands on an already-settled drawing (digital silence, or the first
    // frames of a track while the window refills) stops the Timer in the same
    // pass as it drops the gate: `Timer::stop` takes effect at once, no final
    // trigger fires, and the tick that would have disarmed the tap never runs.
    {
        let viz = state.rodio.visualizer();
        viz_global.on_window_hidden(move || viz.set_enabled(false));
    }

    // set-enabled — persist only. Arming is the mirrored `set-active` above:
    // flipping the setting moves `AppWindow.watched-viz-active`, so routing it
    // through here as well would arm the tap for a view that isn't open. No
    // write-back to `enabled` either — the Settings → Playback toggle two-way
    // binds it, so it has already landed by the time this fires.
    {
        let state = state.clone();
        viz_global.on_set_enabled(move |on| {
            state.persist_blocking("persist viz_enabled", move |s| {
                library::settings::set_visualizer_enabled(s, on)
            });
        });
    }

    // set-style — resolve the picker's index to a key, publish both, persist.
    // The chips two-way bind `style-idx`, so that half has already landed; the
    // key is what the strip mounts on, and the shadow is what the tick reads.
    {
        let state = state.clone();
        let style = style.clone();
        let weak = ui.as_weak();
        viz_global.on_set_style(move |index| {
            let picked = style_index_from_i32(index);
            style.set(picked);
            if let Some(ui) = weak.upgrade() {
                publish_style(&ui.global::<Visualizer>(), picked);
            }
            let key = STYLES[picked].to_owned();
            state.persist_blocking("persist viz_style", move |s| {
                library::settings::set_visualizer_style(s, key)
            });
        });
    }
}

#[cfg(test)]
#[path = "tests/visualizer_tests.rs"]
mod tests;
