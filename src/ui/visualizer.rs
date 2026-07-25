//! Wire the Now-Playing spectrum visualizer to Rust.
//!
//! Seeds the `Visualizer` global from `settings.json` at startup and owns the
//! two callbacks. `set-enabled` — fired by the Settings → Playback toggle —
//! follows the two-phase shape of the sibling audio toggles (mirroring
//! [`crate::ui::equalizer`]): apply to the live Rodio tap synchronously, then
//! persist on the blocking pool.
//!
//! `tick` is the render loop. It runs on the UI thread, driven by the strip's
//! own 16 ms `Timer` in `ui/components/now-playing/spectrum-bars.slint`, and is
//! the only consumer of the sample ring: snapshot → FFT → bands → model. A
//! 2048-point real FFT is sub-millisecond, so it stays on the UI thread rather
//! than paying for a third thread and a second shared cell.
//!
//! Most of what keeps this cheap lives elsewhere and composes for free: the
//! strip only mounts while the visualizer is enabled, the Now-Playing view only
//! mounts while it's open, and the Timer stops once a paused player's bars have
//! decayed (see `idle` below). The one gate the mount tree can't provide is
//! window visibility — a hidden window's Timers keep firing — so `tick` folds
//! that into the same check that skips the transform on a paused player.

use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::library;
use crate::player::spectrum::{FFT_SIZE, NUM_BANDS, SpectrumAnalyzer};
use crate::services::settings;
use crate::state::AppState;
use crate::ui::settings_bind::toggle_binding;
use crate::ui::tray_bridge;
use crate::{AppWindow, Visualizer};

/// Below this level a bar is visually at rest. Once every band is under it the
/// strip has finished decaying and its driving Timer can stop.
const IDLE_LEVEL: f32 = 0.001;

pub fn install_visualizer(ui: &AppWindow, state: &AppState) {
    // Read the persisted toggle. The *backend* tap is armed separately by
    // `hydrate_audio_dsp` at `AppState::init` — this read only seeds the Slint
    // side. An unreadable file must fall back to the same value `init` used
    // (it defaults the whole `SettingsData`), or the tap would run armed with
    // the bars hidden, or vice versa.
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
            // A pause stops the tap but leaves the last window of audio sitting
            // in the ring, so re-analysing it would freeze the bars on a stale
            // spectrum. A hidden window has nothing to draw for at all — the
            // strip's Timer fires off the event loop, not the render loop, and
            // the loop stays alive through a close-to-tray hide. Either way the
            // saving is the snapshot and the FFT; the decay path below still
            // runs, so `idle` stays truthful and the Timer can still stop.
            let analyzing = playing && tray_bridge::is_window_visible();
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

    // set-enabled — arm/disarm the live tap, then persist. No write-back to
    // `enabled`: the Settings → Playback toggle two-way binds it, so it has
    // already landed by the time this fires (the crossfade / gapless shape).
    viz_global.on_set_enabled(toggle_binding(
        state,
        "persist viz_enabled",
        library::playback::player_set_visualizer_enabled,
        library::settings::set_visualizer_enabled,
    ));
}
