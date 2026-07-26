//! Oscilloscope trace for the audio visualizer.
//!
//! The waveform style draws the signal itself rather than its spectrum, so it
//! skips [`spectrum`] entirely — no window, no FFT, no banding. What it needs
//! instead is a *stable* view: consecutive snapshots of the ring start at an
//! arbitrary phase, so a trace drawn straight off them slides sideways every
//! frame and reads as broken. The fix is the same one every oscilloscope uses —
//! trigger on a rising zero crossing and draw from there.
//!
//! The pipeline, once per drawn frame:
//!
//! 1. [`find_trigger`] over the leading part of the snapshot, leaving a full
//!    span of samples behind it to draw,
//! 2. [`downsample`] that span to one point per drawn vertex,
//! 3. [`write_path_commands`] turns the points into the SVG string the Slint
//!    `Path` renders.
//!
//! Like [`spectrum`], everything here is a free function over slices;
//! [`WaveformAnalyzer`] exists only to hold the two buffers that must not be
//! reallocated every frame.
//!
//! Playback speed needs no handling, unlike the spectrum's band edges: the tap
//! sits under rodio's speed stage either way, but for a trace that only changes
//! how much wall-clock time the drawn span covers, which is not wrong.
//!
//! [`spectrum`]: super::spectrum

use std::fmt::Write as _;

/// Samples copied out of the ring per frame. The excess over [`WAVE_SPAN`] is
/// the slack [`find_trigger`] searches.
pub const WAVE_WINDOW: usize = 2048;

/// Samples actually drawn — ~23 ms at 44.1 kHz, a couple of periods of a bass
/// note. Short enough that the shape reads, long enough that it doesn't twitch.
pub const WAVE_SPAN: usize = 1024;

/// Vertices in the drawn polyline. Well above the pixel budget of the strip at
/// any window size, and short enough that the per-frame path string stays small.
pub const WAVE_POINTS: usize = 192;

/// How far below zero the signal must dip before a crossing back up counts.
/// Without it the trigger latches onto noise around the axis and the trace
/// jitters exactly as if it had none.
const TRIGGER_HYSTERESIS: f32 = 0.02;

/// Fraction of its height the trace keeps per frame once the audio stops.
/// Matched to the spectrum's decay so both styles settle at the same rate.
const DECAY: f32 = 0.8;

#[allow(
    clippy::cast_precision_loss,
    reason = "vertex indices are counts in the low hundreds, which convert to f32 exactly"
)]
fn index_to_f32(i: usize) -> f32 {
    i as f32
}

/// Index of the most recent rising zero crossing within `samples[..search_len]`,
/// or `0` if there is none.
///
/// "Most recent" rather than "first" keeps the drawn span as close to live as
/// the slack allows; because every candidate is a crossing of the *same*
/// polarity, which one is picked doesn't change the shape that gets drawn, only
/// its latency.
///
/// The `0` fallback is an untriggered trace, which is the right answer for the
/// two signals that have no crossing to find: silence, and a window sitting
/// entirely on one side of the axis.
#[must_use]
pub fn find_trigger(samples: &[f32], search_len: usize) -> usize {
    let mut armed = false;
    let mut trigger = 0;
    for (i, &sample) in samples.iter().take(search_len).enumerate() {
        if sample < -TRIGGER_HYSTERESIS {
            armed = true;
        } else if armed && sample >= 0.0 {
            trigger = i;
            armed = false;
        }
    }
    trigger
}

/// Reduce `src` to one value per slot of `out`, oldest first.
///
/// Each slot takes the sample of largest magnitude in its bucket, **sign
/// intact**. Averaging would be the obvious alternative and is wrong here: at a
/// few samples per bucket it acts as a lowpass, so a bright mix draws as a
/// nearly flat line. Taking the peak keeps transients, which is what makes the
/// trace look like the music.
///
/// More slots than samples is handled by holding the nearest sample, so `out` is
/// always filled.
pub fn downsample(src: &[f32], out: &mut [f32]) {
    if src.is_empty() {
        out.fill(0.0);
        return;
    }
    let buckets = out.len();
    for (i, slot) in out.iter_mut().enumerate() {
        let lo = i * src.len() / buckets;
        // At least one sample per bucket, so upsampling holds rather than
        // reading an empty range.
        let hi = ((i + 1) * src.len() / buckets).max(lo + 1).min(src.len());
        *slot = src[lo..hi]
            .iter()
            .fold(0.0, |peak: f32, &s| if s.abs() > peak.abs() { s } else { peak });
    }
}

/// Write `points` into `out` as SVG path commands: one `M`, then an `L` per
/// remaining vertex.
///
/// Coordinates are normalized — x across `0..1`, y over `-1..1` — so the
/// `Path`'s viewbox is a constant and no vertex count has to be kept in step
/// across the language boundary. `out` is reused between frames; this is the one
/// place in the visualizer that touches a heap buffer per frame, and clearing
/// rather than reallocating is what keeps that to the `SharedString` the caller
/// hands to Slint.
pub fn write_path_commands(points: &[f32], out: &mut String) {
    out.clear();
    // A lone point has no span to normalize against. It lands at x = 0 and
    // draws nothing, which is the honest answer for a one-vertex trace.
    let span = index_to_f32(points.len().saturating_sub(1)).max(1.0);
    for (i, &sample) in points.iter().enumerate() {
        let x = index_to_f32(i) / span;
        // Screen coordinates grow downward, amplitude grows upward, so the
        // sample is flipped to put positive peaks at the top of the strip where
        // a scope draws them. `0.0 - sample` rather than `-sample` because the
        // latter turns a silent sample into `-0.000`, and a resting trace is a
        // whole line of them.
        let y = 0.0 - sample;
        // Writing into a String cannot fail. The space before each `L` is not
        // required by the SVG grammar — a command letter terminates the number
        // before it — but it costs a byte a vertex and leaves nothing to the
        // parser's discretion.
        let _ = if i == 0 {
            write!(out, "M{x:.4} {y:.3}")
        } else {
            write!(out, " L{x:.4} {y:.3}")
        };
    }
}

/// Holds the sample window and the drawn points, both allocated once.
///
/// Usage mirrors [`SpectrumAnalyzer`]: fill [`window_mut`](Self::window_mut)
/// from the ring, then call [`analyze`](Self::analyze).
///
/// [`SpectrumAnalyzer`]: super::spectrum::SpectrumAnalyzer
pub struct WaveformAnalyzer {
    window: Vec<f32>,
    points: Box<[f32]>,
}

impl WaveformAnalyzer {
    /// Build an analyzer over a `window`-sample snapshot drawn as `points`
    /// vertices — [`WAVE_WINDOW`] and [`WAVE_POINTS`] in production.
    #[must_use]
    pub fn new(window: usize, points: usize) -> Self {
        Self {
            window: vec![0.0; window],
            points: vec![0.0; points].into_boxed_slice(),
        }
    }

    /// The buffer the next [`analyze`](Self::analyze) call reads. Fill it with
    /// the most recent samples, oldest first.
    pub fn window_mut(&mut self) -> &mut [f32] {
        &mut self.window
    }

    /// Produce this frame's trace.
    ///
    /// `active` is false when there is nothing to draw from — a paused player, a
    /// hidden window. The ring still holds the last window of audio then, so
    /// re-reading it would freeze the trace mid-shape; instead the previous one
    /// collapses toward the centre line, which is both the better look and what
    /// lets the caller's idle check eventually stop the driving timer.
    pub fn analyze(&mut self, active: bool) -> &[f32] {
        if active {
            let slack = self.window.len().saturating_sub(WAVE_SPAN);
            let start = find_trigger(&self.window, slack);
            let end = (start + WAVE_SPAN).min(self.window.len());
            downsample(&self.window[start..end], &mut self.points);
        } else {
            for point in &mut self.points {
                *point *= DECAY;
            }
        }
        &self.points
    }
}

#[cfg(test)]
#[path = "tests/waveform_tests.rs"]
mod tests;
