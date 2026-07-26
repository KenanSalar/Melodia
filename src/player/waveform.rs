//! Oscilloscope trace for the audio visualizer.
//!
//! The waveform style draws the signal itself rather than its spectrum, so it
//! skips [`spectrum`] entirely — no window, no FFT, no banding.
//!
//! # A column is a range, not a point
//!
//! The strip is a few hundred pixels wide and the span it shows is a few
//! thousand samples, so several samples always share a column. Picking one of
//! them — the loudest, the nearest, the mean — throws away the others, and every
//! way of picking is wrong in its own way: the loudest alternates between
//! opposite extremes and scribbles, the nearest aliases, the mean acts as a
//! lowpass and flattens anything bright.
//!
//! So a column carries the **whole range** its samples covered: a
//! [`Column`] of `min` and `max`, and the strip is drawn as the area between the
//! two edges. Nothing is skipped, so nothing can alias, and the shape reads as
//! the music rather than as noise. `DeaDBeeF`'s scope
//! (`ddb_scope_point_t { ymin, ymax }`) and every audio editor's waveform view
//! resolve the same problem the same way; `foobar2000`'s oscilloscope arrives
//! somewhere similar by brute force, drawing every single sample and letting the
//! overdraw fill the band in.
//!
//! The pipeline, once per drawn frame:
//!
//! 1. [`find_trigger`] over the leading part of the snapshot, leaving a full
//!    span of samples behind it to draw,
//! 2. [`min_max_columns`] reduces that span to one range per drawn column,
//! 3. [`write_path_commands`] closes the two edges into the single filled figure
//!    the Slint `Path` renders.
//!
//! Like [`spectrum`], everything here is a free function over slices;
//! [`WaveformAnalyzer`] exists only to hold the two buffers that must not be
//! reallocated every frame.
//!
//! [`spectrum`]: super::spectrum

use std::fmt::Write as _;

/// Milliseconds of audio drawn across the strip.
///
/// Time, not samples: a fixed sample count would show 23 ms of a 44.1 kHz file
/// and 10 ms of a 96 kHz one, so the same music would draw differently depending
/// on how it was mastered. `DeaDBeeF` defaults to 50 ms and `foobar2000` to 100;
/// this sits between them, low enough that a bass note still reads as a wave
/// rather than a blur at the strip's height.
pub const WAVE_SPAN_MS: u32 = 40;

/// Extra milliseconds snapshotted *ahead* of the drawn span, giving
/// [`find_trigger`] somewhere to look. Also the trace's worst-case latency.
pub const TRIGGER_SLACK_MS: u32 = 20;

/// Logical pixels per drawn column.
///
/// `DeaDBeeF` uses one column per pixel. One per two is visually
/// indistinguishable under the envelope's own 1.25 px stroke and halves
/// everything downstream — the path string, its re-parse, and the tessellation
/// of the filled figure — which matters here in a way it doesn't for a scope
/// drawing straight to a canvas.
const LOGICAL_PX_PER_COLUMN: f32 = 2.0;

/// Column bounds. The floor keeps a sub-100 px strip from degenerating into a
/// handful of very wide columns; the ceiling bounds the per-frame path string,
/// and is what the analyzer's buffer is sized to.
const MIN_COLUMNS: usize = 64;
pub const MAX_COLUMNS: usize = 512;

/// How far below zero the signal must dip before a crossing back up counts.
/// Without it the trigger latches onto noise around the axis and the trace
/// jitters exactly as if it had none.
const TRIGGER_HYSTERESIS: f32 = 0.02;

/// Fraction of its height the trace keeps per frame once the audio stops.
/// Matched to the spectrum's decay so both styles settle at the same rate.
const DECAY: f32 = 0.8;

/// Half-thickness, in viewbox units, that a drawn column never falls below.
///
/// A column whose two edges land on each other contributes no area, so a wholly
/// silent trace closes a **zero-area polygon** and hands the renderer an outline
/// lying exactly on top of itself — geometry nothing owes you anything sensible
/// for, and in practice what drew the resting line as dashes. The floor keeps
/// the figure a real shape at every amplitude.
///
/// Its size is chosen against the stroke rather than for its own sake. The
/// viewbox is two units tall across the strip's fixed 56 px, so this is half a
/// pixel either side of the centre — close enough that the two edges' 1.25 px
/// strokes still overlap and a resting trace reads as one line rather than as a
/// pair of rails, far enough that they are nowhere near coincident.
const MIN_HALF_THICKNESS: f32 = 0.018;

#[allow(
    clippy::cast_precision_loss,
    reason = "column indices are counts in the low hundreds, which convert to f32 exactly"
)]
fn index_to_f32(i: usize) -> f32 {
    i as f32
}

/// Samples spanning `ms` at `sample_rate`. Saturates rather than wrapping; the
/// caller clamps to its buffer anyway.
fn samples_for_ms(sample_rate: u32, ms: u32) -> usize {
    usize::try_from(u64::from(sample_rate) * u64::from(ms) / 1000).unwrap_or(usize::MAX)
}

/// The range of sample values one drawn column covers.
///
/// `min <= max` always holds for a column produced by [`min_max_columns`], which
/// is what keeps the drawn figure from folding over itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Column {
    pub min: f32,
    pub max: f32,
}

/// Drawn columns for a strip `width` logical pixels wide.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped into MIN_COLUMNS..=MAX_COLUMNS before the cast, so the value is small and positive"
)]
pub fn columns_for_width(width: f32) -> usize {
    if !width.is_finite() || width <= 0.0 {
        return MIN_COLUMNS;
    }
    (width / LOGICAL_PX_PER_COLUMN).clamp(index_to_f32(MIN_COLUMNS), index_to_f32(MAX_COLUMNS))
        as usize
}

/// Index of the most recent rising zero crossing within `samples[..search_len]`,
/// or `0` if there is none.
///
/// "Most recent" rather than "first" keeps the drawn span as close to live as
/// the slack allows; because every candidate is a crossing of the *same*
/// polarity, which one is picked doesn't change the shape that gets drawn, only
/// its latency. (`foobar2000` takes the first instead, and confirms with a third
/// sample where this uses a hysteresis threshold.)
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

/// Reduce `src` to one [`Column`] per slot of `out`, oldest first.
///
/// Each column spans the samples that fall in it and reports their full range,
/// so no sample is skipped and no column can misrepresent what it covered. More
/// slots than samples is handled by holding the nearest sample, giving a column
/// of zero height rather than a gap.
pub fn min_max_columns(src: &[f32], out: &mut [Column]) {
    if src.is_empty() {
        out.fill(Column::default());
        return;
    }
    let buckets = out.len();
    for (i, column) in out.iter_mut().enumerate() {
        let lo = i * src.len() / buckets;
        // At least one sample per bucket, so upsampling holds rather than
        // reading an empty range — which is also what makes the fold below
        // guaranteed to see a value.
        let hi = ((i + 1) * src.len() / buckets).max(lo + 1).min(src.len());
        let mut range = Column {
            min: f32::MAX,
            max: f32::MIN,
        };
        for &sample in &src[lo..hi] {
            if sample < range.min {
                range.min = sample;
            }
            if sample > range.max {
                range.max = sample;
            }
        }
        *column = range;
    }
}

/// Write `columns` into `out` as SVG path commands: the upper edge left to
/// right, the lower edge back, closed into one filled figure.
///
/// Coordinates are normalized — x across `0..1`, y over `-1..1` — so the
/// `Path`'s viewbox is a constant and no column count has to be kept in step
/// across the language boundary. `out` is reused between frames; this is the one
/// place in the visualizer that touches a heap buffer per frame, and clearing
/// rather than reallocating is what keeps that to the `SharedString` the caller
/// hands to Slint.
///
/// A silent column is drawn [`MIN_HALF_THICKNESS`] tall either side of the axis
/// rather than flat, so a resting trace is a visible rule — the trace's
/// equivalent of the bars resting as dots — and, more importantly, so the figure
/// always encloses real area.
pub fn write_path_commands(columns: &[Column], out: &mut String) {
    out.clear();
    if columns.is_empty() {
        return;
    }
    // A lone column has no span to normalize against; it lands at x = 0.
    let span = index_to_f32(columns.len() - 1).max(1.0);

    // Lower edge left to right, then upper edge back. That order rather than the
    // reverse is what gives the closed figure a *positive* signed area, which is
    // what Slint's femtovg renderer reads to decide whether a subpath is solid
    // or a hole — and a lone subpath handed over as a hole is not a thing to
    // rely on the renderer being sensible about.
    for (i, column) in columns.iter().enumerate() {
        let x = index_to_f32(i) / span;
        let y = edges(*column).1;
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
    for (i, column) in columns.iter().enumerate().rev() {
        let x = index_to_f32(i) / span;
        let y = edges(*column).0;
        let _ = write!(out, " L{x:.4} {y:.3}");
    }
    out.push('Z');
}

/// A column's two screen-space edges, `(upper, lower)`.
///
/// Screen coordinates grow downward and amplitude grows upward, so the column is
/// flipped to put positive peaks at the top of the strip where a scope draws
/// them. The floor is applied about the column's own midpoint, so a loud
/// asymmetric column keeps its centre and only a near-silent one is opened up.
fn edges(column: Column) -> (f32, f32) {
    let mid = 0.5 * (column.min + column.max);
    let half = (0.5 * (column.max - column.min)).max(MIN_HALF_THICKNESS);
    let flipped = 0.0 - mid;
    (flipped - half, flipped + half)
}

/// Holds the sample window and the drawn columns, both allocated once at their
/// widest and used a prefix at a time.
///
/// Usage mirrors [`SpectrumAnalyzer`]: fill [`window_mut`](Self::window_mut)
/// from the ring, then call [`analyze`](Self::analyze). Both take the sample
/// rate and derive the same window length from it, so there is no order-dependent
/// state between them.
///
/// [`SpectrumAnalyzer`]: super::spectrum::SpectrumAnalyzer
pub struct WaveformAnalyzer {
    window: Vec<f32>,
    columns: Vec<Column>,
}

impl WaveformAnalyzer {
    /// Build an analyzer that can snapshot up to `window_cap` samples and draw
    /// up to `column_cap` columns. In production `window_cap` is
    /// [`RING_CAP`](super::visualizer::RING_CAP) — a window wider than the ring
    /// would only be padded with silence.
    #[must_use]
    pub fn new(window_cap: usize, column_cap: usize) -> Self {
        Self {
            window: vec![0.0; window_cap],
            columns: vec![Column::default(); column_cap],
        }
    }

    /// Samples to snapshot at `sample_rate`: the drawn span plus the trigger's
    /// slack, capped at what the buffer (and so the ring) can hold. Above about
    /// 192 kHz the cap bites and the trace narrows in time; nothing breaks, it
    /// just shows less.
    fn window_len(&self, sample_rate: u32) -> usize {
        samples_for_ms(sample_rate, WAVE_SPAN_MS + TRIGGER_SLACK_MS).clamp(2, self.window.len())
    }

    /// The buffer the next [`analyze`](Self::analyze) call reads. Fill it with
    /// the most recent samples, oldest first.
    pub fn window_mut(&mut self, sample_rate: u32) -> &mut [f32] {
        let len = self.window_len(sample_rate);
        &mut self.window[..len]
    }

    /// Produce this frame's columns, at most `columns` of them.
    ///
    /// `active` is false when there is nothing to draw from — a paused player, a
    /// hidden window, a track that hasn't started. The ring still holds the last
    /// window of audio then, so re-reading it would freeze the trace mid-shape;
    /// instead the previous one collapses toward the centre line, which is both
    /// the better look and what lets the caller's idle check eventually stop the
    /// driving timer.
    pub fn analyze(&mut self, active: bool, sample_rate: u32, columns: usize) -> &[Column] {
        let width = columns.clamp(1, self.columns.len());
        if active {
            let window = self.window_len(sample_rate);
            let span = samples_for_ms(sample_rate, WAVE_SPAN_MS).clamp(1, window);
            let trigger = find_trigger(&self.window[..window], window - span);
            let drawn = &self.window[trigger..trigger + span];
            min_max_columns(drawn, &mut self.columns[..width]);
        } else {
            // The whole buffer, not just the drawn prefix: the strip can be
            // resized while paused, and a column that widened back into view
            // undecayed would pop.
            for column in &mut self.columns {
                column.min *= DECAY;
                column.max *= DECAY;
            }
        }
        &self.columns[..width]
    }
}

#[cfg(test)]
#[path = "tests/waveform_tests.rs"]
mod tests;
