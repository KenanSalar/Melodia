//! The loudness targets a gain is reckoned against, one home for each so a measured gain and a
//! tagged one level to the same place.

use rusqlite::types::{FromSql, FromSqlResult, ValueRef};

/// The integrated loudness `ReplayGain` 2.0 levels a track to, and so what every gain the library
/// stores is relative to.
pub const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;

/// The integrated loudness an Opus `R128_*_GAIN` tag levels to, fixed by RFC 7845 §5.2.
pub const R128_REFERENCE_LUFS: f64 = -23.0;

/// Where a track's gains came from: the file's own tags, or Melodia's measurement of an untagged
/// file, which plays only while measuring is switched on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GainOrigin {
    #[default]
    Tagged,
    Measured,
}

/// Read from whether a measurement stood in for the tags.
impl FromSql for GainOrigin {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let measured = bool::column_result(value)?;
        Ok(if measured { Self::Measured } else { Self::Tagged })
    }
}
