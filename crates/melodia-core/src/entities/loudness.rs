//! The loudness targets a gain is reckoned against, one home for each so a measured gain and a
//! tagged one level to the same place.

/// The integrated loudness `ReplayGain` 2.0 levels a track to, and so what every gain the library
/// stores is relative to.
pub const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;

/// The integrated loudness an Opus `R128_*_GAIN` tag levels to, fixed by RFC 7845 §5.2.
pub const R128_REFERENCE_LUFS: f64 = -23.0;
