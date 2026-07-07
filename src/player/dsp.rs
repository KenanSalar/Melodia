//! Tiny DSP math shared by the equalizer and `ReplayGain` paths.

/// Convert a decibel value to a linear amplitude factor.
pub(crate) fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
