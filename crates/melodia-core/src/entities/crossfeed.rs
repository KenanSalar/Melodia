//! A device's crossfeed as it is kept, its strength checked where it enters so a hand edit can't
//! hand the DSP an amount past either end of the slider.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

/// Subtle at the low end, strong at the high.
pub const STRENGTH_RANGE: RangeInclusive<f32> = 0.0..=1.0;

/// Where a device's strength starts before it is ever moved.
pub const DEFAULT_STRENGTH: CrossfeedStrength = CrossfeedStrength(0.7);

/// How far crossfeed narrows the stereo image, within [`STRENGTH_RANGE`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32")]
pub struct CrossfeedStrength(f32);

impl CrossfeedStrength {
    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for CrossfeedStrength {
    fn default() -> Self {
        DEFAULT_STRENGTH
    }
}

impl TryFrom<f32> for CrossfeedStrength {
    type Error = StrengthError;

    /// Refuses a value outside the range, NaN among them.
    fn try_from(strength: f32) -> Result<Self, StrengthError> {
        if STRENGTH_RANGE.contains(&strength) {
            Ok(Self(strength))
        } else {
            Err(StrengthError(strength))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[error(
    "crossfeed strength {0} is outside {low} to {high}",
    low = STRENGTH_RANGE.start(),
    high = STRENGTH_RANGE.end()
)]
pub struct StrengthError(f32);

/// One device's crossfeed, the default (off) being what a device with no entry plays. Switched
/// off, it keeps its strength for when it is switched back on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrossfeedSetting {
    pub enabled: bool,
    pub strength: CrossfeedStrength,
}
