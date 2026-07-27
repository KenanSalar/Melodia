//! One drawn frame of the visualizer, per style.
//!
//! Both functions run on the UI thread off the strip's own `Timer` and are the
//! only consumers of the sample ring. Each takes the analysis rate its caller
//! already resolved — `0` when nothing should be analysed, which is how a pause,
//! a minimised window and a track that hasn't started yet all reach the same
//! path: no snapshot, no transform, just the decay. Each returns whether its
//! drawing has settled, which is what stops the Timer.
//!
//! Neither allocates. The analyzers own every buffer they need, and the path
//! string is reused across frames.

use slint::{Model, VecModel};

use crate::player::spectrum::SpectrumAnalyzer;
use crate::player::visualizer::VisualizerShared;
use crate::player::waveform::{self, WaveformAnalyzer};

/// Below this level a drawing is visually at rest. Once everything is under it
/// the strip has finished decaying and its driving Timer can stop.
const IDLE_LEVEL: f32 = 0.001;

/// One bars (or mirrored) frame: snapshot → two FFTs → bands → model.
///
/// Every band is written every tick, changed or not, and that is load-bearing
/// beyond the drawing: the repaint it asks for is what [`super::pulse`] counts to
/// tell a window being drawn from one that isn't. Skipping unchanged levels would
/// weaken that signal, not just save a few model writes.
pub(super) fn bars(
    viz: &VisualizerShared,
    analyzer: &mut SpectrumAnalyzer,
    model: &VecModel<f32>,
    rate: u32,
) -> bool {
    if rate > 0 {
        // Straight into the transforms' own input buffers — no intermediate
        // copy. They overlap, the long one simply reaching further back, so one
        // read of the ring fills both.
        analyzer.fill_windows(|window| viz.snapshot(window));
    }

    let mut idle = true;
    for (band, &level) in analyzer.analyze(rate).iter().enumerate() {
        idle &= level < IDLE_LEVEL;
        model.set_row_data(band, level);
    }
    idle
}

/// One waveform frame: snapshot → trigger → min/max columns → path string.
pub(super) fn waveform(
    viz: &VisualizerShared,
    analyzer: &mut WaveformAnalyzer,
    path: &mut String,
    rate: u32,
    strip_width: f32,
) -> bool {
    // The trace's span is a fixed number of milliseconds, so how many samples
    // that is depends on the rate — which is why the window can only be sized
    // once the rate is known. Without one there is nothing to size against, and
    // `analyze` decays the last trace instead.
    let live = rate > 0;
    if live {
        // Straight into the analyzer's own window — no intermediate buffer.
        viz.snapshot(analyzer.window_mut(rate));
    }

    let columns = analyzer.trace(live, rate, waveform::columns_for_width(strip_width), path);
    columns
        .iter()
        .all(|c| c.max.abs() < IDLE_LEVEL && c.min.abs() < IDLE_LEVEL)
}
