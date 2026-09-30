//! `SlintEventSink` translates OS media-control events into
//! `library::*` calls on the tokio runtime. It does **not** import Slint —
//! the name reflects "where it ships to" (the UI binary), not what it
//! depends on. Keeping the media controls decoupled from Slint matters for the
//! `EventSink` trait contract in `services::integrations::media_controls`.

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_engine::player::engine::event_sink::{EventSink, PlayerEvent};

pub struct SlintEventSink {
    pub state: AppState,
}

impl EventSink for SlintEventSink {
    fn handle(&self, ev: PlayerEvent) {
        let s = self.state.clone();
        self.state.runtime.spawn(async move {
            let ctx = s.playback_ctx();
            let r = match ev {
                PlayerEvent::Play => library::playback::player_play(&ctx),
                PlayerEvent::Pause => library::playback::player_pause(&ctx),
                PlayerEvent::PlayPause => library::playback::player_toggle_play_pause(&ctx),
                PlayerEvent::Next => library::playback::player_next(&ctx),
                PlayerEvent::Previous => library::playback::player_previous(&ctx),
                PlayerEvent::Stop => library::playback::player_stop(&ctx),
                PlayerEvent::SeekTo(ms) => library::playback::player_seek(&ctx, ms),
                PlayerEvent::SetVolume(v) => {
                    library::playback::player_set_volume_committed(&ctx, v).await
                }
                PlayerEvent::SetShuffle(enabled) => library::queue::queue_set_shuffle(&s, enabled),
                PlayerEvent::SetRepeat(mode) => library::queue::queue_set_repeat(&s, mode),
                // Applied and then persisted, the two steps the speed control's own callback takes.
                PlayerEvent::SetSpeed(speed) => {
                    let applied = library::playback::player_set_playback_speed(&ctx, speed);
                    s.persist_blocking("persist playback_speed", move |st| {
                        library::settings::set_playback_speed(st, speed)
                    });
                    applied
                }
            };
            if let Err(e) = r {
                log::warn!("media controls -> library error: {e}");
            }
        });
    }
}
