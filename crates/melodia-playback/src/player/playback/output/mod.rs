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

// One exclusive backend per platform, each answering to the same names: `SUPPORTED`, `POLLING`,
// `HARDWARE_VOLUME`, `devices`, `open`, the `ExclusiveStream` it returns and the `Claim` a reopen
// carries across.
cfg_select! {
    target_os = "linux" => {
        mod alsa;
        mod realtime;
        mod reserve;
        use self::alsa as exclusive;
    }
    target_os = "windows" => {
        mod endpoint_volume;
        mod mmcss;
        mod wasapi;
        use self::wasapi as exclusive;
    }
    _ => {
        mod unsupported;
        use self::unsupported as exclusive;
    }
}

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use melodia_audio::player::source::audio::{SampleRate, Shape, SourceFormat, frames_in};
use melodia_core::error::{self, AppError};
use melodia_core::utils::toast::{self, ToastKind};

use self::claim::{ClaimError, Fallback, FallbackReason};
use self::device::{DeviceStream, ExternalVolume, Feed, Lead};
use self::encode::DeviceFormat;
use self::exclusive::{Claim, ExclusiveStream};
use self::mixer::Mixer;
use super::stream_health::AudioStreamHealth;

/// Silence written after a reopen lands on a new rate, before any voice plays, until the user sets
/// their own.
///
/// A DAC that mutes while its clock relocks would otherwise clip the head of the track that asked
/// for the rate. Not every one does, so this is sized for the ones that do: the silence falls only
/// at a rate boundary, which is never gapless anyway, while too little clips on the DACs it exists
/// for.
pub const DEFAULT_RESYNC_HOLD: Duration = Duration::from_millis(200);

/// The longest hold a user may set. The slowest DACs relock well inside it, and past it the
/// silence reads as a stall rather than a gap.
pub const MAX_RESYNC_HOLD: Duration = Duration::from_secs(1);

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
    Shared {
        rate: Option<SampleRate>,
    },
    Exclusive(ExclusiveRequest),
}

/// A claim on one device, opened at exactly the source's shape and a format that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExclusiveRequest {
    /// The device's id, or `None` for the first the system lists.
    pub device: Option<String>,
    pub shape: Shape,
    pub format: SourceFormat,
    pub tuning: ExclusiveTuning,
    /// Carry the volume on the device's own control where it has one in hardware, leaving the
    /// samples untouched. Part of the request so a change reopens the claim: the gain moving
    /// between the voices and the device mid-stream can play a period at the wrong level.
    pub hardware_volume: bool,
}

/// How an exclusive writer paces the device. Part of the request, so changing it reopens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExclusiveTuning {
    /// What the writer hands the device at a time. A device that can't run it gets the nearest
    /// period it can, which [`Negotiated::period`] reports.
    pub period: Duration,
    pub drive: Drive,
}

impl ExclusiveTuning {
    /// Short enough that a stop lands quickly, long enough that the writer wakes rarely.
    pub const DEFAULT_PERIOD: Duration = Duration::from_millis(20);
    /// Below a couple of milliseconds no device keeps up, and every one rounds it up anyway.
    pub const MIN_PERIOD: Duration = Duration::from_millis(2);
    /// A stop waits out a period, and a voice's command waits for a fill, so a longer one would
    /// start to read as a hang.
    pub const MAX_PERIOD: Duration = Duration::from_millis(100);

    /// `period` held inside the range a claim asks for.
    pub fn new(period: Duration, drive: Drive) -> Self {
        Self { period: period.clamp(Self::MIN_PERIOD, Self::MAX_PERIOD), drive }
    }
}

impl Default for ExclusiveTuning {
    fn default() -> Self {
        Self { period: Self::DEFAULT_PERIOD, drive: Drive::default() }
    }
}

/// What wakes the exclusive writer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Drive {
    /// The device signals each time it wants a period, and gets exactly one.
    #[default]
    Events,
    /// The writer wakes on a timer and tops the device's buffer up. Some USB drivers stutter in
    /// event mode and play cleanly in this one. Only WASAPI tells the two apart; ALSA's writer
    /// always blocks on the card.
    Polling,
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

/// Whether this platform has an exclusive backend. Where it doesn't, every claim falls back.
pub const EXCLUSIVE_SUPPORTED: bool = exclusive::SUPPORTED;

/// Whether the exclusive backend tells [`Drive::Polling`] from [`Drive::Events`].
pub const POLLING_SUPPORTED: bool = exclusive::POLLING;

/// Whether an exclusive claim can carry the volume on the device's own control.
pub const HARDWARE_VOLUME_SUPPORTED: bool = exclusive::HARDWARE_VOLUME;

/// Every device an exclusive claim can be aimed at, empty where there is no exclusive backend.
/// Blocking: it asks every device.
pub fn devices() -> Vec<OutputDevice> {
    exclusive::devices()
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
    pub fallback: Option<Fallback>,
    /// Whether the device's own control carries the volume, so the voices hand it the samples at
    /// unity. Only an exclusive claim that asked for it, on a device with one in hardware.
    pub hardware_volume: bool,
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
    /// The refusal last reported, so a claim retried at every track start and refused the same way
    /// is reported once rather than per track. A claim that takes in between leaves it standing, or
    /// a mixed queue on a 16-bit card warns at every lossy track. Going shared clears it, as do a
    /// new output choice and a claim taking after a disconnect: each is news, where a format
    /// refused again is not.
    reported: Option<Fallback>,
    /// The silence a reopen onto a new rate writes first. Not part of the request: changing it
    /// reopens nothing, and only the next rate change hears it.
    resync_hold: Duration,
    feed: Feed,
    mixer: Mixer,
}

/// The stream on one backend or the other. Dropping it stops audio and releases the device.
enum Stream {
    Shared(DeviceStream),
    Exclusive(ExclusiveStream),
}

impl Stream {
    /// Close the stream, keeping an exclusive one's claim on its card.
    fn into_claim(self) -> Option<Claim> {
        match self {
            Self::Shared(_) => None,
            Self::Exclusive(stream) => stream.into_claim(),
        }
    }

    fn negotiated(&self) -> Negotiated {
        match self {
            Self::Shared(stream) => stream.negotiated(),
            Self::Exclusive(stream) => stream.negotiated(),
        }
    }

    /// [`Negotiated::hardware_volume`], asked without cloning the rest: the volume slider asks it
    /// on every move.
    fn hardware_volume(&self) -> bool {
        match self {
            Self::Shared(_) => false,
            Self::Exclusive(stream) => stream.hardware_volume(),
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
            reported: None,
            resync_hold: DEFAULT_RESYNC_HOLD,
            feed,
            mixer,
        })
    }

    /// Write `hold` of silence after a reopen onto a new rate, held under [`MAX_RESYNC_HOLD`].
    pub fn set_resync_hold(&mut self, hold: Duration) {
        self.resync_hold = hold.min(MAX_RESYNC_HOLD);
    }

    /// Take the user's volume, as an amplitude, for a claim carrying it on the device's own
    /// control. The voices apply [`Self::voice_gain`] of it instead.
    pub fn set_volume(&self, volume: f64) {
        self.feed.volume.store(volume);
    }

    /// The user's volume, as an amplitude.
    pub fn volume(&self) -> f64 {
        self.feed.volume.load()
    }

    /// The gain the voices apply for `volume` on the stream open now.
    pub fn voice_gain(&self, volume: f64) -> f64 {
        let on_device = self.stream.as_ref().is_some_and(Stream::hardware_volume);
        voice_gain(volume, on_device)
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
        let held = self.close_for(&request);
        // After the drop and before the open, which is the one window where no stream can report:
        // a loss still flagged here belongs to the stream just dropped, and left standing it would
        // reopen the new one for nothing. Its lead is that stream's too.
        self.feed.health.take_device_lost();
        self.feed.lead.clear();

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
            let hold = usize::try_from(frames_in(self.resync_hold, rate)).unwrap_or(usize::MAX);
            self.feed.pull.lock().hold(hold);
        }
        // The stall watch counts from the last beat, and the old stream's was before the open.
        self.feed.health.beat();
        Ok(negotiated)
    }

    /// Close the stream, keeping an exclusive one's claim only where `request` could reuse it. A
    /// shared request closes it whole, which hands the card back before the shared stream opens.
    fn close_for(&mut self, request: &OutputRequest) -> Option<Claim> {
        let stream = self.stream.take()?;
        match request {
            OutputRequest::Exclusive(_) => stream.into_claim(),
            OutputRequest::Shared { .. } => None,
        }
    }

    /// Open a stream for `request`, reusing `held` where it claims the card asked for.
    fn start(
        &mut self,
        request: &OutputRequest,
        held: Option<Claim>,
    ) -> Result<Negotiated, AppError> {
        let stream = match request {
            OutputRequest::Shared { rate } => {
                self.reported = None;
                Stream::Shared(self.open_shared(*rate)?)
            }
            OutputRequest::Exclusive(exclusive) => match self.claim(exclusive, held) {
                Ok(stream) => {
                    self.reported
                        .take_if(|fallback| fallback.reason == FallbackReason::NotConnected);
                    stream
                }
                Err(e) => Stream::Shared(self.fall_back(exclusive, &e)?),
            },
        };
        let negotiated = stream.negotiated();
        self.stream = Some(stream);
        Ok(negotiated)
    }

    /// Open the system's default device shared, logging which device that was where it won't
    /// open: the error alone can't say, and the default is whatever the system named at the time.
    /// At debug, since a caller retrying a lost device reports the outcome once it gives up.
    fn open_shared(&self, rate: Option<SampleRate>) -> Result<DeviceStream, AppError> {
        let target = device::default_target().inspect_err(|e| {
            log::debug!("audio: no default output to open: {}", error::describe(e));
        })?;
        device::open(&target, &self.feed, rate).inspect_err(|e| {
            log::debug!(
                "audio: the default output, {}, would not open: {}",
                target.name().as_deref().unwrap_or("unnamed"),
                error::describe(e)
            );
        })
    }

    fn claim(&self, request: &ExclusiveRequest, held: Option<Claim>) -> Result<Stream, ClaimError> {
        exclusive::open(request, &self.feed, held).map(Stream::Exclusive)
    }

    /// Open shared in place of a refused claim, carrying why and which device refused.
    ///
    /// The refusing device is looked up rather than carried on the error, so no backend has to
    /// thread its name through every way a claim can fail.
    ///
    /// **A refusal is a warning and a toast once, not per track.** A fallback is retried at every
    /// track start, and a 16-bit card refuses every lossy file, so the same answer again goes to
    /// debug and says nothing to the user.
    fn fall_back(
        &mut self,
        request: &ExclusiveRequest,
        refusal: &ClaimError,
    ) -> Result<DeviceStream, AppError> {
        let fallback = Fallback {
            reason: refusal.reason(),
            device: refusing_device(request.device.as_deref()),
        };
        let first_report = self.reported.as_ref() != Some(&fallback);
        let level = if first_report { log::Level::Warn } else { log::Level::Debug };
        if first_report {
            toast::notify(ToastKind::ExclusiveRefused, fallback.device.clone().unwrap_or_default());
        }
        // No name can also mean no backend or a failed listing, so it isn't read as a disconnect.
        match &fallback.device {
            Some(device) => log::log!(
                level,
                "audio: exclusive output refused by {device}, playing shared: {}",
                error::describe(refusal)
            ),
            None => log::log!(
                level,
                "audio: exclusive output refused, playing shared: {}",
                error::describe(refusal)
            ),
        }
        self.reported = Some(fallback.clone());
        let mut stream = self.open_shared(Some(request.shape.rate))?;
        stream.negotiated.fallback = Some(fallback);
        Ok(stream)
    }

    /// Report the next refusal whatever was reported before, for an output choice the user just
    /// made: a device they picked again is asked again.
    pub fn forget_refusal(&mut self) {
        self.reported = None;
    }

    /// Close the stream until the next [`Self::reopen`], giving the device back to everything
    /// else on the system. Nothing plays meanwhile; the voices keep what they hold.
    pub fn park(&mut self) {
        self.stream = None;
        self.parked = true;
        self.feed.lead.clear();
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

    /// How far the ear is behind the voices' clocks on whichever stream is open. The same cell for
    /// the output's life, so a holder reads each new stream's measurement without asking again.
    pub fn lead(&self) -> Arc<Lead> {
        Arc::clone(&self.feed.lead)
    }

    /// Where a claim carrying the volume on the device's own control reports it was moved from
    /// outside Melodia. The same cell for the output's life, like [`Self::lead`].
    pub fn external_volume(&self) -> Arc<ExternalVolume> {
        Arc::clone(&self.feed.external_volume)
    }

    /// What the device agreed to, as opposed to what it was asked for, or `None` while no stream
    /// is open.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.stream.as_ref().map(Stream::negotiated)
    }
}

/// The voices' gain for `volume`: unity while the device's own control carries it, except at zero,
/// which stays a silence in the voices so a mute is instant and needs no switch on the device.
fn voice_gain(volume: f64, on_device: bool) -> f64 {
    if on_device && volume > 0.0 { 1.0 } else { volume }
}

/// The name of the device a claim on `id` was aimed at, or of the first listed where none was
/// chosen, which is the one a claim with no id takes on every backend.
fn refusing_device(id: Option<&str>) -> Option<String> {
    let mut devices = exclusive::devices().into_iter();
    let device = match id {
        Some(id) => devices.find(|device| device.id == id),
        None => devices.next(),
    };
    device.map(|device| device.name)
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
