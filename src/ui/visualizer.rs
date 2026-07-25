//! Wire the Now-Playing spectrum visualizer to Rust.
//!
//! Seeds the `Visualizer` global from `settings.json` at startup and owns the
//! three callbacks.
//!
//! `tick` is the render loop. It runs on the UI thread, driven by the strip's
//! own 16 ms `Timer` in `ui/components/now-playing/spectrum-bars.slint`, and is
//! the only consumer of the sample ring: snapshot → FFT → bands → model. A
//! 2048-point real FFT is sub-millisecond, so it stays on the UI thread rather
//! than paying for a third thread and a second shared cell.
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
//! Between them the tap is armed exactly while something is drawing it.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::library;
use crate::player::spectrum::{FFT_SIZE, NUM_BANDS, SpectrumAnalyzer};
use crate::services::settings;
use crate::state::AppState;
use crate::ui::tray_bridge;
use crate::{AppWindow, Visualizer};

/// Below this level a bar is visually at rest. Once every band is under it the
/// strip has finished decaying and its driving Timer can stop.
const IDLE_LEVEL: f32 = 0.001;

pub fn install_visualizer(ui: &AppWindow, state: &AppState) {
    // Read the persisted toggle. This only decides whether the strip mounts —
    // the backend tap stays disarmed until `set-active` says a mounted strip is
    // on screen, so an unreadable file can't leave the two out of step.
    let enabled = match settings::read_settings(&state.paths) {
        Ok(s) => s.visualizer.viz_enabled,
        Err(e) => {
            log::warn!("read settings for visualizer: {e}");
            settings::VisualizerFlags::default().viz_enabled
        }
    };

    // Backing model for the `bars` `[float]` global property. Kept here (cloned
    // into the tick handler) so every frame mutates the same model the strip
    // reads, rather than replacing the property with a fresh one.
    let model: Rc<VecModel<f32>> = Rc::new(VecModel::from(vec![0.0; NUM_BANDS]));

    let viz_global = ui.global::<Visualizer>();
    viz_global.set_enabled(enabled);
    viz_global.set_bars(ModelRc::from(model.clone()));
    viz_global.set_idle(true);

    // tick — one frame of analysis. Slint callbacks are `FnMut`, so the
    // analyzer (FFT plan, its three buffers, the Hann table, the bin→band map
    // and the smoothing state) is simply owned by the closure and reused across
    // ticks; nothing here allocates.
    {
        let viz = state.rodio.visualizer();
        let model = model.clone();
        let weak = ui.as_weak();
        let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);

        viz_global.on_tick(move |playing| {
            // A pause leaves the last window of audio sitting in the ring, so
            // re-analysing it would freeze the bars on a stale spectrum instead
            // of letting them fall. A hidden window has nothing to draw for at
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

            let sample_rate = if analyzing {
                // Straight into the FFT's own input buffer — no intermediate
                // window, no per-tick copy.
                viz.snapshot(analyzer.window_mut());
                // Not `sample_rate()`: the tap sits under rodio's speed stage,
                // so the analysis rate has to fold the speed back in.
                viz.analysis_rate()
            } else {
                0
            };

            let levels = analyzer.analyze(sample_rate);
            let idle = levels.iter().all(|&level| level < IDLE_LEVEL);
            for (band, &level) in levels.iter().enumerate() {
                model.set_row_data(band, level);
            }

            if let Some(ui) = weak.upgrade() {
                ui.global::<Visualizer>().set_idle(idle);
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
}
