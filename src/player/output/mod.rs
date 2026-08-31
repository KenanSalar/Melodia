//! Everything below the DSP chain: the device stream, the decks, the sum, and the clock.
//!
//! This is what rodio used to hold. The chain above it is untouched — the ordering, the ramp, the
//! tap and the limiter are all the same code reading the same samples — and what changes is that
//! the four things underneath now answer to this tree:
//!
//! - **The rate conversion is visible.** rodio did it inside `Mixer::add`, once, against whichever
//!   source reached a deck first, which is why a source that named no span pinned every later one
//!   to the first's rate. [`convert`] is per source and rebuilt when a deck advances.
//! - **Playback speed is a ratio rather than a lie about the sample rate**, so a position is media
//!   time at every level and there is only one timeline to reason about.
//! - **The device handle is owned.** rodio's had to be leaked to outlive the decks; the drop order
//!   here says the same thing without giving up the ability to close the device.
//! - **The sum is ours**, and stays unclamped — [`mixer`] argues why.
//!
//! [`AudioOutput`] is the whole public surface: build it, hand [`Mixer`] to the decks, keep it
//! alive. `tests/crossfade.rs` skips it and drives [`mixer::pair`] directly, which is what lets the
//! full chain be tested with no sound card.

pub mod convert;
pub mod deck;
pub mod device;
pub mod mixer;

use crate::error::AppError;

use self::convert::Shape;
use self::device::{DeviceStream, Negotiated};
use self::mixer::Mixer;

/// The open device and the decks feeding it.
///
/// Field order is the drop order and it is load-bearing: the stream goes first, so the callback has
/// stopped before the decks it pulls from are torn down.
pub struct AudioOutput {
    stream: DeviceStream,
    mixer: Mixer,
}

impl AudioOutput {
    /// Open the default device and build `voices` decks against whatever it negotiated.
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

    /// The decks. Handed to `player::decks` at boot and not reachable any other way.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// What the device agreed to, as opposed to what it was asked for.
    pub fn negotiated(&self) -> Negotiated {
        self.stream.negotiated()
    }
}
