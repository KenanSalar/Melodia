//! The callbacks all nine track lists wire alike, written once over [`TrackListActions`]: the
//! five list views and the four details through [`super::track_detail`].

use slint::{ModelRc, StrongHandle, Weak};

use super::macros::spawn_logged;
use super::play_row_start;
use crate::ui::track_list_view::{TrackListActions, TrackListColumnState, persist_visible};
use melodia_app::library;
use melodia_app::state::AppState;

/// Play Next and Add to Queue, the row menu's two entries that append the ids they are handed.
/// `collect` is where a list holding rows that aren't in the library drops them.
pub(in crate::ui) fn wire_queue_actions<G: TrackListActions + TrackListColumnState>(
    g: &G,
    state: &AppState,
    collect: fn(&ModelRc<i32>) -> Vec<i64>,
) {
    {
        let s = state.clone();
        g.bind_play_next(move |ids| {
            let id_vec = collect(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(
                s,
                format_args!("{}::play_next", G::VIEW_ID),
                library::queue::queue_play_next_many(&s, id_vec)
            );
        });
    }
    {
        let s = state.clone();
        g.bind_add_to_queue(move |ids| {
            let id_vec = collect(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(
                s,
                format_args!("{}::add_to_queue", G::VIEW_ID),
                library::queue::queue_add_tracks(&s, id_vec)
            );
        });
    }
}

/// The column popup has already flipped its `show-*` flag, so a toggle only persists.
pub(in crate::ui) fn wire_toggle_column<G>(weak: &Weak<G>, state: &AppState)
where
    G: TrackListActions + TrackListColumnState + StrongHandle + 'static,
{
    let Some(g) = weak.upgrade() else { return };
    let s = state.clone();
    let weak = weak.clone();
    g.bind_toggle_column(move |_id| {
        let Some(g) = weak.upgrade() else { return };
        persist_visible(&s, &g);
    });
}

/// A row activation: replace the queue with the view's displayed `ids` and start on the row that
/// was activated.
pub(in crate::ui) fn play_displayed(
    state: &AppState,
    view_id: &'static str,
    ids: Vec<i64>,
    track_id: i32,
    idx: i32,
) {
    if ids.is_empty() {
        return;
    }
    let start = play_row_start(&ids, i64::from(track_id), idx);
    let s = state.clone();
    spawn_logged!(
        s,
        format_args!("{view_id}::play_row"),
        library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
    );
}
