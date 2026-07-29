//! `RecentlyPlayed.*` callbacks, split by concern:
//!
//! * [`covers`] — the lazy Most Played cover-lookup callback.
//! * [`strip`] — the Most Played strip-card play action.
//! * [`tracklist`] — the recency list: row actions, filter, in-memory sort,
//!   column visibility, modifier-aware selection, and the header Shuffle
//!   pill.
//! * [`lifecycle`] — section enter/leave cache management + the joined
//!   `library_changed` + `stats_changed` re-fetch subscriber.

mod covers;
mod lifecycle;
mod strip;
mod tracklist;

use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::library;
use crate::services::settings::SortDir;
use crate::state::AppState;
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::{AppWindow, RecentlyPlayed};

/// Nav-sidebar index of the Recently-Played tab. Used by the lifecycle module
/// to seed the section-active shadow. Mirrors the `NAV_*` convention in
/// `callbacks/cross_tab_nav.rs` — kept local because Slint globals can't expose
/// `const`s that Rust reads ergonomically.
pub(super) const NAV_RECENTLY_PLAYED: i32 = 8;

/// The settings view-id under which this view's sort + column state persist.
const VIEW_ID: &str = crate::ui::track_list_view::view_id::RECENTLY_PLAYED;

/// Wire every `RecentlyPlayed.*` callback. Call once after
/// `recently_played::install_recently_played_models`.
pub fn wire_recently_played(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    hydrate_sort_from_settings(state, rp_ui, &ui.global::<RecentlyPlayed>());

    covers::wire(ui, rp_ui);
    strip::wire(ui, state, rp_ui);
    tracklist::wire(ui, state, rp_ui);
    lifecycle::wire(ui, state, rp_ui);
}

/// Read the persisted sort from settings and seed both the Rust cache and the
/// Slint properties. `None` (never persisted) leaves the recency default in
/// place.
fn hydrate_sort_from_settings(
    state: &AppState,
    rp_ui: &RecentlyPlayedUi,
    g: &RecentlyPlayed<'_>,
) {
    let Some(sort) = library::settings::get_view_sort(state, VIEW_ID) else {
        return;
    };
    g.set_sort_field(SharedString::from(sort.field.as_str()));
    g.set_sort_dir(SharedString::from(match sort.dir {
        SortDir::Asc => "asc",
        SortDir::Desc => "desc",
    }));
    recently_played_ui_mod::set_sort(rp_ui, sort.field, sort.dir);
}
