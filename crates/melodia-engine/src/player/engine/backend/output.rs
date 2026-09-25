//! The device under the decks: reopening it, and following each track's sample rate into it.
//!
//! **A rate change is decided at the track boundary, from the decoder, before the track is heard.**
//! The reopen happens under the decks lock, ahead of the append that starts the track, so nothing
//! of it plays at the old rate. That is also why a crossfade and a rate change cannot share a
//! transition, and why a gapless stage refuses a track at another rate: both would need the
//! outgoing track still playing across the reopen.

use std::sync::MutexGuard;
use std::sync::atomic::Ordering;

use melodia_audio::player::source::audio::SampleRate;
use melodia_core::error::{AppError, describe};
use melodia_playback::player::playback::decks::Decks;
use melodia_playback::player::playback::output::device::Negotiated;
use melodia_playback::player::playback::output::{AudioOutput, OutputRequest};

use super::PlaybackEngine;

impl PlaybackEngine {
    /// Reopen the output on whatever the default device is now, asking for what the last stream
    /// was asked for, and carry on from where the decks are.
    ///
    /// **The decks lock is held across the whole reopen**, so a transport op queues behind it
    /// instead of sending a command to a stream that isn't there and waiting out
    /// `output::voice::SERVICE_TIMEOUT` for an answer. Blocking: it opens a device.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when there is no output to reopen, or the device refuses to open.
    pub fn reopen_output(&self) -> Result<Negotiated, AppError> {
        let _decks = self.lock_decks();
        let mut output = self.output.lock();
        let output = output
            .as_mut()
            .ok_or_else(|| AppError::Player("There is no audio output to reopen".to_owned()))?;
        output.reopen(output.request())
    }

    /// What the device agreed to, or `None` while no stream is open.
    pub fn negotiated(&self) -> Option<Negotiated> {
        self.output.lock().as_ref().and_then(AudioOutput::negotiated)
    }

    /// Open the output at each track's own rate from the next track on, or go back to the
    /// device's own config. Lock-free; nothing reopens until a track boundary asks.
    pub fn set_follow_rate(&self, on: bool) {
        self.follow_rate.store(on, Ordering::Relaxed);
    }

    pub(super) fn follows_rate(&self) -> bool {
        self.follow_rate.load(Ordering::Relaxed)
    }

    /// What the output should be asked for while a source at `rate` plays.
    fn wanted_request(&self, rate: SampleRate) -> OutputRequest {
        OutputRequest { rate: self.follows_rate().then_some(rate) }
    }

    /// Whether a source at `rate` can play on the output as it is, with no reopen. Always true
    /// with no output, which is the device-free rigs the tests build.
    pub(super) fn plays_without_reopen(&self, rate: SampleRate) -> bool {
        self.output
            .lock()
            .as_ref()
            .is_none_or(|output| output.request() == self.wanted_request(rate))
    }

    /// Reopen the output for a track starting fresh at `rate`.
    ///
    /// **While following, this reopens even at the rate already asked for.** `PipeWire` picks the
    /// card's rate only when a stream starts on an idle graph, and ours never idles, so a rate
    /// another app held the card at when ours opened would stick after that app left. A same-rate
    /// reopen writes no resync silence, so a track start is the cheap place to take the card back.
    /// A gapless transition doesn't come through here and keeps its rate as it is.
    ///
    /// Takes the decks guard as proof it is held: this runs inside a transport op, between its
    /// decode and its append. A failure is logged and playback carries on over whatever
    /// `AudioOutput::reopen` fell back to.
    pub(super) fn reopen_for_track(&self, _decks: &MutexGuard<'_, Decks>, rate: SampleRate) {
        let wanted = self.wanted_request(rate);
        let mut output = self.output.lock();
        let Some(output) = output.as_mut() else {
            return;
        };
        if output.request() == wanted && !self.follows_rate() {
            return;
        }
        let previous_rate = output.negotiated().map(|negotiated| negotiated.shape.rate);
        match output.reopen(wanted) {
            Ok(negotiated) if previous_rate == Some(negotiated.shape.rate) => {
                log::debug!("Output reopened at {} Hz", negotiated.shape.rate);
            }
            Ok(negotiated) => {
                log::info!("Output reopened for {wanted:?}: {} Hz", negotiated.shape.rate);
            }
            Err(e) => log::warn!("Failed to reopen the output for {wanted:?}: {}", describe(&e)),
        }
    }
}
