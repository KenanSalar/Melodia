//! The per-view slices, their handles, and the Settings sections beside them.

use std::sync::Arc;

use melodia_app::services;
use melodia_app::state::AppState;
use melodia_artwork::media::image;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Nav};
use melodia_views::ui;
use slint::ComponentHandle;

/// The view handles [`super::install_ui`] goes on to wire.
///
/// Only handles a caller reads: every wired closure owns its own strong `Arc`, and nothing holds a
/// `Weak<…Ui>`, so a field kept for the rest would guard nothing.
pub struct UiHandles {
    pub cover_thumbs: Arc<image::cover_thumbs::CoverThumbs>,
    pub tracks_ui: Arc<ui::tracks::TracksUi>,
    pub albums_ui: Arc<ui::albums::AlbumsUi>,
    pub artists_ui: Arc<ui::artists::ArtistsUi>,
    pub genres_ui: Arc<ui::genres::GenresUi>,
    pub playlists_ui: Arc<ui::playlists::PlaylistsUi>,
    pub radio_ui: Arc<ui::radio::RadioUi>,
}

/// Answers every track row's cover from the shared row tier. Rows carry only the path, so only
/// instantiated rows pay a lookup and nothing pins an evicted buffer.
fn install_row_covers(app: &AppWindow, cover_thumbs: &Arc<image::cover_thumbs::CoverThumbs>) {
    let ct = cover_thumbs.clone();
    // `generation` is read for its effect on the binding, never its value — see `RowCovers`.
    app.global::<melodia_ui::RowCovers>().on_request(move |path, _generation| {
        ct.get_or_schedule_opt(ui::grid_prewarm::nonempty_artwork_path(path.as_str()))
    });

    ui::cover_generation::notify_on_decode(cover_thumbs, app, |app| {
        let covers = app.global::<melodia_ui::RowCovers>();
        covers.set_generation(covers.get_generation().wrapping_add(1));
    });
}

/// Re-runs every card grid's cover binding once a scheduled decode lands.
///
/// The one install for the one grid tier, `set_decoded_notifier` being a `OnceLock`. A bump on an
/// unmounted page dirties nothing, so answering for every grid is cheaper than the tier knowing
/// which one asked. Radio's logo tier is its own.
fn install_grid_covers(app: &AppWindow) {
    ui::cover_generation::notify_on_decode(ui::grid_prewarm::tier(), app, |app| {
        macro_rules! repaint {
            ($($global:ty),+ $(,)?) => {$({
                let g = app.global::<$global>();
                g.set_covers_generation(g.get_covers_generation().wrapping_add(1));
            })+};
        }
        repaint!(
            melodia_ui::Albums,
            melodia_ui::Artists,
            melodia_ui::Browse,
            melodia_ui::Favorites,
            melodia_ui::Playlists,
            melodia_ui::RecentlyPlayed,
        );
    });
}

/// Installs every view slice, seeding the persisted nav index first and reopening each view's
/// persisted detail last.
pub fn install_views(
    app: &AppWindow,
    state: &AppState,
    startup_view_state: Option<&services::view_state::ViewStateData>,
) -> UiHandles {
    // Before any `wire_*`: every section seeds its `section_active` shadow off these, and its gate
    // can't correct a wrong seed (`ui_setup_tests` says why).
    if let Some(vs) = startup_view_state {
        // One fold for the indices an older `views.json` holds that route nowhere now, one for
        // Radio switched off. Neither belongs inside the other.
        let idx = ui::radio::fold_disabled_nav_index(
            ui::my_library::fold_retired_nav_index(vs.last_nav_index),
            state.radio_enabled.get(),
        );
        if (0..=services::view_state::MAX_NAV_INDEX).contains(&idx) {
            app.global::<Nav>().set_selected_index(idx);
        }
        ui::my_library::seed_tab(app, vs.my_library_tab);
        ui::radio::seed_tab(app, vs.radio_tab);
    }

    ui::callbacks::wire_all(app, state);
    // Takes no view handle, so it needn't wait for the five tabs.
    ui::my_library::install(app, state);

    // Ahead of the first `install`, every slice cloning the row tier into its handle.
    let cover_thumbs = Arc::new(image::cover_thumbs::CoverThumbs::new());
    install_row_covers(app, &cover_thumbs);
    install_grid_covers(app);
    ui::drag::install(app);

    let cx = ui::view_ctx::ViewCtx {
        app,
        state,
        cover_thumbs: &cover_thumbs,
        view_state: startup_view_state,
    };

    // The peer parameters are the ordering: a hand-off wired against a handle that doesn't exist
    // yet is a compile error.
    let tracks_ui = ui::tracks::install(cx);
    let browse_ui = ui::browse::install(cx);
    let albums_ui = ui::albums::install(cx);
    let artists_ui = ui::artists::install(cx, &albums_ui);
    let genres_ui = ui::genres::install(cx);
    let playlists_ui = ui::playlists::install(cx);
    ui::favorites::install(cx, &artists_ui);
    ui::recently_played::install(cx);
    let radio_ui = ui::radio::install(cx);
    let search_ui = ui::search::install(cx, &albums_ui, &artists_ui);

    ui::callbacks::wire_cross_tab_nav(app, state, &albums_ui, &artists_ui, &genres_ui);

    // So Mouse-4/5 replay can open a detail without handles threaded through `winit_filter`.
    *ui::nav_history::nav().handles().albums.lock() = Some(albums_ui.clone());
    *ui::nav_history::nav().handles().artists.lock() = Some(artists_ui.clone());
    *ui::nav_history::nav().handles().genres.lock() = Some(genres_ui.clone());
    *ui::nav_history::nav().handles().playlists.lock() = Some(playlists_ui.clone());
    *ui::nav_history::nav().handles().radio.lock() = Some(radio_ui.clone());

    // Here rather than in each slice's `install`: the history seed below needs none of them landed.
    ui::albums::seed_detail_from_settings(app, state, &albums_ui);
    ui::artists::seed_detail_from_settings(app, state, &artists_ui);
    ui::genres::seed_detail_from_settings(app, state, &genres_ui);
    ui::playlists::seed_detail_from_settings(app, state, &playlists_ui);
    ui::radio::seed_detail_from_settings(app, state, &radio_ui);

    // The boot view, so Mouse-4 has a target after the first sidebar click. Reads the detail
    // globals while still `-1`; each reopen appends its own entry once it lands.
    ui::nav_history::record_current(app);

    // The Now-Playing heart and star rating fan into every per-row cache.
    let row_flags = ui::callbacks::RowFlagHandles {
        tracks: &tracks_ui,
        browse: &browse_ui,
        albums: &albums_ui,
        artists: &artists_ui,
        genres: &genres_ui,
        playlists: &playlists_ui,
        search: &search_ui,
    };
    ui::callbacks::wire_now_playing_favorite(app, state, row_flags);
    ui::callbacks::wire_now_playing_rating(app, state, row_flags);
    // Deferred past `app.show()`: until the window is on a monitor its scale factor reads 1.0, and
    // a `HiDPI` display would get the 1× decode size.
    let (tune_rows, tune_radio) = (cover_thumbs.clone(), radio_ui.clone());
    let weak = app.as_weak();
    let retune = move || {
        let Some(app) = weak.upgrade() else { return };
        ui::grid_prewarm::tune_for_display(&app);
        // Radio's logo tier is its own, and `radio::covers` says why.
        ui::radio::tune_cache_for_display(&app, &tune_radio);
        tune_rows.set_thumb_size(image::cover_thumbs::row_cover_size(f64::from(
            app.window().scale_factor(),
        )));
    };
    // Again on every resize, of the window or of the content panel the grids lay out in: a cap
    // read once at launch is one a later maximize overruns. A retune leaves the tier's buffers in
    // place, so cards keep painting until each replacement lands.
    let chrome = app.global::<melodia_ui::WindowChrome>();
    chrome.on_display_changed(retune.clone());
    chrome.on_content_panel_resized(retune.clone());
    if let Err(e) = slint::invoke_from_event_loop(retune) {
        log::warn!("Failed to schedule cover-cache display tuning: {}", describe(&e));
    }

    UiHandles { cover_thumbs, tracks_ui, albums_ui, artists_ui, genres_ui, playlists_ui, radio_ui }
}

/// Every Settings section, plus the notifications stack. The updater's task and callbacks wire
/// later, beside the onboarding card that defers the task.
pub fn install_library_settings_and_friends(
    app: &AppWindow,
    state: &AppState,
) -> Result<std::rc::Rc<ui::shell::notifications::NotificationsUi>, melodia_core::error::AppError> {
    ui::settings::library_settings::install(app, state)
        .map_err(|e| melodia_core::error::AppError::io("library_settings install", e))?;
    ui::callbacks::wire_library_settings(app, state);
    ui::settings::playback_settings::install_playback_settings(app, state);
    ui::settings::signal_path::install(app, state);
    ui::settings::output_settings::install(app, state);
    ui::equalizer::install_equalizer(app, state);
    ui::replaygain::install_replaygain(app, state);
    ui::visualizer::install_visualizer(app, state);
    ui::sleep_timer::install_sleep_timer(app, state);
    ui::settings::scrobbling_settings::install_scrobbling(app, state);
    ui::settings::discord_settings::install_discord(app, state);
    ui::settings::radio_settings::install_radio(app, state);
    ui::settings::lyrics_settings::install(app, state);
    let notifications = ui::shell::notifications::install(app);
    ui::settings::file_watching::install(app, state, &notifications);
    ui::settings::rating_writeback::install(app, state);
    ui::settings::updater_settings::install(app, state);
    ui::settings::about::install(app, state);
    ui::settings::diagnostics::install(app, state, &notifications);
    ui::settings::settings_page::install(app, state);
    ui::track_columns::install(app);
    // Toast rows carry strings Rust resolved once and outlive every navigation, so a language
    // switch reaches them only through this. Not in the locale callback, which wires first.
    {
        let notifications = notifications.clone();
        if let Err(e) = ui::locale_refresh::on_locale_changed(state, app.as_weak(), move |ui| {
            notifications.refresh_for_locale(ui);
        }) {
            log::warn!("notifications locale refresher: {}", describe(&e));
        }
    }
    Ok(notifications)
}
