//! A headphone correction's filters as the DSP takes them, checked once where they enter, so a
//! hand-edited settings file or an imported profile can't hand the audio thread a curve it can't
//! run.

use std::ops::RangeInclusive;

/// The most filters one correction holds; a set with more is refused, never cut.
pub const MAX_CORRECTION_FILTERS: usize = 20;

/// Not bounded by any one sample rate: a filter at or past a source's Nyquist sits out at that
/// rate, so one profile serves every rate.
pub const FREQUENCY_RANGE_HZ: RangeInclusive<f64> = 10.0..=24_000.0;

/// Wider than a profile's optimiser writes, so the bound refuses a typo rather than a curve.
pub const GAIN_RANGE_DB: RangeInclusive<f64> = -30.0..=30.0;

/// Above zero, where the filter's bandwidth term is defined.
pub const Q_RANGE: RangeInclusive<f64> = 0.05..=20.0;

/// The filter shapes a correction runs: the RBJ cookbook's, which the profiles are fitted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    Peaking,
    LowShelf,
    HighShelf,
}

/// One filter as a profile states it. Plain data; [`CorrectionFilters`] is where it is checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorrectionFilter {
    pub kind: FilterKind,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

impl CorrectionFilter {
    /// A NaN or an infinity sits in no range, so it is refused with the rest.
    pub fn check(&self) -> Result<(), FilterError> {
        if !FREQUENCY_RANGE_HZ.contains(&self.freq_hz) {
            return Err(FilterError::Frequency(self.freq_hz));
        }
        if !GAIN_RANGE_DB.contains(&self.gain_db) {
            return Err(FilterError::Gain(self.gain_db));
        }
        if !Q_RANGE.contains(&self.q) {
            return Err(FilterError::Q(self.q));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum FilterError {
    #[error(
        "filter frequency {0} Hz is outside {low} to {high} Hz",
        low = FREQUENCY_RANGE_HZ.start(),
        high = FREQUENCY_RANGE_HZ.end()
    )]
    Frequency(f64),

    #[error(
        "filter gain {0} dB is outside {low} to {high} dB",
        low = GAIN_RANGE_DB.start(),
        high = GAIN_RANGE_DB.end()
    )]
    Gain(f64),

    #[error("filter Q {0} is outside {low} to {high}", low = Q_RANGE.start(), high = Q_RANGE.end())]
    Q(f64),
}

/// One to [`MAX_CORRECTION_FILTERS`] filters, each in range: the only form the DSP accepts.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionFilters(Box<[CorrectionFilter]>);

impl CorrectionFilters {
    pub fn filters(&self) -> &[CorrectionFilter] {
        &self.0
    }
}

impl TryFrom<Vec<CorrectionFilter>> for CorrectionFilters {
    type Error = FilterSetError;

    fn try_from(filters: Vec<CorrectionFilter>) -> Result<Self, FilterSetError> {
        if filters.is_empty() {
            return Err(FilterSetError::Empty);
        }
        if filters.len() > MAX_CORRECTION_FILTERS {
            return Err(FilterSetError::TooMany(filters.len()));
        }
        for (index, filter) in filters.iter().enumerate() {
            filter.check().map_err(|source| FilterSetError::Filter { index, source })?;
        }
        Ok(Self(filters.into_boxed_slice()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum FilterSetError {
    #[error("a correction needs at least one filter")]
    Empty,

    #[error("{0} filters is more than the {MAX_CORRECTION_FILTERS} a correction holds")]
    TooMany(usize),

    #[error("filter {} is out of range", .index + 1)]
    Filter { index: usize, source: FilterError },
}
