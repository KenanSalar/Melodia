//! Everything from the DSP chain down to the device, naming cpal and never the network, the
//! inverse of `melodia-audio`. `output` is everything under the chain, so it stays nested.

pub mod player {
    pub mod playback;
}
