//! The album/artist/genre ids behind a row menu's "Go to …" entries, as the
//! Slint side spells them, and the subscriber keeping the playing track's on
//! `Player` for the now-playing bar's menu.
//!
//! `TrackSummary` carries none of the three (`queue_sheet::links` argues why),
//! so both hosts without a list row read them beside what they show: the queue
//! sheet per row, the bar once per track change.

use async_compat::Compat;
use slint::{ComponentHandle, Weak};

use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackLinks;
use melodia_core::error::describe;
use melodia_engine::player::engine::state::PlayerViewModelLight;
use melodia_ui::{AppWindow, Player, QueueRow};

/// The FK trio as the Slint row spells it: 0 for absent, so the menu's
/// `album-id != 0` gate reads a missing linkage and a not-yet-fetched one the
/// same way. That is the honest reading — until the fetch lands, we don't know.
#[derive(Clone, Copy, Default)]
pub(in crate::ui) struct RowLinks {
    pub album: i32,
    pub artist: i32,
    pub genre: i32,
}

impl RowLinks {
    /// Write the trio onto a row, and say whether anything moved — which is what
    /// lets the patch skip a `set_row_data` that would invalidate the
    /// `ListView`'s delegate cache for nothing.
    pub(in crate::ui) fn stamp(self, row: &mut QueueRow) -> bool {
        if row.album_id == self.album && row.artist_id == self.artist && row.genre_id == self.genre
        {
            return false;
        }
        row.album_id = self.album;
        row.artist_id = self.artist;
        row.genre_id = self.genre;
        true
    }
}

impl From<TrackLinks> for RowLinks {
    fn from(l: TrackLinks) -> Self {
        Self {
            album: to_row_id(l.album_id),
            artist: to_row_id(l.artist_id),
            genre: to_row_id(l.genre_id),
        }
    }
}

fn to_row_id(id: Option<i64>) -> i32 {
    id.and_then(|v| i32::try_from(v).ok()).unwrap_or(0)
}

/// Keep `Player.track-{album,artist,genre}-id` naming the playing track's.
///
/// Do-while over the view model, so the track a launch restores is read with no
/// seed of its own. The outgoing track's ids are cleared before the read, so a
/// menu open across a track change never offers them for the new one, and a read
/// overtaken by another change is dropped: the `changed()` below then wakes at
/// once for the newer track.
pub fn spawn_playing_links_subscriber(
    ui: Weak<AppWindow>,
    state: &AppState,
) -> Result<(), slint::EventLoopError> {
    let state = state.clone();
    let mut rx = state.sinks.view_model.subscribe();
    slint::spawn_local(Compat::new(async move {
        let mut shown = None;
        loop {
            let playing = playing_track_id(rx.borrow_and_update().as_ref());
            if playing != shown {
                shown = playing;
                publish(&ui, RowLinks::default());
                if let Some(id) = playing {
                    let links = read_links(&state, id).await;
                    if playing_track_id(rx.borrow().as_ref()) == playing {
                        publish(&ui, links);
                    }
                }
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
        log::debug!("ui::track_links playing-track subscriber stopped");
    }))?;
    Ok(())
}

fn playing_track_id(vm: Option<&PlayerViewModelLight>) -> Option<i64> {
    vm?.current_track.as_ref().map(|t| t.id)
}

/// Awaited inline, sqlx having a reactor under `Compat`. A failed read or a row
/// gone since is the same unlinked trio, which hides the entries.
async fn read_links(state: &AppState, id: i64) -> RowLinks {
    match library::tracks::get_track_links(&state.db, &[id]).await {
        Ok(links) => links.into_iter().next().map(RowLinks::from).unwrap_or_default(),
        Err(e) => {
            log::warn!("ui::track_links get_track_links({id}): {}", describe(&e));
            RowLinks::default()
        }
    }
}

fn publish(ui: &Weak<AppWindow>, links: RowLinks) {
    let Some(ui) = ui.upgrade() else { return };
    let player = ui.global::<Player>();
    player.set_track_album_id(links.album);
    player.set_track_artist_id(links.artist);
    player.set_track_genre_id(links.genre);
}

#[cfg(test)]
#[path = "tests/track_links_tests.rs"]
mod tests;
