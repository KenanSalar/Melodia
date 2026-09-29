//! The device under the decks: reopening it, following each track's format into it, and taking
//! it exclusively.
//!
//! **A reopen is decided at the track boundary, from the decoder, before the track is heard.**
//! It happens under the decks lock, ahead of the append that starts the track, so nothing of it
//! plays in the old format. That is also why a crossfade and a reopen cannot share a transition,
//! and why a gapless stage refuses a track that needs one: both would need the outgoing track
//! still playing across it.
//!
//! **An exclusive claim is held through a pause and given back on stop**, so a stopped player
//! never keeps other applications off the card.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use melodia_audio::player::source::audio::{AudioSource, Shape, SourceFormat};
use melodia_core::error::{AppError, describe};
use melodia_playback::player::playback::decks::Decks;
use melodia_playback::player::playback::output::device::{ExternalVolume, Lead};
use melodia_playback::player::playback::output::{
    self, AudioOutput, ExclusiveRequest, ExclusiveTuning, Negotiated, OutputMode, OutputRequest,
    OutputStatus,
};
use parking_lot::Mutex;

use super::PlaybackEngine;
use crate::player::engine::signal_path::{self, SignalInputs, SignalPath, Transport};

/// How the user asked for the output to be opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputChoice {
    pub mode: OutputMode,
    /// The card an exclusive claim is aimed at, or `None` for the first the system lists.
    pub device: Option<String>,
    pub tuning: ExclusiveTuning,
    /// Carry the volume on the device's own control where an exclusive claim can.
    pub hardware_volume: bool,
}

/// The device under the decks, and the cells read of it without its lock, which a reopen holds
/// for as long as a device takes to open.
#[derive(Default)]
pub(super) struct EngineOutput {
    /// `None` on the device-free rigs the tests build. Only ever locked under the decks lock,
    /// which is the order `reopen_output` takes them in. `Arc` so the deferred half of a faded
    /// stop can give an exclusive card back once the fade has played out.
    pub(super) device: Arc<Mutex<Option<AudioOutput>>>,
    /// Open the output at each track's own rate. Read only at a track boundary, never by the audio
    /// thread.
    follow_rate: AtomicBool,
    /// Shared or exclusive, and which card. Read at a track boundary, never by the audio thread.
    choice: Mutex<OutputChoice>,
    /// How far the ear is behind the decks' clocks, measured by whichever stream is open.
    pub(super) lead: Arc<Lead>,
    /// Where a claim carrying the volume on the device reports the system moved it.
    external_volume: Arc<ExternalVolume>,
    /// Whether the output is parked, waiting on a disconnected device, or reopening: the health
    /// checks ask on every tick, from a runtime worker.
    status: Arc<OutputStatus>,
}

impl EngineOutput {
    /// `output`, with the cells it publishes for every stream it opens.
    pub(super) fn over(output: AudioOutput) -> Self {
        Self {
            lead: output.lead(),
            external_volume: output.external_volume(),
            status: output.status(),
            device: Arc::new(Mutex::new(Some(output))),
            follow_rate: AtomicBool::new(false),
            choice: Mutex::default(),
        }
    }
}

impl PlaybackEngine {
    /// Reopen the output on what the last stream was asked for, and carry on from where the
    /// decks are. `Ok(None)` while the output is parked, which nothing should reopen.
    ///
    /// **The decks lock is held across the whole reopen**, so a transport op queues behind it
    /// instead of sending a command to a stream that isn't there and waiting out
    /// `output::voice::SERVICE_TIMEOUT` for an answer. Blocking: it opens a device.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no output to reopen, or the device refuses to open.
    pub fn reopen_output(&self) -> Result<Option<Negotiated>, AppError> {
        let decks = self.lock_decks();
        let mut output = self.output.device.lock();
        let output = output
            .as_mut()
            .ok_or_else(|| AppError::Player("There is no audio output to reopen".to_owned()))?;
        if output.is_parked() {
            return Ok(None);
        }
        let request = output.request().clone();
        reopen_routed(&decks, output, request).map(Some)
    }

    /// Close the output for good, giving an exclusive claim back and the device's own volume with
    /// it. For the way out: `process::exit` runs no destructor, and a parked output would be
    /// reopened by a track starting while the monitor still runs. Parked on the way, so the stall
    /// watch doesn't take the missing stream for a lost one while the process exits.
    pub fn close_output(&self) {
        let closed = self.output.device.lock().take();
        if let Some(mut output) = closed {
            output.park();
        }
    }

    /// Hand the user's `volume`, as an amplitude, to the output and answer the gain the voices
    /// apply for it on the stream open now.
    pub(super) fn route_volume(&self, volume: f64) -> f64 {
        self.set_output_volume(volume);
        self.voice_gain(volume)
    }

    /// [`Self::route_volume`] for a track starting fresh from `source`, reopening the output for it
    /// in between: handed over first, so a claim opens with the device's own control already
    /// there, and the gain read after, off whichever route the reopen landed on.
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

    /// Hand the user's volume, as an amplitude, to the output for a claim carrying it on the
    /// device's own control.
    fn set_output_volume(&self, volume: f64) {
        if let Some(output) = self.output.device.lock().as_ref() {
            output.set_volume(volume);
        }
    }

    /// The gain the voices apply for `volume` on the output as it is. `volume` itself with no
    /// output, which is the device-free rigs the tests build.
    fn voice_gain(&self, volume: f64) -> f64 {
        self.output.device.lock().as_ref().map_or(volume, |output| output.voice_gain(volume))
    }

    /// Where the system moving the device's own volume control is reported, while a claim carries
    /// the volume there, for the player to follow. Lock-free, and the same cell across reopens.
    pub fn external_volume(&self) -> Arc<ExternalVolume> {
        Arc::clone(&self.output.external_volume)
    }

    /// Whether the output was closed on purpose, so a stream that stopped beating is not a fault.
    /// Lock-free, so a health check never waits out a reopen.
    pub fn output_parked(&self) -> bool {
        self.output.status.parked()
    }

    /// Whether the output stands in for a claim whose device wasn't connected, which is the only
    /// case [`Self::disconnected_device_returned`] has anything to look for. Lock-free.
    pub fn awaiting_disconnected_device(&self) -> bool {
        self.output.status.awaiting_device()
    }

    /// Whether a reopen is holding the decks lock across a device open, for a poll that would
    /// rather skip a round than wait on it or read its silent gap as a stall. Lock-free.
    pub fn output_reopening(&self) -> bool {
        self.output.status.reopening()
    }

    /// What the device agreed to, or `None` while no stream is open.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.output.device.lock().as_ref().and_then(AudioOutput::negotiated)
    }

    /// Whether a claim that fell back because its device wasn't connected can find it listed
    /// again, so it is worth reclaiming now: a fallback is otherwise retried only at the next
    /// track start, which a gapless album never reaches. Lists the devices only while such a
    /// fallback stands. Blocking.
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

    /// The path the playing source takes to the device, or `None` while nothing plays or no
    /// stream is open. Takes the decks lock and then the output lock, never both at once.
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

    /// Open the output at each track's own rate from the next track on, or go back to the
    /// device's own config. Lock-free; nothing reopens until a track boundary asks.
    ///
    /// Stays off where that changes nothing ([`output::FOLLOW_RATE_SUPPORTED`]), whatever a
    /// settings file says: it would only reopen the output at every track and turn crossfade off.
    pub fn set_follow_rate(&self, on: bool) {
        self.output.follow_rate.store(on && output::FOLLOW_RATE_SUPPORTED, Ordering::Relaxed);
    }

    /// Write `hold` of silence after each reopen onto a new rate, from the next one on. Blocking:
    /// it waits out any reopen in flight.
    pub fn set_resync_hold(&self, hold: Duration) {
        if let Some(output) = self.output.device.lock().as_mut() {
            output.set_resync_hold(hold);
        }
    }

    /// Whether the output is reopened to each track's rate, which exclusive output always is.
    pub(super) fn follows_rate(&self) -> bool {
        self.output.follow_rate.load(Ordering::Relaxed)
            || self.output.choice.lock().mode == OutputMode::Exclusive
    }

    /// Take the card for ourselves or give it back, now rather than at the next track.
    ///
    /// Whatever is loaded, paused included, reopens under the new choice. With nothing loaded an
    /// exclusive choice waits for the first play to claim, and a shared one reopens the default
    /// device, which is what hands the card back to everything else. Blocking: it opens a device.
    pub fn set_output_choice(&self, choice: OutputChoice) {
        let exclusive = choice.mode == OutputMode::Exclusive;
        *self.output.choice.lock() = choice;
        let decks = self.lock_decks();
        let wanted = match decks.active().voice.playing() {
            Some(playing) => Some(self.wanted_request(playing.shape, playing.format)),
            None if exclusive => None,
            None => Some(OutputRequest::default()),
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
        if output.serves(&wanted) && !output.is_parked() {
            return;
        }
        match reopen_routed(&decks, output, wanted) {
            Ok(negotiated) => log::info!("Output reopened for a new output choice: {negotiated:?}"),
            Err(e) => log::warn!("Failed to reopen the output: {}", describe(&e)),
        }
    }

    /// What the output should be asked for while a source in `shape` and `format` plays.
    fn wanted_request(&self, shape: Shape, format: SourceFormat) -> OutputRequest {
        let choice = self.output.choice.lock();
        match choice.mode {
            OutputMode::Exclusive => OutputRequest::Exclusive(ExclusiveRequest {
                device: choice.device.clone(),
                shape,
                format,
                tuning: choice.tuning,
                hardware_volume: choice.hardware_volume,
            }),
            OutputMode::Shared => OutputRequest::Shared {
                rate: self.output.follow_rate.load(Ordering::Relaxed).then_some(shape.rate),
            },
        }
    }

    /// Whether a source in `shape` and `format` can play on the output as it is, with no reopen.
    /// Always true with no output, which is the device-free rigs the tests build.
    pub(super) fn plays_without_reopen(&self, shape: Shape, format: SourceFormat) -> bool {
        let wanted = self.wanted_request(shape, format);
        self.output.device.lock().as_ref().is_none_or(|output| output.serves(&wanted))
    }

    /// Reopen the output for a track starting fresh in `shape` and `format`.
    ///
    /// **A shared output following the rate reopens even at the rate already asked for.**
    /// `PipeWire` settles the card's rate when a stream starts, not when one leaves, and ours
    /// never idles, so a rate another app held the card at when ours opened would stick after that
    /// app left. A same-rate reopen writes no resync silence, so a track start is the cheap place
    /// to take the card back. An exclusive claim that fell back is retried here for the same
    /// reason, since whatever held the card may have let go, but only where the refusal can pass:
    /// a rate or format the card lacks is refused again, and retrying it would restart the shared
    /// stream at every skip. A gapless transition doesn't come through here and keeps the output
    /// as it is.
    ///
    /// Takes the decks guard as proof it is held: this runs inside a transport op, between its
    /// decode and its append. A failure is logged and playback carries on over whatever
    /// `AudioOutput::reopen` fell back to.
    fn reopen_for_track(&self, decks: &MutexGuard<'_, Decks>, shape: Shape, format: SourceFormat) {
        let wanted = self.wanted_request(shape, format);
        let mut output = self.output.device.lock();
        let Some(output) = output.as_mut() else {
            return;
        };
        let reclaim = matches!(wanted, OutputRequest::Shared { rate: Some(_) })
            || output
                .negotiated()
                .and_then(|negotiated| negotiated.fallback)
                .is_some_and(|fallback| fallback.reason.may_pass_later());
        if output.serves(&wanted) && !output.is_parked() && !reclaim {
            return;
        }
        let previous_rate = output.negotiated().map(|negotiated| negotiated.shape.rate);
        match reopen_routed(decks, output, wanted) {
            Ok(negotiated) if previous_rate == Some(negotiated.shape.rate) => {
                log::debug!("Output reopened: {negotiated:?}");
            }
            Ok(negotiated) => log::info!("Output reopened: {negotiated:?}"),
            Err(e) => log::warn!("Failed to reopen the output: {}", describe(&e)),
        }
    }
}

/// Reopen `output` for `request`, with the voices' gain on whichever side of the device carries
/// the volume once it has.
///
/// **The software gain across the reopen, unity only after it**, and only where the new stream took
/// the device's own control. Whatever plays in between is attenuated twice or at the software
/// gain, so a change of route, a fallback to shared included, dips and never jumps.
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

/// Close an exclusive stream so the card goes back to everything else. A shared one stays open:
/// it holds nothing another application wants.
pub(super) fn release_exclusive(output: &Mutex<Option<AudioOutput>>) {
    if let Some(output) = output.lock().as_mut()
        && matches!(output.request(), OutputRequest::Exclusive(_))
    {
        output.park();
    }
}
