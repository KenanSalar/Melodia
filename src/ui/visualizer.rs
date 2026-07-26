//! Wire the Now-Playing audio visualizer to Rust.
//!
//! Seeds the `Visualizer` global from `settings.json` at startup and owns the
//! four callbacks.
//!
//! `tick` is the render loop. It runs on the UI thread, driven by the strip's
//! own 16 ms `Timer` in `ui/components/now-playing/visualizer-strip.slint`, and
//! is the only consumer of the sample ring. What it does with a snapshot depends
//! on the active style: the bars run snapshot → FFT → bands → model, the
//! waveform runs snapshot → trigger → min/max columns → path string and **no
//! FFT at all**. Either way a 2048-point real FFT is sub-millisecond, so this
//! stays on the UI thread rather than paying for a third thread and a second
//! shared cell.
//!
//! Most of what keeps that cheap composes for free from the mount tree: the
//! strip only mounts while the visualizer is enabled, and the Now-Playing view
//! only mounts while it's open, so a closed view costs nothing at all.
//!
//! # Arming the tap
//!
//! The mount tree can't gate the *producer* — the audio thread would happily
//! keep filling the ring for a view nobody has open, and for a window hidden to
//! tray. So the persisted setting and the tap's arm state are deliberately two
//! different things: `set-enabled` (the Settings → Playback toggle) only
//! persists, and this module is the sole writer of the arm state, from two
//! places on the UI thread:
//!
//! - `set-active` — the mount/unmount boundary, mirrored out of `AppWindow`.
//! - `tick` — the steady state, which folds in pause and window visibility.
//!
//! Between them the tap is armed exactly while something is drawing it. The
//! style has no say in it: every style reads the same ring.

use std::cell::Cell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::library;
use crate::player::spectrum::{FFT_SIZE, NUM_BANDS, SpectrumAnalyzer};
use crate::player::visualizer::RING_CAP;
use crate::player::waveform::{self, MAX_COLUMNS, WaveformAnalyzer};
use crate::services::settings;
use crate::state::AppState;
use crate::ui::tray_bridge;
use crate::{AppWindow, Visualizer};

/// Below this level a drawing is visually at rest. Once everything is under it
/// the strip has finished decaying and its driving Timer can stop.
const IDLE_LEVEL: f32 = 0.001;

const STYLE_BARS: &str = "bars";
const STYLE_WAVEFORM: &str = "waveform";

/// Style keys in picker order.
///
/// Two things mirror this **by position** and neither can be checked by the
/// compiler: the `viz-style-names` `@tr` array in
/// `ui/components/now-playing/flyout-presets.slint` — rendered by both the
/// Settings chips and the Now-Playing view's flyout — and the branch in
/// `visualizer-strip.slint` that mounts on the key. `tests/visualizer_tests.rs`
/// pins both against this array.
const STYLES: [&str; 2] = [STYLE_BARS, STYLE_WAVEFORM];

/// Picker index for a persisted key. An unrecognized key — a hand-edited file,
/// or one written by a newer build — falls back to the default style rather
/// than leaving the strip blank.
fn style_index(key: &str) -> usize {
    STYLES.iter().position(|&style| style == key).unwrap_or(0)
}

/// Whether an index selects the waveform. Out-of-range indices can only come
/// from a picker that has drifted out of step with [`STYLES`], and they answer
/// the same way an unknown key does.
fn is_waveform(index: usize) -> bool {
    STYLES.get(index).copied() == Some(STYLE_WAVEFORM)
}

pub fn install_visualizer(ui: &AppWindow, state: &AppState) {
    // Read the persisted settings. `enabled` only decides whether the strip
    // mounts — the backend tap stays disarmed until `set-active` says a mounted
    // strip is on screen, so an unreadable file can't leave the two out of step.
    let flags = match settings::read_settings(&state.paths) {
        Ok(s) => s.visualizer,
        Err(e) => {
            log::warn!("read settings for visualizer: {e}");
            settings::VisualizerFlags::default()
        }
    };
    let selected = style_index(&flags.viz_style);

    // Backing model for the `bars` `[float]` global property. Kept here (cloned
    // into the tick handler) so every frame mutates the same model the strip
    // reads, rather than replacing the property with a fresh one.
    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(vec![0.0; NUM_BANDS]));

    // The active style, read every tick and written by `set-style`. A shadow
    // rather than a read back off the global: it keeps a string compare out of
    // the 16 ms tick, and both ends live on the UI thread.
    let style: Rc<Cell<usize>> = Rc::new(Cell::new(selected));

    let viz_global = ui.global::<Visualizer>();
    viz_global.set_enabled(flags.viz_enabled);
    viz_global.set_style(SharedString::from(STYLES[selected]));
    viz_global.set_style_idx(i32::try_from(selected).unwrap_or_default());
    viz_global.set_bars(ModelRc::from(model.clone()));
    viz_global.set_idle(true);

    // The bars come up at rest for free: their model is seeded with a level per
    // band and each one floors at a dot. The trace has no model to fall back on,
    // and the Timer that would write its path doesn't run until something plays
    // — nothing is playing and `idle` is true — so a Now-Playing view opened on
    // a freshly started app would show an empty strip. Seed the resting figure
    // through the real writer, so it is exactly what a decayed trace settles to.
    {
        let mut resting = String::new();
        waveform::write_path_commands(&[waveform::Column::default(); 2], &mut resting);
        viz_global.set_wave_path(SharedString::from(resting.as_str()));
    }

    // tick — one frame of analysis. Slint callbacks are `FnMut`, so both
    // analyzers (the FFT plan and its buffers, the Hann table, the bin→band map,
    // the smoothing state, the sample window and the trace) are simply owned by
    // the closure and reused across ticks; nothing here allocates except the one
    // `SharedString` the waveform path has to be handed to Slint as.
    {
        let viz = state.rodio.visualizer();
        let model = model.clone();
        let style = style.clone();
        let weak = ui.as_weak();
        let mut spectrum = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
        // A window wider than the ring would only be padded with silence, so the
        // ring's capacity is the analyzer's too.
        let mut wave = WaveformAnalyzer::new(RING_CAP, MAX_COLUMNS);
        // Sized once for the widest trace — two vertices a column, ~17 bytes a
        // vertex — so the per-frame rebuild only ever writes into capacity it
        // already has.
        let mut path = String::with_capacity(MAX_COLUMNS * 2 * 20);

        viz_global.on_tick(move |playing, strip_width| {
            // A pause leaves the last window of audio sitting in the ring, so
            // re-analysing it would freeze the drawing on a stale shape instead
            // of letting it fall. A hidden window has nothing to draw for at
            // all — the strip's Timer fires off the event loop, not the render
            // loop, and the loop stays alive through a close-to-tray hide.
            // Computed rather than returned early on: the decay path below still
            // has to run, so `idle` stays truthful and the Timer can still stop.
            let analyzing = playing && tray_bridge::is_window_visible();

            // The steady-state writer of the tap's arm state, so pause, minimise
            // and hide-to-tray all silence the producer as well as the analysis.
            // Re-arming can miss up to one 16 ms window of audio; `snapshot`
            // pads the front with silence, so the first frame back is a touch
            // low rather than wrong.
            viz.set_enabled(analyzing);

            let waveform = is_waveform(style.get());
            let idle = if waveform {
                // The trace's span is a fixed number of milliseconds, so how
                // many samples that is depends on the rate — and on the speed,
                // which `analysis_rate` folds in, keeping the span 40 ms of what
                // you *hear* rather than of the file.
                let rate = if analyzing { viz.analysis_rate() } else { 0 };
                // Nothing has played yet, so there is no rate to size a window
                // against; `analyze` decays the last trace instead.
                let live = rate > 0;
                if live {
                    // Straight into the analyzer's own window — no intermediate
                    // buffer, no per-tick copy.
                    viz.snapshot(wave.window_mut(rate));
                }
                let columns =
                    wave.analyze(live, rate, waveform::columns_for_width(strip_width));
                waveform::write_path_commands(columns, &mut path);
                columns
                    .iter()
                    .all(|c| c.max.abs() < IDLE_LEVEL && c.min.abs() < IDLE_LEVEL)
            } else {
                let sample_rate = if analyzing {
                    // Straight into the FFT's own input buffer.
                    viz.snapshot(spectrum.window_mut());
                    // Not `sample_rate()`: the tap sits under rodio's speed
                    // stage, so the analysis rate has to fold the speed back in.
                    viz.analysis_rate()
                } else {
                    0
                };
                let levels = spectrum.analyze(sample_rate);
                let idle = levels.iter().all(|&level| level < IDLE_LEVEL);
                for (band, &level) in levels.iter().enumerate() {
                    model.set_row_data(band, level);
                }
                idle
            };

            if let Some(ui) = weak.upgrade() {
                let global = ui.global::<Visualizer>();
                // Only the mounted style's property is written — the other one's
                // consumer isn't in the tree to read it.
                if waveform {
                    global.set_wave_path(SharedString::from(path.as_str()));
                }
                global.set_idle(idle);
            }
        });
    }

    // set-active — the mount/unmount boundary. Closing Now Playing (or turning
    // the setting off with it open) unmounts the strip, which stops `tick`, so
    // something outside the strip has to disarm the tap on the way out. On the
    // way in it arms optimistically and the next tick refines it.
    {
        let viz = state.rodio.visualizer();
        viz_global.on_set_active(move |active| {
            viz.set_enabled(active && tray_bridge::is_window_visible());
        });
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
            // A negative or out-of-range index can only come from a picker that
            // has drifted out of step with `STYLES`; it falls back to the
            // default style, the same way an unknown key does.
            let picked = usize::try_from(index)
                .ok()
                .filter(|&i| i < STYLES.len())
                .unwrap_or_default();
            style.set(picked);
            if let Some(ui) = weak.upgrade() {
                let global = ui.global::<Visualizer>();
                global.set_style(SharedString::from(STYLES[picked]));
                global.set_style_idx(i32::try_from(picked).unwrap_or_default());
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
