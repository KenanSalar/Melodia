//! The Songs list's keyset loads, and the row patch a favourite or rating takes.
//!
//! Every load takes its ticket on the thread that read its sort and filter, before it spawns, so
//! the last one asked for is the one shown whichever query lands first. A sort or a keystroke
//! asked for while a library tick's refresh is still owed runs as that refresh.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::TracksUi;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::list_selection;
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::paged_rows::{Keyset, Load, Ticket};
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder};
use melodia_core::error::AppResult;
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

/// The sort and the filter a load runs under, captured when it is asked for.
pub struct SongsQuery {
    order: TrackOrder,
    needle: Needle,
}

impl SongsQuery {
    pub fn new(sort_field: &str, sort_dir: &str, filter: &str) -> Self {
        Self {
            order: TrackOrder::new(sort_field, SortDir::from_token(sort_dir)),
            needle: fold_needle(filter),
        }
    }
}

/// Reload the list: its ids, the rows it opens on with their covers warm, and every row it still
/// holds read again. What a launch, a re-enter and a library tick ask for.
pub fn refresh(
    state: &AppState,
    tracks_ui: &Arc<TracksUi>,
    weak: &Weak<AppWindow>,
    query: SongsQuery,
) {
    let ticket = tracks_ui.tickets.issue_reload();
    spawn_refresh(state, tracks_ui, weak, ticket, query);
}

/// Reload the ids under a new sort or filter. The rows the list holds are still current and the
/// library hasn't changed size, so it re-reads no row, warms no cover and leaves the count, unless
/// it overtakes a refresh, which it then runs in that one's place.
pub fn requery(
    state: &AppState,
    tracks_ui: &Arc<TracksUi>,
    weak: &Weak<AppWindow>,
    query: SongsQuery,
) {
    match tracks_ui.tickets.issue_requery() {
        Load::Requery(ticket) => {
            let s = state.clone();
            let tu = Arc::clone(tracks_ui);
            let weak = weak.clone();
            spawn_logged!(s, "tracks::requery", requery_now(&s, tu, weak, ticket, query));
        }
        Load::Reload(ticket) => spawn_refresh(state, tracks_ui, weak, ticket, query),
    }
}

fn spawn_refresh(
    state: &AppState,
    tracks_ui: &Arc<TracksUi>,
    weak: &Weak<AppWindow>,
    ticket: Ticket,
    query: SongsQuery,
) {
    let s = state.clone();
    let tu = Arc::clone(tracks_ui);
    let weak = weak.clone();
    spawn_logged!(s, "tracks::refresh", refresh_now(&s, tu, weak, ticket, query));
}

/// Patch the row showing `id` after a favourite or rating write. Only that row is read again, so
/// scroll position and its neighbours stay put.
pub fn patch_row(
    weak: &Weak<AppWindow>,
    id: i64,
    edit: impl FnOnce(&mut UiTrackListRow) + Send + 'static,
) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Tracks>().get_rows();
        if let Some(model) = TrackRows::installed(&rows, "Tracks.rows") {
            model.patch(id, edit);
        }
    });
}

async fn refresh_now(
    state: &AppState,
    tracks_ui: Arc<TracksUi>,
    weak: Weak<AppWindow>,
    ticket: Ticket,
    query: SongsQuery,
) -> AppResult<()> {
    let keyset = track_rows::load(
        state,
        TrackListScope::Library,
        query.order,
        &query.needle,
        &tracks_ui.scroll,
    )
    .await?;
    let total = library_total(state, &keyset, &query.needle).await?;
    // Overtaken already, so nothing this warms would be drawn.
    if !tracks_ui.tickets.is_latest(ticket) {
        return Ok(());
    }
    track_rows::prewarm(&tracks_ui.cover_thumbs, &keyset).await;

    let unfiltered = query.needle.is_empty();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if !tracks_ui.tickets.is_latest(ticket) {
            return;
        }
        let g = ui.global::<Tracks>();
        g.set_total_count(total);
        let rows = g.get_rows();
        let Some(model) = TrackRows::installed(&rows, "Tracks.rows") else {
            return;
        };
        model.show(keyset);
        model.reread();
        tracks_ui.tickets.reload_shown(ticket);
        // A filtered keyset no longer holds the rows the filter hides, and a filter drops no
        // pick, so only an unfiltered one can say which picks the library has lost.
        if unfiltered {
            list_selection::keep_selection_within(&g, || model.row_ids().into_iter().collect());
        }
    });
    Ok(())
}

async fn requery_now(
    state: &AppState,
    tracks_ui: Arc<TracksUi>,
    weak: Weak<AppWindow>,
    ticket: Ticket,
    query: SongsQuery,
) -> AppResult<()> {
    let keyset = track_rows::load(
        state,
        TrackListScope::Library,
        query.order,
        &query.needle,
        &tracks_ui.scroll,
    )
    .await?;
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if !tracks_ui.tickets.is_latest(ticket) {
            return;
        }
        let rows = ui.global::<Tracks>().get_rows();
        if let Some(model) = TrackRows::installed(&rows, "Tracks.rows") {
            model.show(keyset);
        }
    });
    Ok(())
}

/// The library's size, which the header states whatever the filter leaves.
async fn library_total(
    state: &AppState,
    keyset: &Keyset<UiTrackListRow>,
    needle: &Needle,
) -> AppResult<i32> {
    if needle.is_empty() {
        return Ok(len_as_i32(keyset.ids.len()));
    }
    let count = library::tracks::count(&state.db).await?;
    Ok(i32::try_from(count).unwrap_or(i32::MAX))
}
