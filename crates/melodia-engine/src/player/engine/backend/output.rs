//! The device under the decks: reopening it, following each track's format into it, and taking
//! it exclusively.
//!
//! A reopen is decided at the track boundary, from the decoder, under the decks lock ahead of the
//! append, so nothing of the track plays in the old format. A crossfade or a gapless stage would
//! need the outgoing track playing across it, so a track that needs one starts at `EndOfStream`.
//!
//! An exclusive claim is held through a pause and given back on stop, which a long pause can end
//! in where the user asks.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use melodia_audio::player::source::audio::{AudioSource, SampleRate, Shape, SourceFormat};
use melodia_audio::player::source::file_decode::FileDecoder;
use melodia_core::error::{AppError, describe};
use melodia_playback::player::playback::decks::Decks;
use melodia_playback::player::playback::output::device::{ExternalVolume, Lead};
use melodia_playback::player::playback::output::{
    self, AudioOutput, ExclusiveRequest, ExclusiveTuning, Negotiated, OutputMode, OutputRequest,
    OutputStatus, RateFallback,
};
use parking_lot::Mutex;

use super::PlaybackEngine;
use crate::player::engine::signal_path::{self, SignalInputs, SignalPath, Transport};

/// How the user asked for the output to be opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputChoice {
    pub mode: OutputMode,
    /// The card an exclusive claim is aimed at, and the shared one's too where
    /// [`output::SHARED_DEVICE_SUPPORTED`]. `None` follows the system default.
    pub device: Option<String>,
    pub tuning: ExclusiveTuning,
    /// Carry the volume on the device's own control where an exclusive claim can.
    pub hardware_volume: bool,
    pub rate_fallback: RateFallback,
}

impl OutputChoice {
    /// `None` where the backend's ids can't name a shared target: a Linux card's would go round
    /// the sound server.
    pub fn shared_device(&self) -> Option<&str> {
        self.device.as_deref().filter(|_| output::SHARED_DEVICE_SUPPORTED)
    }

    fn shared_request(&self, rate: Option<SampleRate>) -> OutputRequest {
        OutputRequest::Shared { rate, device: self.shared_device().map(str::to_owned) }
    }
}

/// What opening a file for a track-boundary decision found.
#[derive(Debug, Clone, Copy)]
enum Probed {
    Decodes(Shape, SourceFormat),
    Unreadable,
}

/// The device under the decks, what it is asked to open for, and the cells it publishes, read
/// without the device's lock, which a reopen holds for as long as a device takes to open.
#[derive(Default)]
pub(super) struct EngineOutput {
    /// `None` on the device-free test rigs. Taken after the decks lock wherever both are held;
    /// `Arc` so a faded stop's deferred half can give an exclusive card back.
    pub(super) device: Arc<Mutex<Option<AudioOutput>>>,
    follow_rate: AtomicBool,
    release_when_paused: AtomicBool,
    choice: Mutex<OutputChoice>,
    /// How far the ear is behind the decks' clocks, measured by whichever stream is open.
    pub(super) lead: Arc<Lead>,
    external_volume: Arc<ExternalVolume>,
    /// Asked on every tick by the health checks, from a runtime worker.
    status: Arc<OutputStatus>,
    /// The last file a track-boundary decision opened, so the monitor's per-tick ask about the
    /// next track costs one open. The format rather than the verdict, which the output may change.
    probed: Mutex<Option<(String, Probed)>>,
}

impl EngineOutput {
    pub(super) fn over(output: AudioOutput) -> Self {
        Self {
            lead: output.lead(),
            external_volume: output.external_volume(),
            status: output.status(),
            device: Arc::new(Mutex::new(Some(output))),
            follow_rate: AtomicBool::new(false),
            release_when_paused: AtomicBool::new(false),
            choice: Mutex::default(),
            probed: Mutex::default(),
        }
    }
}

impl PlaybackEngine {
    /// Reopens the output on the last request, carrying on from where the decks are; `Ok(None)`
    /// while parked. Holds the decks lock throughout, so a transport op queues rather than timing
    /// out against a missing stream. Blocking.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no output to reopen, and the device's own open failure
    /// when it refuses.
    pub fn reopen_output(&self) -> Result<Option<Negotiated>, AppError> {
        let decks = self.lock_decks();
        let mut output = self.output.device.lock();
        let output = output
            .as_mut()
            .ok_or_else(|| AppError::player_msg("There is no audio output to reopen"))?;
        if output.is_parked() {
            return Ok(None);
        }
        let request = output.request().clone();
        reopen_routed(&decks, output, request).map(Some)
    }

    /// Closes the output for good, giving an exclusive claim and the device's volume back, since
    /// `process::exit` runs no destructor. Parked so the stall watch doesn't read it as lost.
    pub fn close_output(&self) {
        let closed = self.output.device.lock().take();
        if let Some(mut output) = closed {
            output.park();
        }
    }

    /// Sets the user's volume, as an amplitude, on the device's own control where a claim carries
    /// it, the voices staying at unity, and on the voices otherwise.
    pub fn set_volume(&self, volume: f64) {
        let decks = self.lock_decks();
        decks.set_volume_all(self.route_volume(volume));
    }

    /// Hands `volume` to the output and answers the gain the voices apply for it.
    pub(super) fn route_volume(&self, volume: f64) -> f64 {
        self.set_output_volume(volume);
        self.voice_gain(volume)
    }

    /// [`Self::route_volume`] around a reopen for `source`: handed over first, so a claim opens at
    /// the right level, and the gain read off whichever route the reopen landed on.
    pub(super) fn prepare_output_for(
        &self,
        decks: &MutexGuard<'_, Decks>,
        source: &impl AudioSource,
        volume: f64,
    ) -> f64 {
        self.set_output_volume(volume);
        self.reopen_for_track(decks, source.shape(), source.format());
        self.voice_gain(volume)
    }

    fn set_output_volume(&self, volume: f64) {
        if let Some(output) = self.output.device.lock().as_ref() {
            output.set_volume(volume);
        }
    }

    fn voice_gain(&self, volume: f64) -> f64 {
        self.output.device.lock().as_ref().map_or(volume, |output| output.voice_gain(volume))
    }

    /// Where the system moving the device's volume is reported, the same cell across reopens.
    pub fn external_volume(&self) -> Arc<ExternalVolume> {
        Arc::clone(&self.output.external_volume)
    }

    /// Whether the output was closed on purpose, so a stream that stopped beating is no fault.
    pub fn output_parked(&self) -> bool {
        self.output.status.parked()
    }

    /// Whether a pause holds an exclusive device the user asked to have given back. Asks the
    /// choice rather than the stream, so a shared stand-in for a refused claim counts too, and
    /// never takes the device's lock, which a reopen holds.
    pub fn holds_releasable_claim(&self) -> bool {
        self.output.release_when_paused.load(Ordering::Relaxed)
            && self.output.choice.lock().mode == OutputMode::Exclusive
            && !self.output_parked()
    }

    /// Whether the output stands in for a chosen device that wasn't connected, the only case
    /// [`Self::disconnected_device_returned`] looks for anything in.
    pub fn awaiting_disconnected_device(&self) -> bool {
        self.output.status.awaiting_device()
    }

    /// Whether a reopen holds the decks lock, for a poll that would rather skip than wait.
    pub fn output_reopening(&self) -> bool {
        self.output.status.reopening()
    }

    pub fn negotiated(&self) -> Option<Negotiated> {
        self.output.device.lock().as_ref().and_then(AudioOutput::negotiated)
    }

    /// Whether a stand-in's missing device is listed again. A track start only asks where it
    /// reopens anyway, which a gapless album never does. Blocking.
    pub fn disconnected_device_returned(&self) -> bool {
        if !self.awaiting_disconnected_device() {
            return false;
        }
        let chosen = self.output.choice.lock().device.clone();
        let listed = output::devices();
        match chosen {
            Some(id) => listed.iter().any(|device| device.id == id),
            None => !listed.is_empty(),
        }
    }

    /// The path the playing source takes to the device. Takes the decks lock and then the output
    /// lock, never both at once.
    pub fn signal_path(&self, transport: Transport) -> Option<SignalPath> {
        let source = self.lock_decks().active().voice.playing()?;
        let negotiated = self.negotiated()?;
        Some(signal_path::evaluate(SignalInputs {
            negotiated,
            source,
            eq_on: self.eq.enabled(),
            rg_on: self.rg.enabled(),
            transport,
        }))
    }

    /// Opens at each track's own rate from the next boundary on. Stays off where that changes
    /// nothing, whatever a settings file says: it would only reopen the output at every track.
    pub fn set_follow_rate(&self, on: bool) {
        self.output.follow_rate.store(on && output::FOLLOW_RATE_SUPPORTED, Ordering::Relaxed);
    }

    /// Stays off where nothing can be claimed, whatever a settings file says.
    pub fn set_release_when_paused(&self, on: bool) {
        self.output.release_when_paused.store(on && output::EXCLUSIVE_SUPPORTED, Ordering::Relaxed);
    }

    /// Writes `hold` of silence after each reopen onto a new rate. Waits out a reopen in flight.
    pub fn set_resync_hold(&self, hold: Duration) {
        if let Some(output) = self.output.device.lock().as_mut() {
            output.set_resync_hold(hold);
        }
    }

    /// Takes the card or gives it back now rather than at the next track. With nothing loaded an
    /// exclusive choice waits for the first play to claim, and a shared one reopens shared, which
    /// is what hands the card back. Blocking.
    pub fn set_output_choice(&self, choice: OutputChoice) {
        let exclusive = choice.mode == OutputMode::Exclusive;
        *self.output.choice.lock() = choice;
        let decks = self.lock_decks();
        let wanted = match decks.active().voice.playing() {
            Some(playing) => Some(self.wanted_request(playing.shape, playing.format)),
            None if exclusive => None,
            None => Some(self.output.choice.lock().shared_request(None)),
        };
        let mut output = self.output.device.lock();
        let Some(output) = output.as_mut() else {
            return;
        };
        output.forget_refusal();
        let Some(wanted) = wanted else {
            output.park();
            return;
        };
        if output.serves(&wanted) {
            return;
        }
        match reopen_routed(&decks, output, wanted) {
            Ok(negotiated) => log::info!("Output reopened for a new output choice: {negotiated:?}"),
            Err(e) => log::warn!("Failed to reopen the output: {}", describe(&e)),
        }
    }

    fn wanted_request(&self, shape: Shape, format: SourceFormat) -> OutputRequest {
        let choice = self.output.choice.lock();
        match choice.mode {
            OutputMode::Exclusive => OutputRequest::Exclusive(ExclusiveRequest {
                device: choice.device.clone(),
                shape,
                format,
                tuning: choice.tuning,
                hardware_volume: choice.hardware_volume,
                rate_fallback: choice.rate_fallback,
            }),
            OutputMode::Shared => {
                let rate = self.output.follow_rate.load(Ordering::Relaxed).then_some(shape.rate);
                choice.shared_request(rate)
            }
        }
    }

    pub(super) fn plays_without_reopen(&self, shape: Shape, format: SourceFormat) -> bool {
        let wanted = self.wanted_request(shape, format);
        self.output.device.lock().as_ref().is_none_or(|output| output.serves(&wanted))
    }

    /// Whether the output has to reopen for the file at `path`, which no crossfade can cross; a
    /// file that won't open counts, so nothing fades into it. Opens the file on the first ask.
    pub fn reopens_for(&self, path: &str) -> bool {
        let plays = self.recorded_answer(path).unwrap_or_else(|| match self.open_recorded(path) {
            Ok(decoded) => self.plays_without_reopen(decoded.shape(), decoded.format()),
            Err(e) => {
                log::debug!("Not crossfading into {path}, which won't open: {}", describe(&e));
                false
            }
        });
        !plays
    }

    /// Opens `path` to stage behind the playing track, `None` where the output would reopen for
    /// it. Asked every tick until the track ends, so a refusal opens the file once.
    ///
    /// # Errors
    ///
    /// Whatever [`FileDecoder::open`] refuses the file with, the first time it is asked about.
    pub(super) fn open_for_gapless(&self, path: &str) -> Result<Option<FileDecoder>, AppError> {
        if self.recorded_answer(path) == Some(false) {
            return Ok(None);
        }
        let decoded = self.open_recorded(path)?;
        // Left unstaged, it ends in `EndOfStream` and starts through `play_media`, which reopens.
        if !self.plays_without_reopen(decoded.shape(), decoded.format()) {
            log::debug!("Not staging {path} gapless: the output reopens for its format");
            return Ok(None);
        }
        Ok(Some(decoded))
    }

    /// `None` until something has opened `path`.
    fn recorded_answer(&self, path: &str) -> Option<bool> {
        let found = self
            .output
            .probed
            .lock()
            .as_ref()
            .and_then(|(probed, found)| (probed == path).then_some(*found))?;
        Some(match found {
            Probed::Decodes(shape, format) => self.plays_without_reopen(shape, format),
            Probed::Unreadable => false,
        })
    }

    /// Opens `path`, recording what it holds for the asks after.
    ///
    /// # Errors
    ///
    /// Whatever [`FileDecoder::open`] refuses the file with.
    fn open_recorded(&self, path: &str) -> Result<FileDecoder, AppError> {
        let opened = FileDecoder::open(Path::new(path));
        let found = match &opened {
            Ok(decoded) => Probed::Decodes(decoded.shape(), decoded.format()),
            Err(_) => Probed::Unreadable,
        };
        *self.output.probed.lock() = Some((path.to_owned(), found));
        opened
    }

    /// Reopens the output for a track starting fresh, inside a transport op between its decode
    /// and its append, which the decks guard proves.
    ///
    /// A shared output following the rate reopens even at the rate it has: `PipeWire` settles the
    /// card's rate when a stream starts, and ours never idles, so another app's rate would stick
    /// after it left. A same-rate reopen writes no resync silence. A claim that fell back is
    /// retried here too, where `FallbackReason::retry_at_track_start` allows.
    fn reopen_for_track(&self, decks: &MutexGuard<'_, Decks>, shape: Shape, format: SourceFormat) {
        let wanted = self.wanted_request(shape, format);
        let mut output = self.output.device.lock();
        let Some(output) = output.as_mut() else {
            return;
        };
        let open = output.negotiated();
        let previous_rate = open.as_ref().map(|negotiated| negotiated.shape.rate);
        let retry_claim = open
            .and_then(|negotiated| negotiated.fallback)
            .is_some_and(|fallback| fallback.reason.retry_at_track_start());
        let reclaim = matches!(wanted, OutputRequest::Shared { rate: Some(_), .. }) || retry_claim;
        if output.serves(&wanted) && !reclaim {
            return;
        }
        match reopen_routed(decks, output, wanted) {
            Ok(negotiated) if previous_rate == Some(negotiated.shape.rate) => {
                log::debug!("Output reopened: {negotiated:?}");
            }
            Ok(negotiated) => log::info!("Output reopened: {negotiated:?}"),
            Err(e) => log::warn!("Failed to reopen the output: {}", describe(&e)),
        }
    }
}

/// Reopens `output` with the voices at the software gain across it, unity only after and only
/// where the new stream took the device's control, so a change of route dips and never jumps.
fn reopen_routed(
    decks: &Decks,
    output: &mut AudioOutput,
    request: OutputRequest,
) -> Result<Negotiated, AppError> {
    let volume = output.volume();
    decks.set_volume_all(volume);
    let reopened = output.reopen(request);
    decks.set_volume_all(output.voice_gain(volume));
    reopened
}

/// Gives an exclusive card back. A shared stream stays open, holding nothing anyone wants.
pub(super) fn release_exclusive(output: &Mutex<Option<AudioOutput>>) {
    if let Some(output) = output.lock().as_mut()
        && matches!(output.request(), OutputRequest::Exclusive(_))
    {
        output.park();
    }
}

#[cfg(all(test, any(target_os = "windows", target_os = "linux")))]
#[path = "tests/output_tests.rs"]
mod tests;
