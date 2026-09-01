//! Everything below the DSP chain: the device stream, the voices, the sum, and the clock.
//!
//! The chain above is untouched by any of it — the ordering, the ramp, the tap and the limiter read
//! the same samples either way. What this tree owns is the four things underneath, and each argues
//! itself where it lives: [`convert`] for the rate, the channel map and the speed ratio, [`voice`]
//! for pause, volume and the clock, [`mixer`] for the unclamped sum, [`device`] for the stream and
//! the ladder that opens it.
//!
//! [`AudioOutput`] is the whole public surface: build it, hand [`Mixer`] to the decks, keep it
//! alive. `tests/crossfade.rs` skips it and drives [`mixer::pair`] directly, which is what lets the
//! full chain be tested with no sound card.

pub mod convert;
pub mod device;
pub mod mixer;
pub mod voice;

use crate::error::AppError;

use self::device::{DeviceStream, Negotiated};
use self::mixer::Mixer;
use super::audio::Shape;

/// The open device and the voices feeding it.
///
/// The stream goes first, which is what stops the callback: the voices it pulls live inside its own
/// closure. Nothing dangles the other way round, a voice's two halves sharing one `Arc`, but a
/// control op issued against a stopped callback waits out its whole timeout, so the order stays.
pub struct AudioOutput {
    stream: DeviceStream,
    mixer: Mixer,
}

impl AudioOutput {
    /// Open the default device and build `voices` of them against whatever it negotiated.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when no config the device offers can be opened.
    pub fn open<E>(voices: usize, error_callback: E) -> Result<Self, AppError>
    where
        E: FnMut(cpal::StreamError) + Clone + Send + 'static,
    {
        let (stream, mixer) =
            device::open(|shape: Shape| mixer::pair(voices, shape), error_callback)?;
        Ok(Self { stream, mixer })
    }

    /// The voices, as one mixer. Handed to `player::decks` at boot and not reachable any other way.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// What the device agreed to, as opposed to what it was asked for.
    pub fn negotiated(&self) -> Negotiated {
        self.stream.negotiated()
    }
}
