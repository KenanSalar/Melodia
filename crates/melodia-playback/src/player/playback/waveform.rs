//! Oscilloscope trace for the visualizer: the signal itself, so no window, FFT or banding.
//!
//! A column is a range rather than a point. Several samples share one, and the loudest scribbles,
//! the nearest aliases and the mean lowpasses, so a [`Column`] carries the whole `min`..`max` and
//! the strip fills between the edges, aliasing nothing. Free functions over slices;
//! [`WaveformAnalyzer`] only holds the buffers.

use super::dsp::{VISUALIZER_DECAY, index_to_f32, push_fixed};

/// Time rather than samples, so a track draws the same at any rate; short enough that a bass note
/// still reads as a wave at the strip's height.
pub const WAVE_SPAN_MS: u32 = 40;

/// Extra milliseconds snapshotted *ahead* of the drawn span, giving [`find_trigger`] somewhere
/// to look. Also the trace's worst-case latency.
pub const TRIGGER_SLACK_MS: u32 = 20;

/// Indistinguishable from one under the stroke, and halves the path string, its re-parse and its
/// tessellation.
const LOGICAL_PX_PER_COLUMN: f32 = 2.0;

/// The floor keeps a narrow strip from a handful of wide columns; the ceiling is the widest strip
/// the view can ask for, and what the buffer is sized to.
const MIN_COLUMNS: usize = 64;
pub const MAX_COLUMNS: usize = 1024;

/// How far below zero the signal must dip before a crossing back up counts. Without it the
/// trigger latches onto noise around the axis and jitters.
const TRIGGER_HYSTERESIS: f32 = 0.02;

/// In viewbox units. A column whose edges meet encloses nothing, handing the renderer an outline
/// lying on itself; this keeps the strokes overlapping into one line without ever coinciding.
const MIN_HALF_THICKNESS: f32 = 0.018;

/// Saturates; the caller clamps to its buffer anyway.
fn samples_for_ms(sample_rate: u32, ms: u32) -> usize {
    usize::try_from(u64::from(sample_rate) * u64::from(ms) / 1000).unwrap_or(usize::MAX)
}

/// The range of sample values one drawn column covers. `min <= max` always holds out of
/// [`min_max_columns`], which is what keeps the figure from folding over itself.
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

/// The most recent rising zero crossing in `samples[..search_len]`, keeping the span as live as
/// the slack allows. `0` draws untriggered, right for silence and a window on one side of the axis.
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

/// Reduce `src` to one [`Column`] per slot of `out`, oldest first. More slots than samples holds
/// the nearest sample, giving a flat column rather than a gap.
pub fn min_max_columns(src: &[f32], out: &mut [Column]) {
    if src.is_empty() {
        out.fill(Column::default());
        return;
    }
    let buckets = out.len();
    for (i, column) in out.iter_mut().enumerate() {
        let lo = i * src.len() / buckets;
        // At least one sample per bucket, so upsampling holds rather than reading an empty
        // range — which is also what guarantees the fold below sees a value.
        let hi = ((i + 1) * src.len() / buckets).max(lo + 1).min(src.len());
        let mut range = Column { min: f32::MAX, max: f32::MIN };
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

/// One filled figure, normalized to a constant viewbox so no column count crosses into Slint.
/// Builds its prefixes per call, for a figure drawn once; [`WaveformAnalyzer::trace`] keeps them.
pub fn write_path_commands(columns: &[Column], out: &mut String) {
    let mut prefixes = XPrefixes::new();
    prefixes.fit(columns.len());
    write_path(columns, &prefixes, out);
}

/// Fractional digits each vertex's `x` is written with.
const X_DECIMALS: u32 = 4;

/// Fractional digits each vertex's `y` is written with.
const Y_DECIMALS: u32 = 3;

/// Bytes one entry of [`XPrefixes`] takes: `"0.1234 "`, a lead digit and point, [`X_DECIMALS`]
/// digits and the space.
const X_PREFIX_BYTES: usize = 7;

/// Kept out of the cached entry, so the opening `M` vertex reuses it.
const LINE_TO: &str = " L";

/// The `x` half of every vertex as text, byte-identical between resizes, packed in one string.
/// Through [`push_fixed`] like the per-frame `y`, so both halves round by one rule.
struct XPrefixes {
    /// `"0.1234 "` per column, packed end to end.
    text: String,
    /// Where each column's slice ends in [`text`](Self::text).
    ends: Vec<usize>,
}

impl XPrefixes {
    fn new() -> Self {
        Self { text: String::new(), ends: Vec::new() }
    }

    fn with_capacity(columns: usize) -> Self {
        Self {
            text: String::with_capacity(columns * X_PREFIX_BYTES),
            ends: Vec::with_capacity(columns),
        }
    }

    /// A no-op between resizes.
    fn fit(&mut self, columns: usize) {
        if self.ends.len() == columns {
            return;
        }
        self.text.clear();
        self.ends.clear();
        // A lone column has no span to normalize against; it lands at x = 0.
        let span = index_to_f32(columns.saturating_sub(1)).max(1.0);
        for i in 0..columns {
            push_fixed::<X_DECIMALS>(&mut self.text, index_to_f32(i) / span);
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

/// The one writer, so neither caller holds a copy of the geometry.
fn write_path(columns: &[Column], prefixes: &XPrefixes, out: &mut String) {
    out.clear();
    if columns.is_empty() {
        return;
    }

    // Lower edge first gives a positive signed area, which femtovg reads as solid.
    for (i, column) in columns.iter().enumerate() {
        let y = edges(*column).1;
        out.push_str(if i == 0 { "M" } else { LINE_TO });
        out.push_str(prefixes.get(i));
        push_fixed::<Y_DECIMALS>(out, y);
    }
    for (i, column) in columns.iter().enumerate().rev() {
        let y = edges(*column).0;
        out.push_str(LINE_TO);
        out.push_str(prefixes.get(i));
        push_fixed::<Y_DECIMALS>(out, y);
    }
    out.push('Z');
}

/// `(upper, lower)` on screen, flipped so peaks draw on top. [`MIN_HALF_THICKNESS`] opens a
/// near-silent column about its own midpoint.
fn edges(column: Column) -> (f32, f32) {
    let mid = 0.5 * (column.min + column.max);
    let half = (0.5 * (column.max - column.min)).max(MIN_HALF_THICKNESS);
    let flipped = 0.0 - mid;
    (flipped - half, flipped + half)
}

/// Buffers allocated once at their widest and used a prefix at a time. Fill
/// [`window_mut`](Self::window_mut), then [`trace`](Self::trace); both derive the length from the
/// rate, so no state sits between them.
pub struct WaveformAnalyzer {
    window: Vec<f32>,
    columns: Vec<Column>,
    prefixes: XPrefixes,
}

impl WaveformAnalyzer {
    /// `window_cap` is [`RING_CAP`](super::visualizer::RING_CAP) in production. Both are floored,
    /// a zero-length buffer making the clamps downstream panic.
    #[must_use]
    pub fn new(window_cap: usize, column_cap: usize) -> Self {
        Self {
            window: vec![0.0; window_cap.max(2)],
            columns: vec![Column::default(); column_cap.max(1)],
            prefixes: XPrefixes::with_capacity(column_cap.max(1)),
        }
    }

    /// Capped at the ring, so at extreme rates the trace narrows in time.
    fn window_len(&self, sample_rate: u32) -> usize {
        samples_for_ms(sample_rate, WAVE_SPAN_MS + TRIGGER_SLACK_MS).clamp(2, self.window.len())
    }

    /// Filled with the most recent samples, oldest first.
    pub fn window_mut(&mut self, sample_rate: u32) -> &mut [f32] {
        let len = self.window_len(sample_rate);
        &mut self.window[..len]
    }

    /// The columns without their path, for the tests. Inactive (paused, hidden, not started), the
    /// last trace collapses toward the centre line rather than freezing, so the caller's idle check
    /// can stop its timer.
    pub fn analyze(&mut self, active: bool, sample_rate: u32, columns: usize) -> &[Column] {
        let width = self.fill_columns(active, sample_rate, columns);
        &self.columns[..width]
    }

    /// Not `analyze` plus [`write_path_commands`] at the call site, whose borrow would lock the
    /// caller out of the prefix table.
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

    fn fill_columns(&mut self, active: bool, sample_rate: u32, columns: usize) -> usize {
        let width = columns.clamp(1, self.columns.len());
        if active {
            let window = self.window_len(sample_rate);
            let span = samples_for_ms(sample_rate, WAVE_SPAN_MS).clamp(1, window);
            let trigger = find_trigger(&self.window[..window], window - span);
            let drawn = &self.window[trigger..trigger + span];
            min_max_columns(drawn, &mut self.columns[..width]);
        } else {
            // The whole buffer, not just the drawn prefix: the strip can be resized while
            // paused, and a column that widened back into view undecayed would pop.
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
