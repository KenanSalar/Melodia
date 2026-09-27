//! Everything below the DSP chain: the device stream, the voices, the sum, and the clock.
//!
//! The chain above is untouched by any of it — the ordering, the ramp, the tap and the limiter read
//! the same samples either way. What this tree owns is the four things underneath, and each argues
//! itself where it lives: [`convert`] for the rate, the channel map and the speed ratio, [`voice`]
//! for pause, volume and the clock, [`mixer`] for the unclamped sum, [`device`] for the shared
//! stream and the ladder that opens it. An exclusive claim is its own backend per platform, fed
//! through [`encode`], and falls back to [`device`] whenever it is refused.
//!
//! [`AudioOutput`] is the whole public surface: build it, hand [`Mixer`] to the decks, reopen it
//! when the device goes away. `crates/melodia/tests/crossfade.rs` skips it and drives
//! [`mixer::pair`] directly, which is what lets the full chain be tested with no sound card.

pub mod claim;
pub mod convert;
pub mod device;
pub mod encode;
pub mod mixer;
pub mod voice;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
use self::alsa::Claim;
#[cfg(target_os = "linux")]
mod realtime;
#[cfg(target_os = "linux")]
mod reserve;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use melodia_audio::player::source::audio::{SampleRate, Shape, SourceFormat};
use melodia_core::error::{self, AppError};

use self::claim::{ClaimError, FallbackReason};
use self::device::{DeviceStream, Feed};
use self::encode::DeviceFormat;
use self::mixer::Mixer;
use super::stream_health::AudioStreamHealth;

/// Silence written after a reopen lands on a new rate, before any voice plays.
///
/// A DAC that mutes while its clock relocks would otherwise clip the head of the track that asked
/// for the rate. Not every one does, so this is sized for the ones that do: the silence falls only
/// at a rate boundary, which is never gapless anyway, while too little clips on the DACs it exists
/// for.
const RESYNC_HOLD: Duration = Duration::from_millis(200);

/// Whether the output goes through the system mixer or takes the device for itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    Shared,
    Exclusive,
}

/// What an open asks the device for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRequest {
    /// The system's default output, at `rate` where one is given and the device's own config
    /// otherwise.
    Shared { rate: Option<SampleRate> },
    /// The device named by `device`, or the first the system lists, opened at exactly the
    /// source's shape and a format that holds it.
    Exclusive { device: Option<String>, shape: Shape, format: SourceFormat },
}

impl Default for OutputRequest {
    fn default() -> Self {
        Self::Shared { rate: None }
    }
}

/// A device an exclusive claim can be aimed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDevice {
    /// Stable across reboots and replugs, which is what makes it the persisted choice.
    pub id: String,
    pub name: String,
}

/// Every device an exclusive claim can be aimed at, empty where there is no exclusive backend.
pub fn devices() -> Vec<OutputDevice> {
    cfg_select! {
        target_os = "linux" => alsa::devices(),
        _ => Vec::new(),
    }
}

/// The sample format a stream was opened with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// cpal's, on a shared stream. The system mixer sits past it, so it proves nothing about what
    /// the card receives.
    Shared(cpal::SampleFormat),
    Exclusive(DeviceFormat),
}

impl OutputFormat {
    /// Whether a source in `source` reaches the card unchanged through this format.
    pub fn carries(self, source: SourceFormat) -> bool {
        match self {
            Self::Shared(_) => false,
            Self::Exclusive(format) => format.carries(source),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared(format) => format.fmt(f),
            Self::Exclusive(format) => format.fmt(f),
        }
    }
}

/// What the device actually agreed to, beside what it was asked for.
///
/// Reported rather than assumed because every part of it can differ from the request, and because a
/// bit-perfect mode is only checkable if the negotiated end of it is visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    /// What the host calls the device, or `None` where it would not say. After a reopen this is
    /// the only thing telling a bug report which output the audio moved to.
    pub device_name: Option<String>,
    pub shape: Shape,
    pub format: OutputFormat,
    /// Why an exclusive claim fell back to this shared stream, or `None` where none was refused.
    pub fallback: Option<FallbackReason>,
    /// The period that was asked for, or `None` where the host was left to name its own.
    ///
    /// Kept beside the answer because it is the one of the two that says which pass of the ladder
    /// won, which is the difference between a block this tree sized and one nobody did.
    pub requested_period: Option<cpal::FrameCount>,
    /// What the host says it will hand the callback at a time, or `None` where it cannot say.
    ///
    /// Asked rather than inferred: `StreamTrait::buffer_size` arrived in cpal 0.18, and before it
    /// the only place the real block appeared was `data.len()` inside the callback. cpal calls it
    /// advisory and the hosts that don't track one answer `UnsupportedOperation`, so this is where
    /// a bug report reads the block back, not a bound anything sizes against.
    pub period: Option<cpal::FrameCount>,
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
    /// `None` once a reopen has failed, until one succeeds, and while parked.
    stream: Option<Stream>,
    /// What the open stream was asked for, which is what a device-loss reopen asks for again.
    request: OutputRequest,
    /// Closed on purpose rather than lost, so nothing should read the missing stream as a fault.
    parked: bool,
    feed: Feed,
    mixer: Mixer,
}

/// The stream on one backend or the other. Dropping it stops audio and releases the device.
enum Stream {
    Shared(DeviceStream),
    #[cfg(target_os = "linux")]
    Exclusive(alsa::AlsaStream),
}

/// Where there is no exclusive backend there is never a claim to carry across a reopen.
#[cfg(not(target_os = "linux"))]
enum Claim {}

impl Stream {
    /// Close the stream, keeping an exclusive one's claim on its card.
    fn into_claim(self) -> Option<Claim> {
        match self {
            Self::Shared(_) => None,
            #[cfg(target_os = "linux")]
            Self::Exclusive(stream) => stream.into_claim(),
        }
    }

    fn negotiated(&self) -> Negotiated {
        match self {
            Self::Shared(stream) => stream.negotiated(),
            #[cfg(target_os = "linux")]
            Self::Exclusive(stream) => stream.negotiated(),
        }
    }
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
        let stream = device::open(&target, &feed, None)?;
        Ok(Self {
            stream: Some(Stream::Shared(stream)),
            request: OutputRequest::default(),
            parked: false,
            feed,
            mixer,
        })
    }

    /// Replace the stream with one opened for `request`: an exclusive claim, or the default device.
    ///
    /// An exclusive claim that is refused opens shared instead and says why on the answer, so the
    /// mode never costs the audio. A request that cannot be opened at all falls back to the one
    /// the last stream was opened with. A failure of both leaves the output with no stream rather
    /// than the dead one, so the next attempt starts clean.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no device to open, or it refuses every config. Where the
    /// fallback also failed, the error is the request's own.
    pub fn reopen(&mut self, request: OutputRequest) -> Result<Negotiated, AppError> {
        let previous_rate = self.negotiated().map(|negotiated| negotiated.shape.rate);
        let held = self.stream.take().and_then(Stream::into_claim);
        // After the drop and before the open, which is the one window where no stream can report:
        // a loss still flagged here belongs to the stream just dropped, and left standing it would
        // reopen the new one for nothing.
        self.feed.health.take_device_lost();

        let negotiated = match self.start(&request, held) {
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
                let previous = self.request.clone();
                self.start(&previous, None).map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };
        self.parked = false;

        let rate = negotiated.shape.rate;
        if previous_rate != Some(rate) {
            self.feed.pull.lock().hold(resync_frames(rate));
        }
        // The stall watch counts from the last beat, and the old stream's was before the open.
        self.feed.health.beat();
        Ok(negotiated)
    }

    /// Open a stream for `request`, reusing `held` where it claims the card asked for. A shared
    /// request drops it, which is what hands the card back.
    fn start(
        &mut self,
        request: &OutputRequest,
        held: Option<Claim>,
    ) -> Result<Negotiated, AppError> {
        let stream = match request {
            OutputRequest::Shared { rate } => {
                drop(held);
                Stream::Shared(self.open_shared(*rate)?)
            }
            OutputRequest::Exclusive { device, shape, format } => {
                match self.claim(device.as_deref(), *shape, *format, held) {
                    Ok(stream) => stream,
                    Err(e) => {
                        log::warn!(
                            "Exclusive output refused, playing shared: {}",
                            error::describe(&e)
                        );
                        let mut stream = self.open_shared(Some(shape.rate))?;
                        stream.negotiated.fallback = Some(e.reason());
                        Stream::Shared(stream)
                    }
                }
            }
        };
        let negotiated = stream.negotiated();
        self.stream = Some(stream);
        Ok(negotiated)
    }

    fn open_shared(&self, rate: Option<SampleRate>) -> Result<DeviceStream, AppError> {
        device::open(&device::default_target()?, &self.feed, rate)
    }

    #[cfg(target_os = "linux")]
    fn claim(
        &self,
        device: Option<&str>,
        shape: Shape,
        format: SourceFormat,
        held: Option<Claim>,
    ) -> Result<Stream, ClaimError> {
        alsa::open(device, shape, format, &self.feed, held).map(Stream::Exclusive)
    }

    #[cfg(not(target_os = "linux"))]
    fn claim(
        &self,
        _: Option<&str>,
        _: Shape,
        _: SourceFormat,
        _: Option<Claim>,
    ) -> Result<Stream, ClaimError> {
        Err(ClaimError::Unsupported)
    }

    /// Close the stream until the next [`Self::reopen`], giving the device back to everything
    /// else on the system. Nothing plays meanwhile; the voices keep what they hold.
    pub fn park(&mut self) {
        self.stream = None;
        self.parked = true;
    }

    /// Whether the missing stream was closed by [`Self::park`] rather than lost.
    pub fn is_parked(&self) -> bool {
        self.parked
    }

    /// What the open stream was asked for, as opposed to what it negotiated.
    pub fn request(&self) -> &OutputRequest {
        &self.request
    }

    /// The voices, as one mixer. Handed to `player::playback::decks` at boot and not reachable any other way.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// What the device agreed to, as opposed to what it was asked for, or `None` while no stream
    /// is open.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.stream.as_ref().map(Stream::negotiated)
    }
}

/// [`RESYNC_HOLD`] in device frames at `rate`.
fn resync_frames(rate: SampleRate) -> usize {
    let frames = u128::from(rate.get()) * RESYNC_HOLD.as_millis() / 1_000;
    usize::try_from(frames).unwrap_or(usize::MAX)
}
