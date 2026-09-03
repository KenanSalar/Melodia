//! `SlintEventSink` translates OS media-control events (souvlaki) into
//! `library::*` calls on the tokio runtime. It does **not** import Slint —
//! the name reflects "where it ships to" (the UI binary), not what it
//! depends on. Keeping souvlaki decoupled from Slint matters for the
//! `EventSink` trait contract in `services::integrations::media_controls`.

use crate::library;
use crate::player::event_sink::{EventSink, PlayerEvent};
use crate::state::AppState;

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
                    let r = library::playback::player_set_volume(&ctx, v);
                    // OS media controls are single discrete events; commit
                    // settings.json inline (no slider-style drag thrashing).
                    if r.is_ok() {
                        library::playback::commit_player_settings(&ctx).await
                    } else {
                        r
                    }
                }
            };
            if let Err(e) = r {
                log::warn!("souvlaki -> library error: {e}");
            }
        });
    }
}
