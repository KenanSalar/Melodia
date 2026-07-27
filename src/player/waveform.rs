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
//! 3. the two edges are closed into the single filled figure the Slint `Path`
//!    renders — [`WaveformAnalyzer::trace`] on the per-frame path,
//!    [`write_path_commands`] for a figure drawn once.
//!
//! Like [`spectrum`], everything here is a free function over slices;
//! [`WaveformAnalyzer`] exists only to hold the buffers that must not be
//! reallocated every frame. [`WaveformAnalyzer::trace`] runs all three steps and
//! is what production calls; the pieces stay public because the tests drive them
//! individually and the resting figure is written through
//! [`write_path_commands`] directly.
//!
//! [`spectrum`]: super::spectrum

use std::fmt::Write as _;

use super::dsp::{VISUALIZER_DECAY, index_to_f32};

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
#[expect(
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

/// Write `columns` into `out` as SVG path commands: the lower edge left to
/// right, the upper edge back, closed into one filled figure.
///
/// That order is not a detail — it is what gives the figure a positive signed
/// area, which is what femtovg reads to decide solid from hole. The writer says
/// so where it does it, and `the_closed_figure_winds_positively` pins it.
///
/// Coordinates are normalized — x across `0..1`, y over `-1..1` — so the
/// `Path`'s viewbox is a constant and no column count has to be kept in step
/// across the language boundary. `out` is cleared rather than reallocated, which
/// is what keeps the visualizer's per-frame heap traffic to the `SharedString`
/// the caller hands to Slint.
///
/// This builds its [`XPrefixes`] per call, so it is for the callers that draw a
/// figure once — the resting trace, and the tests. The per-frame path is
/// [`WaveformAnalyzer::trace`], which keeps the table across ticks; both reach
/// the same writer, so neither can drift.
///
/// A silent column is drawn [`MIN_HALF_THICKNESS`] tall either side of the axis
/// rather than flat, so a resting trace is a visible rule — the trace's
/// equivalent of the bars resting as dots — and, more importantly, so the figure
/// always encloses real area.
pub fn write_path_commands(columns: &[Column], out: &mut String) {
    let mut prefixes = XPrefixes::new();
    prefixes.fit(columns.len());
    write_path(columns, &prefixes, out);
}

/// Bytes one entry of [`XPrefixes`] takes: `"0.1234 "`.
const X_PREFIX_BYTES: usize = 7;

/// What every vertex but the first opens with. Not part of the cached entry: a
/// two-byte `push_str` costs nothing next to the float format it sits beside,
/// and keeping it here is what lets the opening `M` vertex reuse the same entry
/// rather than slice a lead off it.
///
/// The space before the `L` is not required by the SVG grammar — a command
/// letter terminates the number before it — but it costs a byte a vertex and
/// leaves nothing to the parser's discretion.
const LINE_TO: &str = " L";

/// The `x` half of every vertex the trace emits, held as text.
///
/// `x` is the column's position across the strip, so it depends on the column
/// index and the column count and nothing else — between resizes it is
/// byte-identical on every frame while the `y` beside it changes. Formatting it
/// per frame is half of all the float formatting the trace does, and the trace
/// is by a wide margin the most expensive thing the visualizer does per frame:
/// at the [`MAX_COLUMNS`] cap it writes 1024 vertices, against a whole spectrum
/// frame's two FFTs.
///
/// One packed string plus each entry's end offset, so a resize rebuilds a single
/// buffer rather than reallocating a string per column. Entries are written with
/// the same `{x:.4}` the per-frame path used to run, so the bytes are identical
/// by construction rather than by a second float formatter agreeing with
/// `core::fmt`'s rounding at every tie.
struct XPrefixes {
    /// `"0.1234 "` per column, packed end to end.
    text: String,
    /// Where each column's slice ends in [`text`](Self::text).
    ends: Vec<usize>,
}

impl XPrefixes {
    fn new() -> Self {
        Self {
            text: String::new(),
            ends: Vec::new(),
        }
    }

    fn with_capacity(columns: usize) -> Self {
        Self {
            text: String::with_capacity(columns * X_PREFIX_BYTES),
            ends: Vec::with_capacity(columns),
        }
    }

    /// Rebuild for `columns` columns, unless that is already what is held —
    /// which is every frame between resizes.
    fn fit(&mut self, columns: usize) {
        if self.ends.len() == columns {
            return;
        }
        self.text.clear();
        self.ends.clear();
        // A lone column has no span to normalize against; it lands at x = 0.
        let span = index_to_f32(columns.saturating_sub(1)).max(1.0);
        for i in 0..columns {
            let x = index_to_f32(i) / span;
            // Writing into a String cannot fail.
            let _ = write!(self.text, "{x:.4} ");
            self.ends.push(self.text.len());
        }
    }

    /// Column `i`'s `"<x> "`, or an empty slice for an index no entry covers.
    fn get(&self, i: usize) -> &str {
        let Some(&end) = self.ends.get(i) else {
            return "";
        };
        let start = i
            .checked_sub(1)
            .and_then(|previous| self.ends.get(previous))
            .copied()
            .unwrap_or_default();
        self.text.get(start..end).unwrap_or_default()
    }
}

/// The one writer. [`write_path_commands`] builds a throwaway table for the
/// callers that draw a figure once — the resting trace, and the tests;
/// [`WaveformAnalyzer::trace`] passes the table it owns. Neither has its own
/// copy of the geometry.
fn write_path(columns: &[Column], prefixes: &XPrefixes, out: &mut String) {
    out.clear();
    if columns.is_empty() {
        return;
    }

    // Lower edge left to right, then upper edge back. That order rather than the
    // reverse is what gives the closed figure a *positive* signed area, which is
    // what Slint's femtovg renderer reads to decide whether a subpath is solid
    // or a hole — and a lone subpath handed over as a hole is not a thing to
    // rely on the renderer being sensible about.
    for (i, column) in columns.iter().enumerate() {
        let y = edges(*column).1;
        // The figure opens on a move rather than a line; every vertex after it
        // is a line to.
        out.push_str(if i == 0 { "M" } else { LINE_TO });
        out.push_str(prefixes.get(i));
        let _ = write!(out, "{y:.3}");
    }
    for (i, column) in columns.iter().enumerate().rev() {
        let y = edges(*column).0;
        out.push_str(LINE_TO);
        out.push_str(prefixes.get(i));
        let _ = write!(out, "{y:.3}");
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

/// Holds the sample window, the drawn columns and the x half of the path text —
/// each allocated once at its widest and used a prefix at a time.
///
/// Usage mirrors [`SpectrumAnalyzer`]: fill [`window_mut`](Self::window_mut)
/// from the ring, then call [`trace`](Self::trace). Both take the sample rate and
/// derive the same window length from it, so there is no order-dependent state
/// between them.
///
/// [`SpectrumAnalyzer`]: super::spectrum::SpectrumAnalyzer
pub struct WaveformAnalyzer {
    window: Vec<f32>,
    columns: Vec<Column>,
    prefixes: XPrefixes,
}

impl WaveformAnalyzer {
    /// Build an analyzer that can snapshot up to `window_cap` samples and draw
    /// up to `column_cap` columns. In production `window_cap` is
    /// [`RING_CAP`](super::visualizer::RING_CAP) — a window wider than the ring
    /// would only be padded with silence.
    ///
    /// Both are floored, at two samples and one column, because everything
    /// downstream clamps *into* these lengths and a zero-length buffer doesn't
    /// narrow those clamps — it makes them panic. `window_len`'s
    /// `clamp(2, self.window.len())` and `analyze`'s
    /// `clamp(1, self.columns.len())` would both hand `usize::clamp` a `min`
    /// above its `max`, and `min_max_columns` would divide by a bucket count of
    /// zero. Cheaper to floor once here than to guard each of them.
    #[must_use]
    pub fn new(window_cap: usize, column_cap: usize) -> Self {
        Self {
            window: vec![0.0; window_cap.max(2)],
            columns: vec![Column::default(); column_cap.max(1)],
            prefixes: XPrefixes::with_capacity(column_cap.max(1)),
        }
    }

    /// Samples to snapshot at `sample_rate`: the drawn span plus the trigger's
    /// slack, capped at what the buffer (and so the ring) can hold. Above about
    /// 192 kHz the cap bites and the trace narrows in time; nothing breaks, it
    /// just shows less.
    fn window_len(&self, sample_rate: u32) -> usize {
        samples_for_ms(sample_rate, WAVE_SPAN_MS + TRIGGER_SLACK_MS).clamp(2, self.window.len())
    }

    /// The buffer the next [`trace`](Self::trace) or [`analyze`](Self::analyze)
    /// call reads. Fill it with the most recent samples, oldest first.
    pub fn window_mut(&mut self, sample_rate: u32) -> &mut [f32] {
        let len = self.window_len(sample_rate);
        &mut self.window[..len]
    }

    /// Produce this frame's columns, at most `columns` of them, without drawing
    /// them. The seam the tests reduce against — production goes through
    /// [`trace`](Self::trace), which is these columns plus the path they make.
    ///
    /// `active` is false when there is nothing to draw from — a paused player, a
    /// hidden window, a track that hasn't started. The ring still holds the last
    /// window of audio then, so re-reading it would freeze the trace mid-shape;
    /// instead the previous one collapses toward the centre line, which is both
    /// the better look and what lets the caller's idle check eventually stop the
    /// driving timer.
    pub fn analyze(&mut self, active: bool, sample_rate: u32, columns: usize) -> &[Column] {
        let width = self.fill_columns(active, sample_rate, columns);
        &self.columns[..width]
    }

    /// This frame's columns, written into the path commands Slint renders.
    ///
    /// The production path, and why it isn't [`analyze`](Self::analyze) followed
    /// by [`write_path_commands`]: the borrow `analyze` hands back keeps the
    /// analyzer mutably borrowed, so the caller couldn't then reach the prefix
    /// table it owns. Composing them here borrows the two fields disjointly.
    pub fn trace(
        &mut self,
        active: bool,
        sample_rate: u32,
        columns: usize,
        out: &mut String,
    ) -> &[Column] {
        let width = self.fill_columns(active, sample_rate, columns);
        self.prefixes.fit(width);
        write_path(&self.columns[..width], &self.prefixes, out);
        &self.columns[..width]
    }

    /// Resolve this frame's columns into the buffer and return how many are
    /// drawn. The body both public entry points share.
    fn fill_columns(&mut self, active: bool, sample_rate: u32, columns: usize) -> usize {
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
                column.min *= VISUALIZER_DECAY;
                column.max *= VISUALIZER_DECAY;
            }
        }
        width
    }
}

#[cfg(test)]
#[path = "tests/waveform_tests.rs"]
mod tests;
