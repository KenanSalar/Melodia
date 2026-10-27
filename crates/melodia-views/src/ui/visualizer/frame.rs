//! One drawn frame per style, off the strip's Timer; the sample ring's only consumers. A rate of
//! `0`, for a pause, a minimised window or a track not yet started, skips the snapshot and the
//! transform and only decays. Each answers whether its drawing settled, which stops the Timer.

use melodia_playback::player::playback::spectrum::SpectrumAnalyzer;
use melodia_playback::player::playback::visualizer::VisualizerShared;
use melodia_playback::player::playback::waveform::{self, WaveformAnalyzer};

/// Below this level a drawing is visually at rest, so the driving Timer can stop.
const IDLE_LEVEL: f32 = 0.001;

/// One bars (or mirrored) frame: snapshot → two FFTs → bands. The figure is drawn where the strip
/// asks for it, against the geometry it has then (`bars-figure`).
pub(super) fn bars(viz: &VisualizerShared, analyzer: &mut SpectrumAnalyzer, rate: u32) -> bool {
    if rate > 0 {
        // Straight into the transforms' own input buffers. They overlap, the long one
        // simply reaching further back, so one read of the ring fills both.
        analyzer.fill_windows(|window| viz.snapshot(window));
    }

    analyzer.analyze(rate).iter().all(|&level| level < IDLE_LEVEL)
}

/// One waveform frame: snapshot → trigger → min/max columns → path string.
pub(super) fn waveform(
    viz: &VisualizerShared,
    analyzer: &mut WaveformAnalyzer,
    path: &mut String,
    rate: u32,
    strip_width: f32,
) -> bool {
    // The trace's span is a fixed number of milliseconds, so the window can only be sized
    // once the rate is known; without one, `analyze` decays the last trace instead.
    let live = rate > 0;
    if live {
        viz.snapshot(analyzer.window_mut(rate));
    }

    let columns = analyzer.trace(live, rate, waveform::columns_for_width(strip_width), path);
    columns.iter().all(|c| c.max.abs() < IDLE_LEVEL && c.min.abs() < IDLE_LEVEL)
}
