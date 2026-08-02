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
use crate::state::AppState;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::persisted_sort;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, Favorites};

/// Nav-sidebar index of the Favorites tab. Used by the cross-tab
/// open-artist hand-off to stamp `ArtistDetail.origin-nav-index` so the
/// back arrow returns here, and by the lifecycle module to seed the
/// section-active shadow. Mirrors the `NAV_*` consts in
/// `callbacks/cross_tab_nav.rs` — same convention: kept local because
/// Slint globals can't expose `const`s that Rust reads ergonomically.
pub(super) const NAV_FAVORITES: i32 = 2;

/// Wire every `Favorites.*` callback. Call once after
/// `favorites::install_favorites_models` and after the Artists UI handle
/// exists (the sub-view module borrows it for the cross-tab open-artist
/// hand-off).
pub fn wire_favorites(
    ui: &AppWindow,
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    hydrate_sort_from_settings(state, fav_ui, &ui.global::<Favorites>());

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
fn hydrate_sort_from_settings(state: &AppState, fav_ui: &FavoritesUi, g: &Favorites<'_>) {
    if let Some((field, dir)) = persisted_sort(state, view_id::FAVORITES) {
        g.set_sort_field(SharedString::from(field.as_str()));
        g.set_sort_dir(SharedString::from(dir));
        favorites_ui_mod::set_sort(fav_ui, field, SortDir::from_token(dir));
    }

    // Sorts an empty cache — the fetch hasn't run yet — but going through the
    // one setter is what keeps "shadow and rows move together" unconditional.
    if let Some((field, dir)) = persisted_sort(state, view_id::FAVORITE_ARTISTS) {
        g.set_artist_sort_field(SharedString::from(field.as_str()));
        g.set_artist_sort_dir(SharedString::from(dir));
        favorites_ui_mod::set_artist_sort(fav_ui, field, SortDir::from_token(dir));
    }
}
