//! The per-view slices, their handles, and the Settings sections beside them.

use std::sync::Arc;

use melodia_app::services;
use melodia_app::state::AppState;
use melodia_artwork::media::image;
use melodia_ui::{AppWindow, Nav};
use melodia_views::ui;
use slint::ComponentHandle;

/// The per-view handles `install_views` hands back for the wiring `main()` still
/// owns: the initial fetches, the playlist and station import/export pills
/// (which need the notifications stack), and the `cover_thumbs` consumers.
///
/// Only handles a *caller* reads live here. `BrowseUi` / `FavoritesUi` /
/// `RecentlyPlayedUi` / `SearchUi` deliberately don't: every wired closure
/// captures its own strong `Arc` clone and is owned by the `AppWindow` for the
/// life of the app, and there is no `Weak<…Ui>` anywhere in the tree — so a
/// field here would be a keepalive guarding nothing.
pub struct UiHandles {
    pub cover_thumbs: Arc<image::cover_thumbs::CoverThumbs>,
    pub tracks_ui: Arc<ui::tracks::TracksUi>,
    pub albums_ui: Arc<ui::albums::AlbumsUi>,
    pub artists_ui: Arc<ui::artists::ArtistsUi>,
    pub genres_ui: Arc<ui::genres::GenresUi>,
    pub playlists_ui: Arc<ui::playlists::PlaylistsUi>,
    pub radio_ui: Arc<ui::radio::RadioUi>,
}

/// Resolve every `TrackListRowItem` thumbnail — all track tables, all views —
/// through the one shared row-tier LRU. Rows carry only the artwork path, so
/// only instantiated rows pay a lookup and nothing pins evicted buffers. Part of
/// no slice's `install`, serving every view.
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

/// Install every view slice and its callbacks. Seeds the persisted nav index
/// and reopens each view's persisted detail (if any). Returns the per-view
/// handles for downstream wiring.
pub fn install_views(
    app: &AppWindow,
    state: &AppState,
    startup_view_state: Option<&services::view_state::ViewStateData>,
) -> UiHandles {
    // The persisted nav index and My Library's tab, *before* any `wire_*` runs:
    // every section handle seeds its synchronous `section_active` shadow by
    // reading these at wire time, and a `SectionActiveGate` can't be relied on
    // to correct a wrong seed — its `ChangeTracker` baselines silently inside
    // `AppWindow::new()`, so a section seeded active for a hidden view stays
    // that way all session. See the gate's bullet in
    // `.claude/rules/ui-patterns.md` for what that costs.
    //
    // The Favorites *tab* seeds down beside the detail views instead, needing
    // the `favorites_ui` handle; My Library's and Radio's need none.
    if let Some(vs) = startup_view_state {
        // 4–7 were Albums / Artists / Genres / Playlists — a `views.json`
        // written by a released build still holds them, and they route nowhere.
        // 10 routes only while Radio is switched on, so the two folds compose:
        // they answer different questions and neither belongs inside the other.
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
    // The page's own three callbacks take no view handle, so they wire here
    // rather than after the five tabs.
    ui::my_library::install(app, state);

    // Ahead of the first `install`, every slice cloning the cache into its handle.
    let cover_thumbs = Arc::new(image::cover_thumbs::CoverThumbs::new());
    install_row_covers(app, &cover_thumbs);

    let cx = ui::view_ctx::ViewCtx {
        app,
        state,
        cover_thumbs: &cover_thumbs,
        view_state: startup_view_state,
    };

    // Each `install` is models + handle + wiring, in that order, inside the
    // slice. The peer parameters below *are* the ordering — `artists::install`
    // cannot be written above `albums_ui` — so a cross-tab hand-off wired
    // against a handle that doesn't exist yet is a compile error, not a comment.
    let tracks_ui = ui::tracks::install(cx);
    let browse_ui = ui::browse::install(cx);
    let albums_ui = ui::albums::install(cx);
    let artists_ui = ui::artists::install(cx, &albums_ui);
    let genres_ui = ui::genres::install(cx);
    let playlists_ui = ui::playlists::install(cx);
    let favorites_ui = ui::favorites::install(cx, &artists_ui);
    let recently_played_ui = ui::recently_played::install(cx);
    let radio_ui = ui::radio::install(cx);
    // Bound under an underscore rather than dropped at the semicolon, so the
    // keepalive note on `UiHandles` has something to attach to.
    let _search_ui = ui::search::install(cx, &albums_ui, &artists_ui);

    // Every track-list view's right-click "Go to Album/Artist/Genre", after all
    // three target handles exist.
    ui::callbacks::wire_cross_tab_nav(app, state, &albums_ui, &artists_ui, &genres_ui);

    // Publish into the nav-history registry so Mouse-4/5 replay can dispatch
    // `open_*` futures without threading handles through `winit_filter`.
    *ui::nav_history::nav().handles().albums.lock() = Some(albums_ui.clone());
    *ui::nav_history::nav().handles().artists.lock() = Some(artists_ui.clone());
    *ui::nav_history::nav().handles().genres.lock() = Some(genres_ui.clone());
    *ui::nav_history::nav().handles().playlists.lock() = Some(playlists_ui.clone());
    *ui::nav_history::nav().handles().radio.lock() = Some(radio_ui.clone());

    // The five persisted-detail reopens stay adjacent here rather than folding
    // into each slice's `install`, because the history seed below depends on
    // none of them having landed yet. The tabbed pages' tab seeds *did* fold
    // in, their handle being the receiver.
    ui::albums::seed_detail_from_settings(app, state, &albums_ui);
    ui::artists::seed_detail_from_settings(app, state, &artists_ui);
    ui::genres::seed_detail_from_settings(app, state, &genres_ui);
    ui::playlists::seed_detail_from_settings(app, state, &playlists_ui);
    ui::radio::seed_detail_from_settings(app, state, &radio_ui);

    // Seed the nav-history with the boot view, so Mouse-4 has a target after the
    // first sidebar click — otherwise a boot landing on a section with no
    // persisted detail records only the destination and `back()` returns `None`.
    // Reads the detail global while it is still `-1`; the async
    // `seed_detail_from_settings` future appends its own entry on top once it
    // lands.
    ui::nav_history::record_current(app);

    // The Now-Playing heart and star rating fan into every per-row cache.
    ui::callbacks::wire_now_playing_favorite(
        app,
        state,
        &tracks_ui,
        &browse_ui,
        &albums_ui,
        &artists_ui,
        &genres_ui,
    );
    ui::callbacks::wire_now_playing_rating(
        app,
        state,
        &tracks_ui,
        &browse_ui,
        &albums_ui,
        &artists_ui,
        &genres_ui,
    );
    // Retune every grid-tier cover LRU to the real display — one band for all of
    // them, drawing the same card at the same size. Genres has no cover cache.
    //
    // **Deferred to the event loop, because this runs long before
    // `app.show()`**: inline, `with_winit_window` finds no window and every tier
    // silently takes its construction fallback, while `scale_factor` answers 1.0
    // until the window is on a monitor and serves a `HiDPI` display the 1×
    // decode size. The closure's `Arc` clones keep the handles this scope drops.
    let (tune_albums, tune_artists) = (albums_ui.clone(), artists_ui.clone());
    let (tune_playlists, tune_favorites) = (playlists_ui.clone(), favorites_ui.clone());
    let (tune_recent, tune_browse) = (recently_played_ui.clone(), browse_ui.clone());
    let tune_radio = radio_ui.clone();
    let tune_rows = cover_thumbs.clone();
    let weak = app.as_weak();
    let retune = move || {
        let Some(app) = weak.upgrade() else { return };
        ui::albums::tune_cache_for_display(&app, &tune_albums);
        ui::artists::tune_cache_for_display(&app, &tune_artists);
        ui::playlists::tune_cache_for_display(&app, &tune_playlists);
        ui::favorites::tune_cache_for_display(&app, &tune_favorites);
        ui::recently_played::tune_cache_for_display(&app, &tune_recent);
        ui::browse::tune_cache_for_display(&app, &tune_browse);
        ui::radio::tune_cache_for_display(&app, &tune_radio);
        // The row tier belongs to no view, so it has no
        // `tune_cache_for_display` — but it owes the same post-show read.
        tune_rows.set_thumb_size(image::cover_thumbs::row_cover_size(f64::from(
            app.window().scale_factor(),
        )));
    };
    // **And again on every resize.** Both answers move with the window, and a cap read once at
    // launch is the cap a later maximize overruns — the cards past it can only paint
    // placeholders, the lookup behind them scheduling against a tier that cannot hold them.
    // Setting the cap exactly rather than growing it: a smaller window really does draw fewer
    // cards, and a drag that oscillates re-warms through the model rebuild it triggers anyway.
    app.global::<melodia_ui::WindowChrome>().on_display_changed(retune.clone());
    if let Err(e) = slint::invoke_from_event_loop(retune) {
        log::warn!("Failed to schedule cover-cache display tuning: {e}");
    }

    // The four handles not returned are deliberately dropped here — see
    // `UiHandles`.
    UiHandles {
        cover_thumbs,
        tracks_ui,
        albums_ui,
        artists_ui,
        genres_ui,
        playlists_ui,
        radio_ui,
    }
}

/// Every Settings section, plus the notifications stack. The updater's Slint
/// state seeds here too; its daily-check task and callbacks wire from `main()`
/// once the `AppState` clones and tokio handle are in scope.
pub fn install_library_settings_and_friends(
    app: &AppWindow,
    state: &AppState,
) -> Result<std::rc::Rc<ui::shell::notifications::NotificationsUi>, melodia_core::error::AppError> {
    ui::settings::library_settings::install(app, state).map_err(|e| {
        melodia_core::error::AppError::Window(format!("library_settings install: {e}"))
    })?;
    ui::callbacks::wire_library_settings(app, state);
    ui::settings::playback_settings::install_playback_settings(app, state);
    ui::equalizer::install_equalizer(app, state);
    ui::replaygain::install_replaygain(app, state);
    ui::visualizer::install_visualizer(app, state);
    ui::sleep_timer::install_sleep_timer(app, state);
    ui::settings::scrobbling_settings::install_scrobbling(app, state);
    ui::settings::discord_settings::install_discord(app, state);
    ui::settings::radio_settings::install_radio(app, state);
    let notifications = ui::shell::notifications::install(app);
    ui::settings::file_watching::install(app, state, &notifications);
    ui::settings::rating_writeback::install(app, state);
    ui::settings::updater_settings::install(app, state);
    ui::settings::motion::install(app, state);
    ui::settings::about::install(app, state);
    // Takes the stack because it both toasts and, on the launch after a panic,
    // pushes the "crashed last time" notice itself.
    ui::settings::diagnostics::install(app, state, &notifications);
    ui::settings::settings_page::install(app, state);
    ui::hero_chips::install(app);
    // The stack is the one surface a language switch can't reach on its own: its rows carry
    // strings Rust resolved once, they outlive every navigation, and the file-watching
    // toggle two cards from the language picker raises one. Wired here rather than inline in
    // the locale callback because that runs before this handle exists.
    {
        let notifications = notifications.clone();
        if let Err(e) = ui::locale_refresh::on_locale_changed(state, app.as_weak(), move |ui| {
            notifications.refresh_for_locale(ui);
        }) {
            log::warn!("notifications locale refresher: {e}");
        }
    }
    Ok(notifications)
}
