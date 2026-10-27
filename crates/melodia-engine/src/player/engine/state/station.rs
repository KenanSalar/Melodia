//! A station's session on the deck: connecting, connected, failed, and the generation that refuses
//! a connect the user already moved on from.

use std::borrow::Cow;
use std::sync::Arc;

use melodia_core::entities::radio::recompose_format;

use super::{PlayerAction, PlayerState};
use crate::player::engine::types::{PlaybackSource, PlaybackStatus, RadioNowPlaying};

impl PlayerState {
    /// Points the state at `station`, returning the generation
    /// [`Self::build_station_connected_actions`] needs. The queue stays untouched, so a stop hands
    /// the library back; the speed pin follows the `Stop` so it lands on emptied decks.
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

    /// Puts the speed back on 1.0 for a station, which [`Self::build_set_speed_actions`] then
    /// holds. Shared with [`restore_station`](super::restore_station) because the state and the
    /// deck move together, or the speed row quotes a rate nothing runs at.
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

    /// Names the format the open measured on the bar's station. The directory's word names the
    /// container (an Ogg mount is `OGG` whether Vorbis or Opus), and the row's correction would
    /// reach the bar only on the next play. Composed against the bar's value through
    /// [`recompose_format`], and a no-op where the two agree.
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
        // Owned only past the guard, so a tune measuring what the bar says allocates nothing.
        let composed = composed.map(Cow::into_owned);
        Arc::make_mut(station).codec = composed;
    }

    /// Clears a station whose stream couldn't open, rather than leave a play button that would
    /// fail the same way.
    pub fn build_station_failed_actions(&mut self, generation: u64) -> Vec<PlayerAction> {
        if !self.is_current_station_session(generation) {
            return vec![];
        }
        self.source = None;
        self.status = PlaybackStatus::Stopped;
        vec![PlayerAction::Stop { fade_ms: 0 }]
    }

    /// An open takes seconds and a click none, so anything arriving off one asks.
    fn is_current_station_session(&self, generation: u64) -> bool {
        self.is_radio() && self.radio_generation == generation
    }

    /// Refuses whatever is still connecting for the current session. Called by every transition
    /// that starts or ends one.
    pub(super) fn end_stream_session(&mut self) {
        self.radio_generation = self.radio_generation.wrapping_add(1);
        if let Some(station) = self.station_mut() {
            Arc::make_mut(station).buffering = false;
        }
    }
}
