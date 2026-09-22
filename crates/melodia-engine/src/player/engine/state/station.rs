//! A station's session on the deck: connecting, connected, failed, and the generation that refuses
//! a connect the user already moved on from.

use std::borrow::Cow;
use std::sync::Arc;

use melodia_core::entities::radio::recompose_format;

use super::{PlayerAction, PlayerState};
use crate::player::engine::types::{PlaybackSource, PlaybackStatus, RadioNowPlaying};

impl PlayerState {
    /// Point the state at `station` and start connecting to it.
    ///
    /// Returns the session generation the caller must carry through to
    /// [`Self::build_station_connected_actions`], and the actions that clear the decks: the queue
    /// itself is deliberately untouched, so stopping the station later resumes the library exactly
    /// where it was (D9). The speed pin's `SetSpeed` follows the `Stop` so it lands on emptied decks.
    pub fn build_station_connecting_actions(
        &mut self,
        station: Arc<RadioNowPlaying>,
    ) -> (u64, Vec<PlayerAction>) {
        self.end_stream_session();
        self.status = PlaybackStatus::Loading;
        self.source = Some(PlaybackSource::Station(station));
        self.duration_ms = 0;
        self.position_ms = 0;
        self.pause_after_current_track = false;

        let mut actions = vec![PlayerAction::Stop { fade_ms: 0 }];
        actions.extend(self.pin_speed_for_station());
        (self.radio_generation, actions)
    }

    /// Put the deck back on 1.0 for a station that is about to sit on it, and hand back the
    /// action that lands it on the deck.
    ///
    /// [`Self::build_set_speed_actions`] refuses to move speed while a station plays, so without
    /// this the transport would claim a rate the deck is not running at. Shared with
    /// [`restore_station`](super::restore_station), the one other way a station reaches the deck,
    /// because the state and the backend have to move together: skip the action and a stream opens
    /// against a deck still resampling, skip the field and the disabled speed row keeps quoting a
    /// rate nothing runs at.
    pub(super) fn pin_speed_for_station(&mut self) -> Option<PlayerAction> {
        if (self.playback_speed - 1.0).abs() <= f64::EPSILON {
            return None;
        }
        self.playback_speed = 1.0;
        Some(PlayerAction::SetSpeed(1.0))
    }

    /// The stream opened: start it, unless the user moved on while it was connecting.
    pub fn build_station_connected_actions(&mut self, generation: u64) -> Vec<PlayerAction> {
        if !self.is_current_station_session(generation) {
            return vec![];
        }
        self.status = PlaybackStatus::Playing;
        vec![PlayerAction::PlayStream { generation, volume: self.effective_volume() }]
    }

    /// Name the format the open just measured, on the station already sitting on the bar.
    ///
    /// The row a station was tuned from carries the directory's word, which names the *container*:
    /// an Ogg mount is filed under `OGG` whether it holds Vorbis or Opus, and the response's
    /// content type says no more. The open is the first moment anything knows better, and the same
    /// correction goes to the row itself — this is what keeps the first play from stating the old
    /// answer until the next one.
    ///
    /// Composed against what is already on the bar rather than against the row, which this layer
    /// does not hold. That value already carries whatever the last play measured, which is
    /// [`recompose_format`]'s whole subject.
    ///
    /// Session-checked like everything else arriving off an open, and a no-op where the two agree,
    /// so the `Arc`'s copy-on-write is paid once per station rather than once per tune.
    pub fn adopt_probed_codec(&mut self, generation: u64, codec: &str) {
        if codec.is_empty() || !self.is_current_station_session(generation) {
            return;
        }
        let Some(station) = self.station_mut() else {
            return;
        };
        let composed = recompose_format(station.codec.as_deref().unwrap_or_default(), codec);
        if composed.as_deref() == station.codec.as_deref() {
            return;
        }
        // Owned only past the guard, so the tune that measures what the bar already says costs
        // neither the string nor the `Arc`.
        let composed = composed.map(Cow::into_owned);
        Arc::make_mut(station).codec = composed;
    }

    /// The stream could not be opened. Clears the station rather than leaving a play button that
    /// would only fail the same way; the caller has already said so in a toast.
    pub fn build_station_failed_actions(&mut self, generation: u64) -> Vec<PlayerAction> {
        if !self.is_current_station_session(generation) {
            return vec![];
        }
        self.source = None;
        self.status = PlaybackStatus::Stopped;
        vec![PlayerAction::Stop { fade_ms: 0 }]
    }

    /// Whether `generation` is still the session the state is on. An open takes seconds and a
    /// click takes none, so anything arriving from one has to ask.
    fn is_current_station_session(&self, generation: u64) -> bool {
        self.is_radio() && self.radio_generation == generation
    }

    /// Invalidate the current station session, so whatever is still connecting for it is refused
    /// rather than played late. Called by every transition that starts or ends one.
    pub(super) fn end_stream_session(&mut self) {
        self.radio_generation = self.radio_generation.wrapping_add(1);
        if let Some(station) = self.station_mut() {
            Arc::make_mut(station).buffering = false;
        }
    }
}
