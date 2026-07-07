//! `RecentlyPlayed.play-track` — the Most Played strip-card play action.
//! See [`super::wire_recently_played`].

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_logged;
use crate::{AppWindow, RecentlyPlayed};

/// Wire the Most Played `play-track` callback. Clicking a card mirrors the
/// tracklist double-click: `queue_append_unique` skip-to's the track if already
/// queued, else appends to the tail and skip-to's the new slot.
pub(super) fn wire(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<RecentlyPlayed>();
    let s = state.clone();
    g.on_play_track(move |id| {
        let s = s.clone();
        let id = i64::from(id);
        spawn_logged!(s, "recently_played::play_track",
            library::queue::queue_append_unique(&s, id));
    });
}
