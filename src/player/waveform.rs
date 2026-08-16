//! Oscilloscope trace for the audio visualizer — the signal itself rather than
//! its spectrum, so it skips [`spectrum`] entirely: no window, no FFT, no banding.
//!
//! # A column is a range, not a point
//!
//! Several samples always share a drawn column, and every way of picking one of
//! them is wrong in its own way: the loudest alternates between opposite
//! extremes and scribbles, the nearest aliases, the mean lowpasses anything
//! bright. So a [`Column`] carries the whole `min`..`max` its samples covered
//! and the strip is the area between the two edges — nothing skipped, so nothing
//! can alias. `DeaDBeeF`'s scope (`ddb_scope_point_t { ymin, ymax }`) and every
//! audio editor's waveform view resolve it the same way.
//!
//! Per drawn frame: [`find_trigger`] over the leading part of the snapshot,
//! [`min_max_columns`] over the span behind it, then the two edges closed into
//! the one filled figure Slint's `Path` renders. Like [`spectrum`] the work is
//! free functions over slices; [`WaveformAnalyzer`] only holds the buffers that
//! must not be reallocated per frame, and its [`trace`](WaveformAnalyzer::trace)
//! is what production calls.
//!
//! [`spectrum`]: super::spectrum

use std::fmt::Write as _;

use super::dsp::{VISUALIZER_DECAY, index_to_f32};

/// Milliseconds of audio drawn across the strip.
///
/// Time, not samples — a fixed sample count would draw the same music
/// differently depending on the rate it was mastered at. Sits between
/// `DeaDBeeF`'s 50 ms and `foobar2000`'s 100, low enough that a bass note still
/// reads as a wave rather than a blur at the strip's height.
pub const WAVE_SPAN_MS: u32 = 40;

/// Extra milliseconds snapshotted *ahead* of the drawn span, giving
/// [`find_trigger`] somewhere to look. Also the trace's worst-case latency.
pub const TRIGGER_SLACK_MS: u32 = 20;

/// Logical pixels per drawn column.
///
/// `DeaDBeeF` uses one per pixel. One per two is indistinguishable under the
/// envelope's own stroke and halves the path string, its re-parse and the
/// tessellation behind it — which matters here in a way it doesn't for a scope
/// drawing straight to a canvas.
const LOGICAL_PX_PER_COLUMN: f32 = 2.0;

/// Column bounds. The floor keeps a narrow strip from degenerating into a
/// handful of very wide columns; the ceiling bounds the per-frame path string
/// and is what the analyzer's buffer is sized to.
///
/// The ceiling is the widest strip the view can ask for at
/// [`LOGICAL_PX_PER_COLUMN`] — its width is capped against its own height and
/// its height capped outright, so nothing past this is reachable. Set below it,
/// the trace redraws the same columns stretched wider as the window grows.
const MIN_COLUMNS: usize = 64;
pub const MAX_COLUMNS: usize = 1024;

/// How far below zero the signal must dip before a crossing back up counts.
/// Without it the trigger latches onto noise around the axis and jitters.
const TRIGGER_HYSTERESIS: f32 = 0.02;

/// Half-thickness, in viewbox units, that a drawn column never falls below.
///
/// A column whose edges land on each other encloses no area, so a silent trace
/// hands the renderer an outline lying on top of itself — geometry nothing owes
/// you anything sensible for, and in practice what drew the resting line as
/// dashes.
///
/// Sized against the stroke rather than for its own sake: close enough that the
/// two edges' strokes still overlap and a resting trace reads as one line rather
/// than a pair of rails, far enough that they are nowhere near coincident. Both
/// it and the stroke are in viewbox units, so the pair holds at every height.
const MIN_HALF_THICKNESS: f32 = 0.018;

/// Samples spanning `ms` at `sample_rate`. Saturates rather than wrapping; the
/// caller clamps to its buffer anyway.
fn samples_for_ms(sample_rate: u32, ms: u32) -> usize {
    usize::try_from(u64::from(sample_rate) * u64::from(ms) / 1000).unwrap_or(usize::MAX)
}

/// The range of sample values one drawn column covers. `min <= max` always
/// holds out of [`min_max_columns`], which is what keeps the figure from
/// folding over itself.
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
/// Most recent rather than first keeps the drawn span as close to live as the
/// slack allows; every candidate is a crossing of the *same* polarity, so the
/// choice changes latency, not shape. The `0` fallback draws untriggered, which
/// is the right answer for the two signals with no crossing to find: silence,
/// and a window sitting entirely on one side of the axis.
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

/// Reduce `src` to one [`Column`] per slot of `out`, oldest first. More slots
/// than samples holds the nearest sample, giving a flat column rather than a gap.
pub fn min_max_columns(src: &[f32], out: &mut [Column]) {
    if src.is_empty() {
        out.fill(Column::default());
        return;
    }
    let buckets = out.len();
    for (i, column) in out.iter_mut().enumerate() {
        let lo = i * src.len() / buckets;
        // At least one sample per bucket, so upsampling holds rather than
        // reading an empty range — which is also what guarantees the fold below
        // sees a value.
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
/// Coordinates are normalized — x across `0..1`, y over `-1..1` — so the
/// `Path`'s viewbox is a constant and no column count crosses the language
/// boundary. `out` is cleared rather than reallocated.
///
/// Builds its [`XPrefixes`] per call, so it is for the callers that draw a
/// figure once — the resting trace, and the tests. [`WaveformAnalyzer::trace`]
/// keeps the table across ticks instead; both reach the same writer.
pub fn write_path_commands(columns: &[Column], out: &mut String) {
    let mut prefixes = XPrefixes::new();
    prefixes.fit(columns.len());
    write_path(columns, &prefixes, out);
}

/// Bytes one entry of [`XPrefixes`] takes: `"0.1234 "`.
const X_PREFIX_BYTES: usize = 7;

/// Append `value` to `out` with exactly `DECIMALS` fractional digits.
///
/// The trace's coordinates are all normalized and quantized to a few places, so
/// the format is a sign, one digit and a zero-padded remainder. `{value:.N$}`
/// would instead ask `core::fmt` for the exactly-rounded decimal — Grisu's
/// `format_exact` with a bignum fallback — twice per column per frame. Integer
/// scale and integer print is the same bytes for a fraction of the work.
///
/// `DECIMALS` is a const parameter so the scale is a compile-time constant and a
/// request too wide for a `u16` overflows the `const` block rather than needing
/// a runtime clamp. That diagnostic arrives at **codegen**: `cargo build` and
/// `cargo test` reject `push_fixed::<5>` with `E0080`, `cargo clippy` — this
/// repo's usual gate — stops before codegen and passes it.
///
/// Two deliberate differences from `core::fmt`, each worth one unit in the last
/// place on a viewbox two units tall: negative zero prints as `0`, and an exact
/// tie rounds away from zero rather than to even.
fn push_fixed<const DECIMALS: u32>(out: &mut String, value: f32) {
    let scale = const { 10u16.pow(DECIMALS) };

    #[expect(
        clippy::cast_possible_truncation,
        reason = "coordinates are normalized to a unit range, so the scaled value is at most ±10_000; a float→int `as` saturates besides, so even a forged input clamps rather than wrapping"
    )]
    let units = (value * f32::from(scale)).round() as i32;

    if units < 0 {
        out.push('-');
    }
    let magnitude = units.unsigned_abs();
    let scale = u32::from(scale);
    let width = DECIMALS as usize;
    // Writing into a String cannot fail.
    let _ = write!(out, "{}.{:0width$}", magnitude / scale, magnitude % scale);
}

/// What every vertex but the first opens with. Kept out of the cached entry so
/// the opening `M` vertex can reuse the same entry rather than slice a lead off
/// it. The space isn't required by the SVG grammar — a command letter terminates
/// the number before it — but it leaves nothing to the parser's discretion.
const LINE_TO: &str = " L";

/// The `x` half of every vertex the trace emits, held as text.
///
/// `x` depends on the column index and the column count and nothing else, so
/// between resizes it is byte-identical every frame while the `y` beside it
/// changes — and formatting it is half of all the coordinate writing the trace
/// does. One packed string plus each entry's end offset, so a resize rebuilds
/// one buffer rather than a string per column. Entries go through
/// [`push_fixed`], the same writer the per-frame `y` takes, so both halves of a
/// vertex round by one rule rather than two that have to agree at every tie.
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
            push_fixed::<4>(&mut self.text, index_to_f32(i) / span);
            self.text.push(' ');
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

/// The one writer, shared by [`write_path_commands`] and
/// [`WaveformAnalyzer::trace`] so neither holds its own copy of the geometry.
fn write_path(columns: &[Column], prefixes: &XPrefixes, out: &mut String) {
    out.clear();
    if columns.is_empty() {
        return;
    }

    // Lower edge left to right, then upper edge back — that order, not the
    // reverse, is what gives the closed figure a *positive* signed area, which
    // is what femtovg reads to decide solid from hole. Pinned by
    // `the_closed_figure_winds_positively`.
    for (i, column) in columns.iter().enumerate() {
        let y = edges(*column).1;
        out.push_str(if i == 0 { "M" } else { LINE_TO });
        out.push_str(prefixes.get(i));
        push_fixed::<3>(out, y);
    }
    for (i, column) in columns.iter().enumerate().rev() {
        let y = edges(*column).0;
        out.push_str(LINE_TO);
        out.push_str(prefixes.get(i));
        push_fixed::<3>(out, y);
    }
    out.push('Z');
}

/// A column's two screen-space edges, `(upper, lower)`, flipped because screen
/// coordinates grow downward and a scope draws positive peaks at the top.
/// [`MIN_HALF_THICKNESS`] is applied about the column's own midpoint, so a loud
/// asymmetric column keeps its centre and only a near-silent one is opened up.
fn edges(column: Column) -> (f32, f32) {
    let mid = 0.5 * (column.min + column.max);
    let half = (0.5 * (column.max - column.min)).max(MIN_HALF_THICKNESS);
    let flipped = 0.0 - mid;
    (flipped - half, flipped + half)
}

/// The sample window, the drawn columns and the x half of the path text — each
/// allocated once at its widest and used a prefix at a time.
///
/// Usage mirrors [`SpectrumAnalyzer`]: fill [`window_mut`](Self::window_mut)
/// from the ring, then call [`trace`](Self::trace). Both derive the window
/// length from the sample rate, so no order-dependent state sits between them.
///
/// [`SpectrumAnalyzer`]: super::spectrum::SpectrumAnalyzer
pub struct WaveformAnalyzer {
    window: Vec<f32>,
    columns: Vec<Column>,
    prefixes: XPrefixes,
}

impl WaveformAnalyzer {
    /// Snapshots up to `window_cap` samples and draws up to `column_cap`
    /// columns. In production `window_cap` is
    /// [`RING_CAP`](super::visualizer::RING_CAP) — a wider window would only be
    /// padded with silence.
    ///
    /// Both are floored because everything downstream clamps *into* these
    /// lengths, and a zero-length buffer doesn't narrow those clamps, it makes
    /// them panic (a `min` above its `max`, and a divide by a zero bucket
    /// count). Cheaper to floor once here than to guard each of them.
    #[must_use]
    pub fn new(window_cap: usize, column_cap: usize) -> Self {
        Self {
            window: vec![0.0; window_cap.max(2)],
            columns: vec![Column::default(); column_cap.max(1)],
            prefixes: XPrefixes::with_capacity(column_cap.max(1)),
        }
    }

    /// The drawn span plus the trigger's slack, capped at what the buffer — and
    /// so the ring — can hold. At extreme sample rates the cap bites and the
    /// trace simply narrows in time.
    fn window_len(&self, sample_rate: u32) -> usize {
        samples_for_ms(sample_rate, WAVE_SPAN_MS + TRIGGER_SLACK_MS).clamp(2, self.window.len())
    }

    /// The buffer the next [`trace`](Self::trace) or [`analyze`](Self::analyze)
    /// call reads. Fill it with the most recent samples, oldest first.
    pub fn window_mut(&mut self, sample_rate: u32) -> &mut [f32] {
        let len = self.window_len(sample_rate);
        &mut self.window[..len]
    }

    /// This frame's columns without the path they make — the seam the tests
    /// reduce against; production goes through [`trace`](Self::trace).
    ///
    /// `active` is false when there is nothing to draw from: a paused player, a
    /// hidden window, a track that hasn't started. The ring still holds the last
    /// window then, so re-reading it would freeze the trace mid-shape; the
    /// previous one collapses toward the centre line instead, which also lets
    /// the caller's idle check eventually stop the driving timer.
    pub fn analyze(&mut self, active: bool, sample_rate: u32, columns: usize) -> &[Column] {
        let width = self.fill_columns(active, sample_rate, columns);
        &self.columns[..width]
    }

    /// This frame's columns, written into the path commands Slint renders.
    ///
    /// Not [`analyze`](Self::analyze) plus [`write_path_commands`] at the call
    /// site, because the borrow `analyze` hands back would keep the caller out
    /// of the prefix table. Composing here borrows the two fields disjointly.
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

    /// Resolve this frame's columns into the buffer, returning how many are
    /// drawn. The body both entry points share.
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
