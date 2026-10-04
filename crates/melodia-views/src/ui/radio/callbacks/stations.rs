//! The row actions all three tabs share.
//!
//! **One door per action, taking the whole row rather than an id**, because the two kinds of
//! station identify themselves differently: a browsed one has no database row and answers to its
//! uuid, a kept one has an id and no place in the browse cache. `id == 0` is the split, and it is
//! spelled once here rather than at every mount.

use std::sync::Arc;

use slint::{ComponentHandle, Model};

use crate::ui::radio::{RadioUi, browse, kept, tab_from_index};
use crate::ui::{clipboard, launcher};
use melodia_app::library;
use melodia_app::library::clipboard::{StationField, StationText, station_lines};
use melodia_app::state::AppState;
use melodia_core::entities::radio::{DirectoryStation, RadioStation};
use melodia_core::error::AppError;
use melodia_ui::{AppWindow, Radio, RadioStationRow};

/// Whether a row names a station that already has a database row.
fn is_kept(row: &RadioStationRow) -> bool {
    super::super::station_has_row(i64::from(row.id))
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
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_set_favorites(move |tab, ids, favorite| {
            let Some(ui) = weak.upgrade() else { return };
            let tab = tab_from_index(&ui.global::<Radio>(), tab);
            // A hand-typed station's card has no star, so the set's star skips it too: un-starring
            // one drops it from the only list that shows it.
            let ids: Vec<i64> = ids
                .iter()
                .map(i64::from)
                .filter(|&id| {
                    kept::resolve(&ru, tab, id)
                        .is_some_and(|station| station.station_uuid.is_some())
                })
                .collect();
            set_kept_favorites(&s, &ru, &weak, ids, favorite);
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

    {
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_copy_kept_stations(move |tab, field, ids| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(field) = StationField::from_token(&field) else {
                log::warn!("radio copy: unknown field {field}");
                return;
            };
            let tab = tab_from_index(&ui.global::<Radio>(), tab);
            let stations: Vec<RadioStation> =
                ids.iter().filter_map(|id| kept::resolve(&ru, tab, i64::from(id))).collect();
            let text = station_lines(stations.iter().map(kept_text), field);
            clipboard::write(&ui, &text);
        });
    }

    {
        let ru = radio_ui.clone();
        let weak = weak.clone();
        g.on_copy_browse_station(move |field, uuid| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(field) = StationField::from_token(&field) else {
                log::warn!("radio copy: unknown field {field}");
                return;
            };
            let Some((station, _logo)) = browse::resolve(&ru, &uuid) else { return };
            let text = station_lines([browsed_text(&station)], field);
            clipboard::write(&ui, &text);
        });
    }
}

/// What a kept station offers a Copy entry, its website read through the user's own first.
fn kept_text(station: &RadioStation) -> StationText<'_> {
    StationText { name: &station.name, stream_url: &station.stream_url, website: station.website() }
}

fn browsed_text(station: &DirectoryStation) -> StationText<'_> {
    StationText {
        name: &station.name,
        stream_url: &station.stream_url,
        website: station.homepage.as_deref(),
    }
}

/// Tune to a station that already has a row.
fn play_kept(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &slint::Weak<AppWindow>, id: i64) {
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        if let Err(e) = library::radio::play_station(&s, id).await {
            log::warn!("radio::play_station: {}", melodia_core::error::describe(&e));
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
            log::warn!("radio::play_station: {}", melodia_core::error::describe(&e));
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
fn toggle_kept(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &slint::Weak<AppWindow>,
    id: i64,
    favorite: bool,
) {
    set_kept_favorites(state, radio_ui, weak, vec![id], favorite);
}

/// The star over every kept station in `ids`, then one re-read of both lists. A station that fails
/// leaves the rest to go, each being its own row.
fn set_kept_favorites(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &slint::Weak<AppWindow>,
    ids: Vec<i64>,
    favorite: bool,
) {
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.spawn(async move {
        for id in ids {
            if let Err(e) = set_kept_favorite(&s, id, favorite).await {
                log::warn!("radio: favorite toggle failed: {}", melodia_core::error::describe(&e));
            }
        }
        refresh_lists(&s, &ru, &weak);
    });
}

/// **Un-starring goes through the removal door, not the flag.** The star and the trash leave a
/// station in the same place, so they owe the same cleanup: a row neither tab would list is one
/// nothing can reach, and `set_favorite` alone leaves it there for good.
async fn set_kept_favorite(state: &AppState, id: i64, favorite: bool) -> Result<(), AppError> {
    if favorite {
        library::radio::set_favorite(state, id, true).await
    } else {
        library::radio::remove_from_favorites(state, id).await
    }
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
            log::warn!("radio: favorite toggle failed: {}", melodia_core::error::describe(&e));
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
