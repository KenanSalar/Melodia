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
use std::time::Duration;

use melodia_audio::player::source::audio::SampleRate;
use melodia_core::error::{self, AppError};

use self::device::{DeviceStream, Feed, Negotiated};
use self::mixer::Mixer;
use super::stream_health::AudioStreamHealth;

/// Silence written after a reopen lands on a new rate, before any voice plays.
///
/// A DAC mutes while its clock relocks, which would clip the head of the track that asked for the
/// rate. A guess until exclusive output meets real hardware; too short clips, too long is a gap
/// at every rate boundary.
const RESYNC_HOLD: Duration = Duration::from_millis(200);

/// What an open asks the device for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputRequest {
    /// The rate to try first, or `None` for the device's own config.
    pub rate: Option<SampleRate>,
}

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
    /// What the open stream was asked for, which is what a device-loss reopen asks for again.
    request: OutputRequest,
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
        let request = OutputRequest::default();
        let stream = device::open(&target, &feed, request.rate)?;
        Ok(Self { stream: Some(stream), request, feed, mixer })
    }

    /// Replace the stream with one on whatever the default device is now, asking it for `request`.
    ///
    /// A request that cannot be opened at all falls back to the one the last stream was opened
    /// with, so asking for something new never costs the audio that was working. A failure of
    /// both leaves the output with no stream rather than the dead one, so the next attempt starts
    /// clean.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no device to open, or it refuses every config. Where the
    /// fallback also failed, the error is the request's own.
    pub fn reopen(&mut self, request: OutputRequest) -> Result<Negotiated, AppError> {
        let previous_rate = self.negotiated().map(|negotiated| negotiated.shape.rate);
        self.stream = None;
        // After the drop and before the open, which is the one window where no stream can report:
        // a loss still flagged here belongs to the stream just dropped, and left standing it would
        // reopen the new one for nothing.
        self.feed.health.take_device_lost();

        let negotiated = match self.start(request) {
            Ok(negotiated) => {
                self.request = request;
                negotiated
            }
            Err(e) if request != self.request => {
                log::warn!(
                    "Output refused {request:?}, reopening {:?}: {}",
                    self.request,
                    error::describe(&e)
                );
                self.start(self.request).map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };

        let rate = negotiated.shape.rate;
        if previous_rate != Some(rate) {
            self.feed.pull.lock().hold(resync_frames(rate));
        }
        // The stall watch counts from the last beat, and the old stream's was before the open.
        self.feed.health.beat();
        Ok(negotiated)
    }

    fn start(&mut self, request: OutputRequest) -> Result<Negotiated, AppError> {
        let target = device::default_target()?;
        let stream = device::open(&target, &self.feed, request.rate)?;
        let negotiated = stream.negotiated();
        self.stream = Some(stream);
        Ok(negotiated)
    }

    /// What the open stream was asked for, as opposed to what it negotiated.
    pub fn request(&self) -> OutputRequest {
        self.request
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

/// [`RESYNC_HOLD`] in device frames at `rate`.
fn resync_frames(rate: SampleRate) -> usize {
    let frames = u128::from(rate.get()) * RESYNC_HOLD.as_millis() / 1_000;
    usize::try_from(frames).unwrap_or(usize::MAX)
}
