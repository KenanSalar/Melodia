//! The row actions all three tabs share.
//!
//! **One door per action, taking the whole row rather than an id**, because the two kinds of
//! station identify themselves differently: a browsed one has no database row and answers to its
//! uuid, a kept one has an id and no place in the browse cache. `id == 0` is the split, and it is
//! spelled once here rather than at every mount.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::entities::radio::DirectoryStation;
use crate::library;
use crate::state::AppState;
use crate::ui::launcher;
use crate::ui::radio::{RadioUi, browse, kept};
use crate::{AppWindow, Radio, RadioStationRow};

/// Whether a row names a station that already has a database row.
fn is_kept(row: &RadioStationRow) -> bool {
    row.id != 0
}

pub(super) fn wire(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let weak = ui.as_weak();

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_play_station(move |row| {
            if is_kept(&row) {
                play_kept(&s, &ru, &weak, i64::from(row.id));
                return;
            }
            let Some((station, logo)) = browse::resolve(&ru, &row.uuid) else {
                return;
            };
            play_browsed(&s, &ru, &weak, station, logo);
        });
    }

    {
        let s = state.clone();
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_toggle_favorite(move |row| {
            if is_kept(&row) {
                toggle_kept(&s, &ru, &weak, i64::from(row.id), !row.is_favorite);
                return;
            }
            toggle_browsed(&s, &ru, &weak, &row);
        });
    }

    {
        let s = state.clone();
        g.on_open_homepage(move |url| {
            if url.is_empty() {
                return;
            }
            s.runtime.spawn(launcher::open_target(url.to_string(), "radio::open_homepage"));
        });
    }
}

/// Tune to a station that already has a row.
fn play_kept(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &slint::Weak<AppWindow>, id: i64) {
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        if let Err(e) = library::radio::play_station(&s, id).await {
            log::warn!("radio::play_station: {}", crate::services::describe(&e));
        }
        refresh_lists(&s, &ru, &weak);
    });
}

/// Tune to a station that is still only a directory answer, keeping it on the way.
fn play_browsed(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &slint::Weak<AppWindow>,
    station: DirectoryStation,
    logo: Option<String>,
) {
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        if let Err(e) = library::radio::play_directory_station(&s, &station, logo.as_deref()).await
        {
            log::warn!("radio::play_station: {}", crate::services::describe(&e));
        }
        refresh_lists(&s, &ru, &weak);
    });
}

/// Re-read the kept lists after any action that moved a row, whichever door it came through.
///
/// **Every action here changes the table and none of it is derivable**: a play stamps
/// `last_played` and bumps `play_count`, a browsed play writes the row itself first, and a star
/// moves list membership. A tab pick paints from cache, so without this a station played from
/// Browse is missing from Recently Played until the next section leave and return.
///
/// A failed play refreshes too, deliberately: the play is counted before the stream is opened,
/// because the recents list records what the user chose rather than what the network allowed.
fn refresh_lists(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &slint::Weak<AppWindow>) {
    let (s, ru) = (state.clone(), radio_ui.clone());
    let _ = weak.upgrade_in_event_loop(move |ui| kept::refresh(&ui, &s, &ru));
}

/// Star or un-star a station that already has a row.
///
/// Not optimistic, unlike the browsed toggle below: un-starring drops the row out of the Favorites
/// list entirely, so there is nothing on screen for an optimistic flip to be right about — the
/// refetch *is* the update.
///
/// **Un-starring goes through the removal door, not the flag.** The star and the trash leave a
/// station in the same place, so they owe the same cleanup: a row neither tab would list is one
/// nothing can reach, and `set_favorite` alone leaves it there for good.
fn toggle_kept(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &slint::Weak<AppWindow>,
    id: i64,
    favorite: bool,
) {
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        let flipped = if favorite {
            library::radio::set_favorite(&s, id, true).await
        } else {
            library::radio::remove_from_favorites(&s, id).await
        };
        if let Err(e) = flipped {
            log::warn!("radio: favorite toggle failed: {}", crate::services::describe(&e));
            return;
        }
        refresh_lists(&s, &ru, &weak);
    });
}

/// Keep or release a station that only exists in the directory answer on screen.
///
/// Optimistic, like every other row flag in the tree: a star is not list membership on Browse, so
/// nothing has to be re-fetched for it to be right, and the star has to answer on the click's own
/// frame.
fn toggle_browsed(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &slint::Weak<AppWindow>,
    row: &RadioStationRow,
) {
    let Some(ui) = weak.upgrade() else { return };
    let Some((station, logo)) = browse::resolve(radio_ui, &row.uuid) else {
        return;
    };
    let uuid = row.uuid.to_string();
    let wanted = !row.is_favorite;

    radio_ui.set_local_favorite(&uuid, wanted);
    browse::apply(&ui, radio_ui);

    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        // The write is unconditional so the facade has a row to resolve, and un-starring then
        // takes the cleanup the trash takes: a station released here without a play behind it is
        // listed by neither tab, and leaving it costs a row per browse-and-unstar forever.
        let flipped =
            match library::radio::set_directory_favorite(&s, &station, wanted, logo.as_deref())
                .await
            {
                Ok(id) if !wanted => library::radio::delete_if_unlisted(&s, id).await,
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            };
        if let Err(e) = flipped {
            log::warn!("radio: favorite toggle failed: {}", crate::services::describe(&e));
            // Put the star back rather than leaving it claiming a row that was never written. A
            // routine failure, so it is a log line and not a toast.
            ru.set_local_favorite(&uuid, !wanted);
            let _ = weak.upgrade_in_event_loop(move |ui| browse::apply(&ui, &ru));
            return;
        }
        // The kept list gained or lost a station, and Browse's own stars come off the same fetch.
        refresh_lists(&s, &ru, &weak);
    });
}
