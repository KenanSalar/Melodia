//! Seeding the UI from persisted state, and the fetches that fill it before the user arrives.

use std::sync::Arc;

use melodia_app::services;
use melodia_app::state::AppState;
use melodia_artwork::media::image;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, ArtistDetail, MiniPlayer, Nav, Player, Queue};
use melodia_views::ui;
use slint::ComponentHandle;

/// Shows the restored source in the player bar before the first view-model push.
pub fn seed_initial_view_model(
    app: &AppWindow,
    state: &AppState,
    cover_thumbs: &Arc<image::cover_thumbs::CoverThumbs>,
) {
    use melodia_engine::player::engine::state::lock_state;

    let s = lock_state(&state.player_state);
    let light = s.to_view_model_light();
    let qvm = s.to_queue_view_model();
    drop(s);

    let player = app.global::<Player>();
    player.set_vm(ui::shell::bridge::to_slint_player_vm(&light, cover_thumbs));
    ui::shell::bridge::show_position(&player, light.position_ms, light.duration_ms);
    player.set_queue(ui::shell::bridge::to_slint_queue_vm(&qvm));

    // The seed above always misses the still-empty row tier, and a paused restore owes no
    // view-model push to fill it. Both slots, an empty path being a no-op.
    ui::shell::bridge::warm_vm_cover(
        app.as_weak(),
        &state.runtime,
        cover_thumbs,
        light.current_track.as_ref().and_then(|t| t.artwork_path.clone()).unwrap_or_default(),
        ui::shell::bridge::VmCoverSlot::Track,
    );
    ui::shell::bridge::warm_vm_cover(
        app.as_weak(),
        &state.runtime,
        cover_thumbs,
        light.radio.as_ref().and_then(|r| r.artwork_path.clone()).unwrap_or_default(),
        ui::shell::bridge::VmCoverSlot::Station,
    );
}

/// Applies the persisted layout in `settings.json` and `views.json` to the Slint globals,
/// re-reading either one passed as `None`.
pub fn hydrate_ui_from_settings(
    app: &AppWindow,
    state: &AppState,
    cached_settings: Option<&services::settings::SettingsData>,
    cached_view_state: Option<&services::view_state::ViewStateData>,
) {
    let owned_view_state;
    let vs: &services::view_state::ViewStateData = if let Some(v) = cached_view_state {
        v
    } else {
        owned_view_state =
            services::view_state::read_view_state(&state.paths).unwrap_or_else(|e| {
                log::warn!("hydrate_ui_from_settings: read view state failed: {}", describe(&e));
                services::view_state::ViewStateData::default()
            });
        &owned_view_state
    };
    // Ahead of anything that can bail: the column defaults are Rust's, so a list left unhydrated
    // has no columns at all rather than the stock set.
    ui::track_list_view::hydrate_all(app, vs);

    let owned_settings;
    let settings: &services::settings::SettingsData = match cached_settings {
        Some(s) => s,
        None => match services::settings::read_settings(&state.paths) {
            Ok(s) => {
                owned_settings = s;
                &owned_settings
            }
            Err(e) => {
                log::warn!("hydrate_ui_from_settings: read settings failed: {}", describe(&e));
                return;
            }
        },
    };
    apply_sidebar_width(app, settings);
    apply_sidebar_collapsed(app, settings);
    apply_queue_panel(app, settings);
    apply_mini_backdrop(app, settings);
    app.global::<ArtistDetail>().set_albums_collapsed(vs.artist_albums_collapsed);
    ui::settings::settings_page::seed_category(app, vs.settings_category);
}

/// `Nav.sidebar-expanded-w` clamps the width where it is drawn, so no Rust-side clamp is needed.
fn apply_sidebar_width(app: &AppWindow, settings: &services::settings::SettingsData) {
    app.global::<Nav>().set_sidebar_width(logical_px(settings.sidebar_width));
}

fn apply_sidebar_collapsed(app: &AppWindow, settings: &services::settings::SettingsData) {
    app.global::<Nav>().set_sidebar_collapsed(settings.layout.sidebar_collapsed);
}

/// Clamped where drawn, as the sidebar's width is. An open panel shows itself: `queue-shown` took
/// its baseline at construction, so the first change pass reads this as an edge.
fn apply_queue_panel(app: &AppWindow, settings: &services::settings::SettingsData) {
    let queue = app.global::<Queue>();
    queue.set_open(settings.queue_panel.queue_panel_open);
    queue.set_panel_width(settings.queue_panel.queue_panel_width.map_or(0.0, logical_px));
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "sidebar and panel widths are small pixel counts; f32 precision is sufficient"
)]
fn logical_px(width: f64) -> f32 {
    width as f32
}

/// Ordinary hydration, unlike `apply_backdrop_style`: this switch only mounts a stack the artwork
/// tiers already fit.
fn apply_mini_backdrop(app: &AppWindow, settings: &services::settings::SettingsData) {
    app.global::<MiniPlayer>().set_backdrop_shown(settings.backdrop.mini_backdrop);
}

/// Starts the Songs tab's first fetch if the launch shows it, else marks it dirty for its first
/// enter: the whole library is the largest thing the launch could hold for nothing on screen.
///
/// The sort comes off the snapshot the header was seeded from, so the two can't disagree; a fresh
/// install takes title ascending, the `Tracks` global's default.
pub fn spawn_initial_tracks_fetch(
    state: &AppState,
    tracks_ui: &Arc<ui::tracks::TracksUi>,
    view_state: Option<&services::view_state::ViewStateData>,
) {
    if !tracks_ui.section_active() {
        tracks_ui.mark_dirty();
        return;
    }
    let query = ui::callbacks::persisted_sort(view_state, ui::track_list_view::view_id::TRACKS)
        .map_or_else(
            || ui::tracks::SongsQuery::new("title", "asc", ""),
            |sort| ui::tracks::SongsQuery::new(&sort.field, sort.dir.as_str(), ""),
        );
    ui::tracks::refresh(state, tracks_ui, query);
}

/// Starts an entity grid's first load if the launch shows its tab, else marks it dirty for its
/// first enter.
///
/// A macro because each `reload_grid` is a free function in its own module, with no trait the
/// four handles share to be generic over.
macro_rules! initial_grid_fetch {
    ($(#[$doc:meta])* $name:ident, $module:ident, $handle:ty) => {
        $(#[$doc])*
        pub fn $name(app: &AppWindow, state: &AppState, handle: &Arc<$handle>) {
            if !handle.section_active() {
                handle.mark_dirty();
                return;
            }
            ui::$module::reload_grid(app, state, handle);
        }
    };
}

initial_grid_fetch!(
    /// Kick off the initial Albums grid load.
    spawn_initial_albums_fetch,
    albums,
    ui::albums::AlbumsUi
);
initial_grid_fetch!(
    /// Kick off the initial Artists grid load.
    spawn_initial_artists_fetch,
    artists,
    ui::artists::ArtistsUi
);
initial_grid_fetch!(
    /// Kick off the initial Genres grid load.
    spawn_initial_genres_fetch,
    genres,
    ui::genres::GenresUi
);
initial_grid_fetch!(
    /// Kick off the initial Playlists grid load.
    spawn_initial_playlists_fetch,
    playlists,
    ui::playlists::PlaylistsUi
);
