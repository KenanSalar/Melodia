//! `Favorites.*` callbacks, split by concern:
//!
//! * [`covers`] — the lazy cover-lookup callbacks for the three card tiers.
//! * [`hero`] — the per-tab shuffle pills.
//! * [`subviews`] — the grid cards' actions, the cross-tab open-artist
//!   hand-off, the tab switch, and the grid column-count push.
//! * [`tracklist`] — the Songs tab: row actions, filter, sort, column
//!   visibility, and modifier-aware selection.
//! * [`lifecycle`] — section enter/leave cache management + the
//!   `library_changed` re-fetch subscriber.
//!
//! Cross-tab back-nav: clicking a favorite artist card sets
//! `ArtistDetail.origin-nav-index = NAV_FAVORITES` synchronously *before*
//! the async fetch yields, then `artists_ui.open_artist_with` flips
//! `Nav.selected-index` in the same `upgrade_in_event_loop` closure that
//! writes `artist-id`. The back arrow on `ArtistDetail` reads the origin
//! and flips Nav back here.

mod covers;
mod hero;
mod lifecycle;
mod subviews;
mod tracklist;

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::services::settings::SortDir;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::persisted_sort;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, Favorites};

/// Wire every `Favorites.*` callback.
///
/// Called by [`super::install`], which is what guarantees the models are in
/// place first and that the Artists handle exists (the sub-view module borrows
/// it for the cross-tab open-artist hand-off). `wire_all` still has to have run
/// before it.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    fav_ui: &Arc<FavoritesUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    hydrate_sort_from_settings(view_state, fav_ui, &ui.global::<Favorites>());

    covers::wire(ui, fav_ui);
    hero::wire(ui, state, fav_ui);
    subviews::wire(ui, state, fav_ui, artists_ui);
    tracklist::wire(ui, state, fav_ui);
    lifecycle::wire(ui, state, fav_ui);
}

/// Read the two persisted sorts from settings and seed both the Rust cache
/// and the Slint properties. `None` (never persisted) leaves the
/// defaults in place.
///
/// Two, because the page has two independently sortable sub-views: the Songs
/// tab's `TrackList` and the Favorite Artists grid. They share the `view_sort`
/// map under separate keys.
fn hydrate_sort_from_settings(
    view_state: Option<&ViewStateData>,
    fav_ui: &FavoritesUi,
    g: &Favorites<'_>,
) {
    if let Some((field, dir)) = persisted_sort(view_state, view_id::FAVORITES) {
        g.set_sort_field(SharedString::from(field.as_str()));
        g.set_sort_dir(SharedString::from(dir));
        favorites_ui_mod::set_sort(fav_ui, field, SortDir::from_token(dir));
    }

    // Sorts an empty cache — the fetch hasn't run yet — but going through the
    // one setter is what keeps "shadow and rows move together" unconditional.
    if let Some((field, dir)) = persisted_sort(view_state, view_id::FAVORITE_ARTISTS) {
        g.set_artist_sort_field(SharedString::from(field.as_str()));
        g.set_artist_sort_dir(SharedString::from(dir));
        favorites_ui_mod::set_artist_sort(fav_ui, field, SortDir::from_token(dir));
    }
}
