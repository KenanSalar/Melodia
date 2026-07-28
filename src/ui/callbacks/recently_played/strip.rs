//! `RecentlyPlayed.play-track` — the Most Played strip-card play action.
//! See [`super::wire_recently_played`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::recently_played::RecentlyPlayedUi;
use crate::{AppWindow, RecentlyPlayed};

/// Wire the Most Played `play-track` callback. Clicking a card loads the strip
/// into the queue and starts on that card — the strip is the context, not the
/// recency list below it. The callback carries no row index (these are cards,
/// not list rows), so the start slot comes from the id.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let s = state.clone();
    let ru = rp_ui.clone();
    g.on_play_track(move |id| {
        let id = i64::from(id);
        let ids = ru.most_played_track_ids();
        let start = ids.iter().position(|&i| i == id);
        if ids.is_empty() {
            return;
        }
        let s = s.clone();
        spawn_logged!(s, "recently_played::play_track",
            library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
    });
}
