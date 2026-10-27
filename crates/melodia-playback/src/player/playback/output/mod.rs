//! Everything below the DSP chain, each part arguing itself: [`convert`] the rate, channel map
//! and speed, [`voice`] pause, volume and the clock, [`mixer`] the unclamped sum, [`device`] the
//! shared stream and its ladder. An exclusive claim is a backend per platform, fed through
//! [`dither`] and [`encode`], falling back to [`device`] when refused.
//!
//! [`AudioOutput`] is the public surface; `crates/melodia/tests/crossfade.rs` drives
//! [`mixer::pair`] directly, testing the full chain with no sound card.

pub mod claim;
pub mod convert;
pub mod device;
pub mod dither;
pub mod encode;
pub mod mixer;
mod negotiated;
mod rates;
mod request;
mod resample;
pub mod voice;

// One exclusive backend per platform, each answering to the same names: `SUPPORTED`, `POLLING`,
// `HARDWARE_VOLUME`, `RATE_FALLBACK`, `SHARED_DEVICE`, `devices`, `open`, the `ExclusiveStream` it
// returns and the `Claim` a reopen carries across.
cfg_select! {
    target_os = "linux" => {
        mod alsa;
        mod alsa_volume;
        mod hardware_volume;
        mod realtime;
        mod reserve;
        use self::alsa as exclusive;
    }
    target_os = "windows" => {
        mod endpoint_volume;
        mod hardware_volume;
        mod mmcss;
        mod wasapi;
        mod wasapi_clock;
        mod wasapi_formats;
        mod wasapi_offered;
        use self::wasapi as exclusive;
    }
    _ => {
        mod unsupported;
        use self::unsupported as exclusive;
    }
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use melodia_audio::player::source::audio::SampleRate;
use melodia_core::error::{self, AppError};
use melodia_core::utils::toast::{self, ToastKind};

pub use self::negotiated::{DeviceLevel, Negotiated, OutputFormat};
pub use self::request::{
    Drive, ExclusiveRequest, ExclusiveTuning, OutputDevice, OutputMode, OutputRequest, RateFallback,
};

use self::claim::{ClaimError, Fallback, FallbackReason, claim_serves};
use self::device::{DeviceStream, ExternalVolume, Feed, Lead, Target};
use self::exclusive::{Claim, ExclusiveStream};
use self::mixer::Mixer;
use super::stream_health::AudioStreamHealth;

/// The silence after a reopen onto a new rate, until the user sets their own: a DAC muting while
/// its clock relocks would clip the track's head. Sized for those, since it falls only at a rate
/// boundary, which is never gapless anyway.
pub const DEFAULT_RESYNC_HOLD: Duration = Duration::from_millis(200);

/// The longest hold a user may set. The slowest DACs relock well inside it, and past it the
/// silence reads as a stall rather than a gap.
pub const MAX_RESYNC_HOLD: Duration = Duration::from_secs(1);

/// Whether this platform has an exclusive backend. Where it doesn't, every claim falls back.
pub const EXCLUSIVE_SUPPORTED: bool = exclusive::SUPPORTED;

/// Whether the exclusive backend tells [`Drive::Polling`] from [`Drive::Events`].
pub const POLLING_SUPPORTED: bool = exclusive::POLLING;

/// Whether an exclusive claim can carry the volume on the device's own control.
pub const HARDWARE_VOLUME_SUPPORTED: bool = exclusive::HARDWARE_VOLUME;

/// Whether an exclusive claim can run the device at another rate where it lacks the source's.
pub const RATE_FALLBACK_SUPPORTED: bool = exclusive::RATE_FALLBACK;

/// Whether a shared stream can be aimed at one of [`devices`] rather than the system default.
pub const SHARED_DEVICE_SUPPORTED: bool = exclusive::SHARED_DEVICE;

/// Whether opening a shared stream at the file's rate can change what the device runs at. Windows
/// shared mode converts every stream to the mix format, so there it would reopen for nothing.
pub const FOLLOW_RATE_SUPPORTED: bool = !cfg!(target_os = "windows");

/// Every device an exclusive claim can be aimed at, and where [`SHARED_DEVICE_SUPPORTED`] a shared
/// stream too, empty where there is no exclusive backend. Blocking: it asks every device.
pub fn devices() -> Vec<OutputDevice> {
    exclusive::devices()
}

/// The open device and the voices feeding it. A reopen replaces the stream and nothing else: the
/// puller lives here, so the decks, their ramps and their clocks carry on into the next stream. The
/// stream is the first field and drops first, stopping the callback, against which a control op
/// would wait out its whole timeout.
pub struct AudioOutput {
    /// `None` once a reopen has failed, until one succeeds, and while parked.
    stream: Option<Stream>,
    /// What a device-loss reopen asks for again.
    request: OutputRequest,
    status: Arc<OutputStatus>,
    /// So the same refusal at a later track start is reported once; a claim taking in between
    /// leaves it, or a mixed queue on a 16-bit card warns per 24-bit track. Going shared, a new
    /// output choice and a claim taking after a disconnect clear it, each being news.
    reported: Option<Fallback>,
    /// The silence a reopen onto a new rate writes first. Not part of the request: changing it
    /// reopens nothing, and only the next rate change hears it.
    resync_hold: Duration,
    /// Held for the output's life so ALSA's config is parsed once, not at every reopen; see
    /// [`device::default_target`].
    host: cpal::Host,
    feed: Feed,
    mixer: Mixer,
}

/// What the periodic checks ask of the output, readable without its lock, which a reopen holds for
/// as long as a device takes to open. The same cell for the output's life, like [`Lead`].
#[derive(Debug, Default)]
pub struct OutputStatus {
    parked: AtomicBool,
    awaiting_device: AtomicBool,
    reopening: AtomicBool,
}

impl OutputStatus {
    /// Closed on purpose, so nothing reads the missing stream as a fault.
    pub fn parked(&self) -> bool {
        self.parked.load(Ordering::Relaxed)
    }

    /// Standing in for a chosen device that wasn't connected, so it is worth looking for again.
    pub fn awaiting_device(&self) -> bool {
        self.awaiting_device.load(Ordering::Relaxed)
    }

    /// No stream beats meanwhile and everything else waits behind the reopen's locks, so a poll
    /// skips the round.
    pub fn reopening(&self) -> bool {
        self.reopening.load(Ordering::Relaxed)
    }
}

/// Whether a shared stream's named device was there, which is what says to wait for it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Named {
    /// Opened, or none was named, or it was there and refused: nothing to wait for.
    Present,
    /// Not connected, with the system default standing in until it comes back.
    Missing,
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
    /// Opens the default device and builds `voices` many against whatever it negotiated, reporting
    /// stream faults into `health`.
    pub fn open(voices: usize, health: Arc<AudioStreamHealth>) -> Result<Self, AppError> {
        let (mut output, target) = Self::unopened(voices, health)?;
        output.stream = Some(Stream::Shared(device::open(&target, &output.feed, None)?));
        Ok(output)
    }

    /// Parked until the first [`Self::reopen`], so a start under exclusive output doesn't open the
    /// device shared only to close it again.
    pub fn open_parked(voices: usize, health: Arc<AudioStreamHealth>) -> Result<Self, AppError> {
        let (mut output, _) = Self::unopened(voices, health)?;
        output.park();
        Ok(output)
    }

    fn unopened(voices: usize, health: Arc<AudioStreamHealth>) -> Result<(Self, Target), AppError> {
        let host = cpal::default_host();
        let target = device::default_target(&host)?;
        let (mixer, pull) = mixer::pair(voices, target.default_shape()?);
        let output = Self {
            stream: None,
            request: OutputRequest::default(),
            status: Arc::default(),
            reported: None,
            resync_hold: DEFAULT_RESYNC_HOLD,
            host,
            feed: Feed::new(pull, health),
            mixer,
        };
        Ok((output, target))
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

    pub fn volume(&self) -> f64 {
        self.feed.volume.load()
    }

    /// The gain the voices apply for `volume` on the stream open now.
    pub fn voice_gain(&self, volume: f64) -> f64 {
        let on_device = self.stream.as_ref().is_some_and(Stream::hardware_volume);
        voice_gain(volume, on_device)
    }

    /// A refused claim opens shared and says why, so the mode never costs the audio. A request that
    /// can't open at all falls back to the last one's, and both failing leaves no stream rather
    /// than the dead one, and returns the request's own error.
    pub fn reopen(&mut self, request: OutputRequest) -> Result<Negotiated, AppError> {
        self.status.reopening.store(true, Ordering::Relaxed);
        let reopened = self.replace_stream(request);
        self.status.reopening.store(false, Ordering::Relaxed);
        reopened
    }

    fn replace_stream(&mut self, request: OutputRequest) -> Result<Negotiated, AppError> {
        let previous_rate = self.negotiated().map(|negotiated| negotiated.shape.rate);
        let held = self.close_for(&request);
        // The one window no stream reports in: a loss still flagged, and the lead, are the dropped
        // stream's, and left standing would reopen the new one for nothing.
        self.feed.health.take_device_lost();
        self.feed.lead.clear();

        // Armed rather than applied once the stream is up: a stream pulls as soon as it starts,
        // so the silence has to be in place before then, which is when each open reshapes.
        self.feed.pull.lock().arm_resync(previous_rate, self.resync_hold);
        let opened = match self.start(&request, held) {
            Ok(negotiated) => {
                self.request = request;
                Ok(negotiated)
            }
            Err(e) if request != self.request => {
                log::warn!(
                    "Output refused {request:?}, reopening {:?}: {}",
                    self.request,
                    error::describe(&e)
                );
                let previous = self.request.clone();
                self.start(&previous, None).map_err(|_| e)
            }
            Err(e) => Err(e),
        };
        self.feed.pull.lock().disarm_resync();
        let negotiated = opened?;
        self.status.parked.store(false, Ordering::Relaxed);

        // The stall watch counts from the last beat, and the old stream's was before the open.
        self.feed.health.beat();
        Ok(negotiated)
    }

    /// Close the stream, keeping an exclusive one's claim only where `request` could reuse it. A
    /// shared request closes it whole, which hands the card back before the shared stream opens.
    fn close_for(&mut self, request: &OutputRequest) -> Option<Claim> {
        self.status.awaiting_device.store(false, Ordering::Relaxed);
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
        let (stream, named) = match request {
            OutputRequest::Shared { rate, device } => {
                self.reported = None;
                let (stream, named) = self.open_shared(*rate, device.as_deref())?;
                (Stream::Shared(stream), named)
            }
            OutputRequest::Exclusive(exclusive) => match self.claim(exclusive, held) {
                Ok(stream) => {
                    self.reported
                        .take_if(|fallback| fallback.reason == FallbackReason::NotConnected);
                    (stream, Named::Present)
                }
                Err(e) => {
                    let (stream, named) = self.fall_back(exclusive, &e)?;
                    (Stream::Shared(stream), named)
                }
            },
        };
        let negotiated = stream.negotiated();
        let refused_as_missing = negotiated
            .fallback
            .as_ref()
            .is_some_and(|fallback| fallback.reason == FallbackReason::NotConnected);
        let awaiting_device = named == Named::Missing || refused_as_missing;
        self.status.awaiting_device.store(awaiting_device, Ordering::Relaxed);
        self.stream = Some(stream);
        Ok(negotiated)
    }

    /// Open `device` shared, or the system default where none is named, where it isn't connected,
    /// or where it won't open: a device that refuses costs the route, never the audio.
    fn open_shared(
        &self,
        rate: Option<SampleRate>,
        device: Option<&str>,
    ) -> Result<(DeviceStream, Named), AppError> {
        let Some(id) = device else {
            return Ok((self.open_default(rate)?, Named::Present));
        };
        match device::named_target(&self.host, id) {
            Ok(Some(target)) => {
                if let Ok(stream) = self.open_target(&target, rate) {
                    return Ok((stream, Named::Present));
                }
            }
            Ok(None) => {
                log::info!(
                    "audio: the chosen output isn't connected, playing on the system default"
                );
                return Ok((self.open_default(rate)?, Named::Missing));
            }
            Err(e) => log::debug!("audio: the chosen output won't open: {}", error::describe(&e)),
        }
        Ok((self.open_default(rate)?, Named::Present))
    }

    fn open_default(&self, rate: Option<SampleRate>) -> Result<DeviceStream, AppError> {
        let target = device::default_target(&self.host).inspect_err(|e| {
            log::debug!("audio: no default output to open: {}", error::describe(e));
        })?;
        self.open_target(&target, rate)
    }

    /// Logs which device would not open, which the error can't say; at debug, a caller retrying a
    /// lost device reporting the outcome once it gives up.
    fn open_target(
        &self,
        target: &Target,
        rate: Option<SampleRate>,
    ) -> Result<DeviceStream, AppError> {
        device::open(target, &self.feed, rate).inspect_err(|e| {
            log::debug!(
                "audio: the output {} would not open: {}",
                target.name().as_deref().unwrap_or("unnamed"),
                error::describe(e)
            );
        })
    }

    fn claim(&self, request: &ExclusiveRequest, held: Option<Claim>) -> Result<Stream, ClaimError> {
        exclusive::open(request, &self.feed, held).map(Stream::Exclusive)
    }

    /// Opens shared in place of a refused claim, the refusing device looked up rather than threaded
    /// through every backend's errors. A warning and a toast once: the same refusal at a later track
    /// start goes to debug.
    fn fall_back(
        &mut self,
        request: &ExclusiveRequest,
        refusal: &ClaimError,
    ) -> Result<(DeviceStream, Named), AppError> {
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
        // On the device the claim was for, where a shared stream can be aimed at it and it is there.
        let device = request
            .device
            .as_deref()
            .filter(|_| SHARED_DEVICE_SUPPORTED && fallback.reason != FallbackReason::NotConnected);
        let (mut stream, named) = self.open_shared(Some(request.shape.rate), device)?;
        stream.negotiated.fallback = Some(fallback);
        Ok((stream, named))
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
        self.status.parked.store(true, Ordering::Relaxed);
        self.status.awaiting_device.store(false, Ordering::Relaxed);
        self.feed.lead.clear();
    }

    /// Whether the missing stream was closed by [`Self::park`] rather than lost.
    pub fn is_parked(&self) -> bool {
        self.status.parked()
    }

    /// For a reader that can't wait out a reopen.
    pub fn status(&self) -> Arc<OutputStatus> {
        Arc::clone(&self.status)
    }

    pub fn request(&self) -> &OutputRequest {
        &self.request
    }

    /// Whether the open stream plays `wanted` with no reopen: its own request, or a claim differing
    /// only in the source where [`claim_serves`] says the device already runs right. A stand-in for
    /// a refused claim serves only its own request, and no stream serves anything.
    pub fn serves(&self, wanted: &OutputRequest) -> bool {
        if self.stream.is_none() {
            return false;
        }
        if self.request == *wanted {
            return true;
        }
        let (OutputRequest::Exclusive(open), OutputRequest::Exclusive(next)) =
            (&self.request, wanted)
        else {
            return false;
        };
        let Some(running) = self.negotiated() else {
            return false;
        };
        claim_serves(open, &running, next)
    }

    /// The voices, as one mixer. Handed to `player::playback::decks` at boot and not reachable any
    /// other way.
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

    /// What the device agreed to, or `None` while no stream is open.
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
