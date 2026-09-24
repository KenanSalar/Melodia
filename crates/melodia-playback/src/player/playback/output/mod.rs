//! Everything below the DSP chain: the device stream, the voices, the sum, and the clock.
//!
//! The chain above is untouched by any of it — the ordering, the ramp, the tap and the limiter read
//! the same samples either way. What this tree owns is the four things underneath, and each argues
//! itself where it lives: [`convert`] for the rate, the channel map and the speed ratio, [`voice`]
//! for pause, volume and the clock, [`mixer`] for the unclamped sum, [`device`] for the stream and
//! the ladder that opens it.
//!
//! [`AudioOutput`] is the whole public surface: build it, hand [`Mixer`] to the decks, reopen it
//! when the device goes away. `crates/melodia/tests/crossfade.rs` skips it and drives
//! [`mixer::pair`] directly, which is what lets the full chain be tested with no sound card.

pub mod convert;
pub mod device;
pub mod mixer;
pub mod voice;

use std::sync::Arc;

use melodia_core::error::AppError;

use self::device::{DeviceStream, Feed, Negotiated};
use self::mixer::Mixer;
use super::stream_health::AudioStreamHealth;

/// The open device and the voices feeding it.
///
/// **A reopen replaces the stream and nothing else.** The puller lives here rather than inside the
/// stream's callback, so dropping a stream hands nothing back and loses nothing: the decks, what
/// they hold, their ramps and their clocks all carry on into the next stream, reshaped to whatever
/// it negotiated.
///
/// The stream goes first, which is what stops the callback: a control op issued against a stopped
/// callback waits out its whole timeout, so the order stays.
pub struct AudioOutput {
    /// `None` once a reopen has failed, until one succeeds.
    stream: Option<DeviceStream>,
    feed: Feed,
    mixer: Mixer,
}

impl AudioOutput {
    /// Open the default device and build `voices` many against whatever it negotiated, reporting
    /// stream faults into `health`.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no device, or no config it offers can be opened.
    pub fn open(voices: usize, health: Arc<AudioStreamHealth>) -> Result<Self, AppError> {
        let target = device::default_target()?;
        let (mixer, pull) = mixer::pair(voices, target.default_shape()?);
        let feed = Feed::new(pull, health);
        let stream = device::open(&target, &feed)?;
        Ok(Self { stream: Some(stream), feed, mixer })
    }

    /// Replace the stream with one on whatever the default device is now.
    ///
    /// A failure leaves the output with no stream at all rather than the dead one, so the next
    /// attempt starts clean.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no device to open, or it refuses every config.
    pub fn reopen(&mut self) -> Result<Negotiated, AppError> {
        self.stream = None;
        // After the drop and before the open, which is the one window where no stream can report:
        // a loss still flagged here belongs to the stream just dropped, and left standing it would
        // reopen the new one for nothing.
        self.feed.health.take_device_lost();

        let target = device::default_target()?;
        let stream = device::open(&target, &self.feed)?;
        let negotiated = stream.negotiated();
        self.stream = Some(stream);
        Ok(negotiated)
    }

    /// The voices, as one mixer. Handed to `player::playback::decks` at boot and not reachable any other way.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// What the device agreed to, as opposed to what it was asked for, or `None` while no stream
    /// is open.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.stream.as_ref().map(DeviceStream::negotiated)
    }
}
