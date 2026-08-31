//! Wire the Now-Playing audio visualizer to Rust.
//!
//! Seeds the `Visualizer` global from `settings.json` and owns the four callbacks; the
//! per-frame analysis is [`frame`]. The contract spanning this directory, the `.slint`
//! strip and both style pickers is `.claude/rules/visualizer.md`.
//!
//! The bars frame runs two real FFTs and stays comfortably sub-millisecond, so the render loop
//! sits on the UI thread rather than paying for a third thread and a second shared cell.
//!
//! What the mount tree *can't* gate is the producer, so this module is the sole writer of the
//! tap's arm state, from three places on the UI thread: `set-active` (the mount boundary), `tick`
//! (the steady state) and `window-hidden` (the one case the tick can't cover, the same signal
//! stopping the Timer it runs off). One benign gap: a pause landing on an already-settled drawing
//! stops the Timer the way a hide does and nothing disarms the tap, which costs nothing.
//!
//! Nor can it gate a window still *open* but not being shown, Slint `Timer`s firing off an event
//! loop that survives a close-to-tray hide. The two signals stay apart because they carry
//! different weight: [`tray_bridge::is_window_visible`] is the OS saying so and safe to stop the
//! Timer on, where [`pulse::frames`] is inferred and only slows it.

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
use crate::ui::shell::tray_bridge;
use crate::{AppWindow, Visualizer};

const STYLE_BARS: &str = "bars";
const STYLE_WAVEFORM: &str = "waveform";
const STYLE_MIRRORED: &str = "mirrored";

/// Ticks without a painted frame before the tick concludes nobody is looking. Ticks rather than a
/// clock, so a long pause — which stops the Timer entirely — doesn't come back reading "no frames
/// in minutes". Deliberately generous: a false trip re-arms the tap and leaves the drawing ramping
/// back in from silence.
const FRAME_STALL_TICKS: u32 = 60;

/// The two flags a strip at rest carries, shared by [`publish_resting`] and [`Analyzers::new`]
/// rather than spelled twice: drift is silent, the tick's `idle != session.was_idle` gate skipping
/// the publish so a stale flag stands for the whole session.
const RESTING_IDLE: bool = true;
const RESTING_DORMANT: bool = false;

/// One drawing session: the buffers the per-frame analysis must not rebuild, plus the shadows
/// tracking what it last published. Built on the first tick after the strip mounts and dropped
/// when it leaves, so a user who never opens Now Playing never pays for the FFT plans.
struct Analyzers {
    spectrum: SpectrumAnalyzer,
    wave: WaveformAnalyzer,
    /// Sized once for the widest trace, so the per-frame rebuild only writes into capacity it
    /// already has.
    path: String,
    /// Shadows `Visualizer.idle` / `Visualizer.dormant`, so an unchanged value doesn't dirty the
    /// property sixty times a second.
    was_idle: bool,
    was_dormant: bool,
    frames: FrameWatch,
}

impl Analyzers {
    fn new() -> Self {
        Self {
            spectrum: SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS),
            // A wider window would only be padded with silence.
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
    /// Seeded from the count as it stands, so a session opens believing it is drawn.
    fn new() -> Self {
        Self {
            last: pulse::frames().unwrap_or_default(),
            stalled_ticks: 0,
        }
    }

    /// Whether the window has painted recently enough to be worth drawing for.
    ///
    /// Every tick dirties a property, so a shown window paints on every one and a window that
    /// isn't leaves the count where it was. `None` means nothing is counting — no evidence either
    /// way, and that must not be what blanks the strip.
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

/// Style keys in picker order. Two things mirror this **by position** and the compiler checks
/// neither: the `viz-style-names` `@tr` array in `components/now-playing/flyout-presets.slint`,
/// and the branch in `visualizer-strip.slint` that mounts on the key. A third mirror is
/// [`crate::services::settings::DEFAULT_VIZ_STYLE`], which has to be the key at index 0 — where
/// both fallbacks below land — and stays a separate literal, a test pinning the two together only
/// being able to fail if they are two.
const STYLES: [&str; 3] = [STYLE_BARS, STYLE_MIRRORED, STYLE_WAVEFORM];

/// Picker index for a persisted key. An unrecognized key — a hand-edited file, or one written by a
/// newer build — takes the default rather than leaving the strip blank.
fn style_index(key: &str) -> usize {
    STYLES.iter().position(|&style| style == key).unwrap_or(0)
}

/// Picker index for the index a picker sent, out of range meaning a picker that has drifted out of
/// step with [`STYLES`].
fn style_index_from_i32(index: i32) -> usize {
    usize::try_from(index).ok().filter(|&i| i < STYLES.len()).unwrap_or(0)
}

/// Whether an index selects the waveform. The other two are the same bars under a different
/// anchor, which Slint resolves from the key.
fn is_waveform(index: usize) -> bool {
    STYLES.get(index).copied() == Some(STYLE_WAVEFORM)
}

/// Both halves: the key the strip mounts on, and the index the pickers bind to.
fn publish_style(global: &Visualizer, index: usize) {
    global.set_style(SharedString::from(STYLES[index]));
    global.set_style_idx(i32::try_from(index).unwrap_or_default());
}

/// The drawing a strip nobody is watching mounts on: every band at rest and the flat figure a
/// decayed trace settles to, with the two flags that say so — both built by the code the live path
/// uses, so neither can drift from what a decay lands on.
///
/// The session-end call is the load-bearing one: the strip's Timer runs on
/// `(playing && window-shown) || !idle`, so a strip remounting over a *paused* player never ticks
/// and whatever the last session left would sit there until playback resumed.
fn publish_resting(global: &Visualizer, model: &VecModel<f32>) {
    rest_bars(model);
    global.set_wave_path(SharedString::from(resting_wave_path().as_str()));
    global.set_idle(RESTING_IDLE);
    global.set_dormant(RESTING_DORMANT);
}

/// Back to the seeded level — the strip floors each band at a visible dot, so rest for the bars is
/// the seed rather than a height.
fn rest_bars(model: &VecModel<f32>) {
    for band in 0..model.row_count() {
        model.set_row_data(band, 0.0);
    }
}

/// The trace's resting figure, through its real writer rather than as a literal so it is exactly
/// what a decay settles to. Two columns is the narrowest input describing a span.
fn resting_wave_path() -> String {
    let mut path = String::new();
    waveform::write_path_commands(&[waveform::Column::default(); 2], &mut path);
    path
}

pub fn install_visualizer(ui: &AppWindow, state: &AppState) {
    // `enabled` only decides whether the strip mounts — the tap stays disarmed until `set-active`
    // says a mounted strip is on screen, so an unreadable file can't leave the two out of step.
    let flags = read_or_default(state, "visualizer").visualizer;
    let selected = style_index(&flags.viz_style);

    // Mutated in place every frame rather than replaced, the strip reading this one.
    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(vec![0.0; NUM_BANDS]));

    // A shadow rather than a read off the global, which would clone a `SharedString` out of a
    // Slint property every tick.
    let style: Rc<Cell<usize>> = Rc::new(Cell::new(selected));

    // Shared between the tick that builds them and the `set-active` that drops them. Both run on
    // the UI thread, and the tick's borrow never outlives one frame.
    let analyzers: Rc<RefCell<Option<Analyzers>>> = Rc::new(RefCell::new(None));

    let viz_global = ui.global::<Visualizer>();
    viz_global.set_enabled(flags.viz_enabled);
    publish_style(&viz_global, selected);
    viz_global.set_bars(ModelRc::from(model.clone()));

    // The bars come up at rest on their own; the trace has nothing to fall back on and its Timer
    // doesn't run until something plays, so a view opened on a fresh app would show an empty strip.
    publish_resting(&viz_global, &model);

    // tick — one frame. Nothing here allocates except the one `SharedString` the waveform path has
    // to be handed to Slint as.
    {
        let viz = state.engine.visualizer();
        let model = model.clone();
        let style = style.clone();
        let analyzers = analyzers.clone();
        let weak = ui.as_weak();

        viz_global.on_tick(move |playing, strip_width| {
            let mut slot = analyzers.borrow_mut();
            // The one construction site, so no mount ordering can leave the tick without buffers
            // however `set-active` and the strip interleave.
            let session = slot.get_or_insert_with(Analyzers::new);

            // Computed rather than returned early on: the decay path below still has to run, so
            // `idle` stays truthful and the Timer can stop.
            let painting = session.frames.painting(pulse::frames());
            let analyzing = playing && painting && tray_bridge::is_window_visible();

            // The steady-state writer of the arm state, so pause, minimise and hide-to-tray all
            // silence the producer as well as the analysis. Arming *discards* the rings, so the
            // drawing ramps back in from silence rather than resuming on a stale shape.
            viz.set_enabled(analyzing);

            // Not `sample_rate()`: the tap sits above the deck's converter, which is where speed
            // is applied. Zero is the "draw nothing new" signal both styles decay on.
            let rate = if analyzing { viz.analysis_rate() } else { 0 };

            let waveform = is_waveform(style.get());
            let idle = if waveform {
                frame::waveform(&viz, &mut session.wave, &mut session.path, rate, strip_width)
            } else {
                frame::bars(&viz, &mut session.spectrum, &model, rate)
            };
            // Settled with nothing arriving to unsettle it: the tick is only still here to watch
            // for frames, which it can do far more slowly.
            let dormant = idle && !painting;

            // The bars ride their own model, written above; only the trace and the two flags come
            // through here. Both drive the strip's Timer so neither can be skipped.
            if waveform || idle != session.was_idle || dormant != session.was_dormant {
                if let Some(ui) = weak.upgrade() {
                    let global = ui.global::<Visualizer>();
                    // Only the mounted style's property — the other one's consumer isn't in the
                    // tree to read it.
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

    // set-active — the mount boundary. Unmounting the strip stops `tick`, so something outside it
    // has to disarm the tap on the way out; on the way in it arms optimistically and the next tick
    // refines it. The session's buffers leave with it, being the feature's resident footprint.
    {
        let viz = state.engine.visualizer();
        let analyzers = analyzers.clone();
        let model = model.clone();
        let weak = ui.as_weak();
        viz_global.on_set_active(move |active| {
            viz.set_enabled(active && tray_bridge::is_window_visible());
            if active {
                // The frame counter isn't free — see `pulse` — so it waits here for a strip that
                // will actually read it.
                if let Some(ui) = weak.upgrade() {
                    pulse::install(&ui);
                }
                return;
            }
            *analyzers.borrow_mut() = None;
            // A strip remounting over a paused player never ticks, so hand back the rest a fresh
            // `Analyzers` shadows — or the next open comes up on the frame this one ended on.
            if let Some(ui) = weak.upgrade() {
                publish_resting(&ui.global::<Visualizer>(), &model);
            }
        });
    }

    // window-hidden — the third writer, and why the tick isn't enough alone. `window-shown` gates
    // the strip's Timer, so a hide landing on an already-settled drawing stops it in the same pass
    // as it drops the gate: `Timer::stop` takes effect at once and the disarming tick never runs.
    {
        let viz = state.engine.visualizer();
        viz_global.on_window_hidden(move || viz.set_enabled(false));
    }

    // set-enabled — persist only. Flipping the setting moves `AppWindow.watched-viz-active`, so
    // arming here too would arm the tap for a view that isn't open. No write-back to `enabled`
    // either: the Settings → Playback toggle two-way binds it, so it has already landed.
    {
        let state = state.clone();
        viz_global.on_set_enabled(move |on| {
            state.persist_blocking("persist viz_enabled", move |s| {
                library::settings::set_visualizer_enabled(s, on)
            });
        });
    }

    // set-style — the chips two-way bind `style-idx`, so that half has landed; the key is what the
    // strip mounts on, and the shadow what the tick reads.
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
