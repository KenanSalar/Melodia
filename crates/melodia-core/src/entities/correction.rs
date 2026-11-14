//! A headphone correction's filters as the DSP takes them, checked once where they enter, so a
//! hand-edited settings file or an imported profile can't hand the audio thread a curve it can't
//! run.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    Peaking,
    LowShelf,
    HighShelf,
}

/// One filter as a profile states it. Plain data; [`CorrectionFilters`] holds only checked ones.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrectionFilter {
    pub kind: FilterKind,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

impl CorrectionFilter {
    /// Refuses a value outside its range, a NaN or an infinity among them.
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<CorrectionFilter>")]
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

/// One device's correction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Correction {
    pub origin: CorrectionOrigin,
    /// The name the device was listed under when this was set, to name it by while it's unplugged.
    /// `None` for System default.
    pub device_name: Option<String>,
    pub filters: CorrectionFilters,
}

/// Where a correction came from, which names it too: a paste has no name of its own.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CorrectionOrigin {
    /// `source` is whose measurement it is.
    AutoEq {
        model: String,
        source: String,
    },
    File {
        name: String,
    },
    Paste,
}

/// Every device's correction, keyed by the device the output is aimed at.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceCorrections {
    /// Its own field rather than a key in `devices`, which no device id can then collide with.
    pub system_default: Option<Correction>,
    /// By `OutputDevice::id`.
    pub devices: BTreeMap<String, Correction>,
}

impl DeviceCorrections {
    /// `None` names System default.
    pub fn for_device(&self, device: Option<&str>) -> Option<&Correction> {
        match device {
            Some(id) => self.devices.get(id),
            None => self.system_default.as_ref(),
        }
    }
}
