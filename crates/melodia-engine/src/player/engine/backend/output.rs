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

use std::sync::MutexGuard;
use std::sync::atomic::Ordering;
use std::time::Duration;

use melodia_audio::player::source::audio::{Shape, SourceFormat};
use melodia_core::error::{AppError, describe};
use melodia_playback::player::playback::decks::Decks;
use melodia_playback::player::playback::output::{
    AudioOutput, ExclusiveRequest, ExclusiveTuning, Negotiated, OutputMode, OutputRequest,
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
        let _decks = self.lock_decks();
        let mut output = self.output.lock();
        let output = output
            .as_mut()
            .ok_or_else(|| AppError::Player("There is no audio output to reopen".to_owned()))?;
        if output.is_parked() {
            return Ok(None);
        }
        output.reopen(output.request().clone()).map(Some)
    }

    /// Whether the output was closed on purpose, so a stream that stopped beating is not a fault.
    pub fn output_parked(&self) -> bool {
        self.output.lock().as_ref().is_some_and(AudioOutput::is_parked)
    }

    /// What the device agreed to, or `None` while no stream is open.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.output.lock().as_ref().and_then(AudioOutput::negotiated)
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
    pub fn set_follow_rate(&self, on: bool) {
        self.follow_rate.store(on, Ordering::Relaxed);
    }

    /// Write `hold` of silence after each reopen onto a new rate, from the next one on. Blocking:
    /// it waits out any reopen in flight.
    pub fn set_resync_hold(&self, hold: Duration) {
        if let Some(output) = self.output.lock().as_mut() {
            output.set_resync_hold(hold);
        }
    }

    /// Whether the output is reopened to each track's rate, which exclusive output always is.
    pub(super) fn follows_rate(&self) -> bool {
        self.follow_rate.load(Ordering::Relaxed)
            || self.output_choice.lock().mode == OutputMode::Exclusive
    }

    /// Take the card for ourselves or give it back, now rather than at the next track.
    ///
    /// Whatever is loaded, paused included, reopens under the new choice. With nothing loaded an
    /// exclusive choice waits for the first play to claim, and a shared one reopens the default
    /// device, which is what hands the card back to everything else. Blocking: it opens a device.
    pub fn set_output_choice(&self, choice: OutputChoice) {
        let exclusive = choice.mode == OutputMode::Exclusive;
        *self.output_choice.lock() = choice;
        let decks = self.lock_decks();
        let wanted = match decks.active().voice.playing() {
            Some(playing) => Some(self.wanted_request(playing.shape, playing.format)),
            None if exclusive => None,
            None => Some(OutputRequest::default()),
        };
        let mut output = self.output.lock();
        let Some(output) = output.as_mut() else {
            return;
        };
        let Some(wanted) = wanted else {
            output.park();
            return;
        };
        if output.request() == &wanted && !output.is_parked() {
            return;
        }
        match output.reopen(wanted) {
            Ok(negotiated) => log::info!("Output reopened for a new output choice: {negotiated:?}"),
            Err(e) => log::warn!("Failed to reopen the output: {}", describe(&e)),
        }
    }

    /// What the output should be asked for while a source in `shape` and `format` plays.
    fn wanted_request(&self, shape: Shape, format: SourceFormat) -> OutputRequest {
        let choice = self.output_choice.lock();
        match choice.mode {
            OutputMode::Exclusive => OutputRequest::Exclusive(ExclusiveRequest {
                device: choice.device.clone(),
                shape,
                format,
                tuning: choice.tuning,
            }),
            OutputMode::Shared => OutputRequest::Shared {
                rate: self.follow_rate.load(Ordering::Relaxed).then_some(shape.rate),
            },
        }
    }

    /// Whether a source in `shape` and `format` can play on the output as it is, with no reopen.
    /// Always true with no output, which is the device-free rigs the tests build.
    pub(super) fn plays_without_reopen(&self, shape: Shape, format: SourceFormat) -> bool {
        let wanted = self.wanted_request(shape, format);
        self.output.lock().as_ref().is_none_or(|output| output.request() == &wanted)
    }

    /// Reopen the output for a track starting fresh in `shape` and `format`.
    ///
    /// **A shared output following the rate reopens even at the rate already asked for.**
    /// `PipeWire` settles the card's rate when a stream starts, not when one leaves, and ours
    /// never idles, so a rate another app held the card at when ours opened would stick after that
    /// app left. A same-rate reopen writes no resync silence, so a track start is the cheap place
    /// to take the card back. An exclusive claim that fell back is retried here for the same
    /// reason: whatever held the card may have let go. A gapless transition doesn't come through
    /// here and keeps the output as it is.
    ///
    /// Takes the decks guard as proof it is held: this runs inside a transport op, between its
    /// decode and its append. A failure is logged and playback carries on over whatever
    /// `AudioOutput::reopen` fell back to.
    pub(super) fn reopen_for_track(
        &self,
        _decks: &MutexGuard<'_, Decks>,
        shape: Shape,
        format: SourceFormat,
    ) {
        let wanted = self.wanted_request(shape, format);
        let mut output = self.output.lock();
        let Some(output) = output.as_mut() else {
            return;
        };
        let reclaim = matches!(wanted, OutputRequest::Shared { rate: Some(_) })
            || output.negotiated().is_some_and(|negotiated| negotiated.fallback.is_some());
        if output.request() == &wanted && !output.is_parked() && !reclaim {
            return;
        }
        let previous_rate = output.negotiated().map(|negotiated| negotiated.shape.rate);
        match output.reopen(wanted) {
            Ok(negotiated) if previous_rate == Some(negotiated.shape.rate) => {
                log::debug!("Output reopened: {negotiated:?}");
            }
            Ok(negotiated) => log::info!("Output reopened: {negotiated:?}"),
            Err(e) => log::warn!("Failed to reopen the output: {}", describe(&e)),
        }
    }
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
