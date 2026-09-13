//! The two MPRIS2 interfaces, answered from the shared [`Published`] snapshot.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, Value};

use melodia_engine::player::engine::event_sink::PlayerEvent;
use melodia_engine::player::engine::state::volume_to_amplitude;
use melodia_engine::player::engine::types::{PlaybackStatus, RepeatMode};

use crate::services::integrations::media_controls::published::Published;
use crate::services::integrations::media_controls::{cover_url, forward, volume_percent};

/// One id for every source, so `SetPosition`'s stale-id check only turns away an id this player
/// never published.
const TRACK_ID: ObjectPath<'static> = ObjectPath::from_static_str_unchecked("/");

/// `org.mpris.MediaPlayer2`.
pub(super) struct Root;

#[expect(clippy::unused_self, reason = "zbus dispatches every member through a receiver")]
#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        log::debug!("MPRIS Raise ignored");
    }

    fn quit(&self) {
        log::debug!("MPRIS Quit ignored");
    }

    /// False, and `CanRaise` beside it: window control needs a Slint handle this layer
    /// deliberately doesn't hold, so a client offering either would offer a dead button.
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_tracklist(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &'static str {
        "Melodia"
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// `org.mpris.MediaPlayer2.Player`.
pub(super) struct Player {
    pub(super) published: Arc<Mutex<Published>>,
    pub(super) events: mpsc::Sender<PlayerEvent>,
}

#[expect(clippy::unused_self, reason = "zbus dispatches every member through a receiver")]
#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        forward(&self.events, PlayerEvent::Next);
    }

    fn previous(&self) {
        forward(&self.events, PlayerEvent::Previous);
    }

    fn pause(&self) {
        forward(&self.events, PlayerEvent::Pause);
    }

    fn play_pause(&self) {
        forward(&self.events, PlayerEvent::PlayPause);
    }

    fn stop(&self) {
        forward(&self.events, PlayerEvent::Stop);
    }

    fn play(&self) {
        forward(&self.events, PlayerEvent::Play);
    }

    fn seek(&self, offset: i64) {
        log::debug!("MPRIS relative seek by {offset} µs ignored (needs library API support)");
    }

    #[expect(clippy::needless_pass_by_value, reason = "zbus deserializes arguments by value")]
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        if track_id != TRACK_ID {
            return;
        }
        let length_ms = self.published.lock().metadata.as_ref().and_then(|m| m.duration_ms);
        if let Some(target_ms) = seek_target(position, length_ms) {
            forward(&self.events, PlayerEvent::SeekTo(target_ms));
        }
    }

    fn open_uri(&self, uri: &str) {
        log::debug!("MPRIS OpenUri requested (ignored): {uri}");
    }

    #[zbus(signal)]
    pub(super) async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        match self.published.lock().status {
            Some(PlaybackStatus::Playing) => "Playing",
            Some(PlaybackStatus::Paused) => "Paused",
            Some(PlaybackStatus::Stopped | PlaybackStatus::Loading) | None => "Stopped",
        }
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<&'static str, Value<'static>> {
        let published = self.published.lock();
        let mut dict = HashMap::with_capacity(6);
        dict.insert("mpris:trackid", Value::new(TRACK_ID));
        let Some(metadata) = published.metadata.as_ref() else {
            return dict;
        };

        // Absent rather than zero for a live source: clients render the two differently, and a
        // stream has no length to seek within.
        if let Some(duration_ms) = metadata.duration_ms {
            dict.insert("mpris:length", Value::new(micros(duration_ms)));
        }
        if let Some(artwork_path) = metadata.artwork_path.as_deref() {
            dict.insert("mpris:artUrl", Value::new(cover_url(artwork_path)));
        }
        dict.insert("xesam:title", Value::new(metadata.title.clone()));
        if let Some(secondary) = metadata.secondary.as_ref() {
            dict.insert("xesam:artist", Value::new(vec![secondary.clone()]));
        }
        if let Some(album) = metadata.album.as_ref() {
            dict.insert("xesam:album", Value::new(album.clone()));
        }
        dict
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        let published = self.published.lock();
        volume_to_amplitude(published.volume, published.is_muted)
    }

    /// Records the level before the player applies it. zbus answers a set with the property at
    /// once, and answering with the old level jumps back the slider a client is dragging. The
    /// player unmutes on any set, so this is the state its own sync will find.
    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        let percent = volume_percent(volume);
        {
            let mut published = self.published.lock();
            published.volume = percent;
            published.is_muted = false;
        }
        forward(&self.events, PlayerEvent::SetVolume(percent));
    }

    #[zbus(property)]
    fn loop_status(&self) -> &'static str {
        as_loop_status(self.published.lock().repeat_mode.unwrap_or(RepeatMode::Off))
    }

    /// Recorded before the player applies it, for `set_volume`'s reason.
    #[zbus(property)]
    fn set_loop_status(&self, loop_status: &str) -> zbus::fdo::Result<()> {
        let mode = parse_loop_status(loop_status).ok_or_else(|| {
            zbus::fdo::Error::InvalidArgs(format!("unknown LoopStatus {loop_status:?}"))
        })?;
        self.published.lock().repeat_mode = Some(mode);
        forward(&self.events, PlayerEvent::SetRepeat(mode));
        Ok(())
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.published.lock().shuffle_enabled
    }

    /// Recorded before the player applies it, for `set_volume`'s reason. An empty queue refuses
    /// to shuffle, and the sync that follows puts the answer back.
    #[zbus(property)]
    fn set_shuffle(&self, shuffle: bool) {
        self.published.lock().shuffle_enabled = shuffle;
        forward(&self.events, PlayerEvent::SetShuffle(shuffle));
    }

    /// Read on request and never announced, as the spec has it: clients extrapolate from `Rate`
    /// between reads and hear about jumps through `Seeked`.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        let published = self.published.lock();
        match published.status {
            Some(PlaybackStatus::Playing | PlaybackStatus::Paused) => micros(published.position_ms),
            Some(PlaybackStatus::Stopped | PlaybackStatus::Loading) | None => 0,
        }
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// MPRIS time, microseconds in a signed field, for a millisecond position.
pub(super) fn micros(ms: u64) -> i64 {
    i64::try_from(ms.saturating_mul(1000)).unwrap_or(i64::MAX)
}

/// The spec's name for a repeat mode.
fn as_loop_status(mode: RepeatMode) -> &'static str {
    match mode {
        RepeatMode::Off => "None",
        RepeatMode::All => "Playlist",
        RepeatMode::One => "Track",
    }
}

fn parse_loop_status(loop_status: &str) -> Option<RepeatMode> {
    match loop_status {
        "None" => Some(RepeatMode::Off),
        "Playlist" => Some(RepeatMode::All),
        "Track" => Some(RepeatMode::One),
        _ => None,
    }
}

/// Where a `SetPosition` lands, or `None` where the spec says to ignore it: a negative position,
/// or one past the end of the track.
fn seek_target(position_us: i64, length_ms: Option<u64>) -> Option<u64> {
    let target_ms = u64::try_from(position_us).ok()? / 1000;
    match length_ms {
        Some(length_ms) if target_ms > length_ms => None,
        _ => Some(target_ms),
    }
}

#[cfg(test)]
#[path = "tests/interface_tests.rs"]
mod tests;
