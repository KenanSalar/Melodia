//! UI installation phase: locale, chrome, per-view installers, settings
//! hydration, initial fetches, and the library-changed refresher.

use std::sync::Arc;

use melodia::{AppWindow, ArtistDetail, Nav, Player, media, services, state::AppState, ui};
use slint::ComponentHandle;

/// Hydrate Slint's bundled-translation runtime from the persisted
/// `settings.locale` *before* `app.run()` so the very first frame's
/// `@tr(...)` resolutions land in the chosen language. Then wire
/// `Settings.language-{names,codes,idx}` + the `language-changed`
/// callback.
pub fn install_locale(
    app: &AppWindow,
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let persisted_locale =
        startup_settings.map_or_else(|| "en".to_owned(), |s| s.locale.clone());
    if let Err(e) = slint::select_bundled_translation(&persisted_locale) {
        log::warn!("select_bundled_translation({persisted_locale}): {e:?}");
    }
    ui::settings::locale::install_locale(app, state);
}

/// Install `WindowChrome` (titlebar mode, drag-region, native-frame hydrate).
/// Errors are logged and swallowed — chrome install is best-effort.
pub fn install_app_chrome(app: &AppWindow, state: &AppState) {
    if let Err(e) = ui::window_chrome::install(app, state) {
        log::warn!("window_chrome::install: {e}");
    }
}

/// Container for the per-view UI handles created during `install_views`,
/// returned for the wiring `main()` still owns: the initial per-view fetches
/// (`tracks` / `albums` / `artists` / `genres` / `playlists`), the playlist
/// import/export pills (which can only be wired once the notifications stack
/// exists), and the `cover_thumbs` consumers (Material You, Now Playing).
/// `now_playing_favorite` / `now_playing_rating` are wired *inside*
/// `install_views` — they need handles that don't leave this function.
///
/// Only the handles a *caller* actually reads live here. `BrowseUi` /
/// `FavoritesUi` / `RecentlyPlayedUi` / `SearchUi` deliberately do **not**:
/// every wired closure
/// and `library_changed_tx` subscriber captures its own strong `Arc` clone, and
/// those closures are owned by the `AppWindow` (and by spawned tasks) for the
/// lifetime of the app. There is not a single `Arc::downgrade` or `Weak<…Ui>`
/// in the tree, so a field here would have been a keepalive guarding nothing.
pub struct UiHandles {
    pub cover_thumbs: Arc<media::cover_thumbs::CoverThumbs>,
    pub tracks_ui: Arc<ui::tracks::TracksUi>,
    pub albums_ui: Arc<ui::albums::AlbumsUi>,
    pub artists_ui: Arc<ui::artists::ArtistsUi>,
    pub genres_ui: Arc<ui::genres::GenresUi>,
    pub playlists_ui: Arc<ui::playlists::PlaylistsUi>,
}

/// Resolve every `TrackListRowItem` thumbnail — all track tables, all views —
/// through the one shared row-tier LRU.
///
/// Rows carry only the artwork path, so only instantiated (~visible) rows pay a
/// lookup/decode and nothing pins evicted buffers. The closure captures only the
/// `Arc<CoverThumbs>` — no UI handle, no reference cycle.
///
/// Not part of any slice's `install`: it sat inside the Tracks block for years
/// while its own comment said it serves every view.
fn install_row_covers(app: &AppWindow, cover_thumbs: &Arc<media::cover_thumbs::CoverThumbs>) {
    let ct = cover_thumbs.clone();
    app.global::<melodia::RowCovers>()
        .on_request(move |path| ct.get_or_load_opt(Some(path.as_str()).filter(|s| !s.is_empty())));
}

/// Install every view slice and its callbacks. Seeds the persisted nav index
/// and reopens each view's persisted detail (if any). Returns the per-view
/// handles for downstream wiring.
pub fn install_views(
    app: &AppWindow,
    state: &AppState,
    startup_view_state: Option<&services::view_state::ViewStateData>,
) -> UiHandles {
    // 5a. The persisted nav index, *before* any `wire_*` runs. Each of the nine
    // section handles seeds its synchronous `section_active` shadow by reading
    // `Nav.selected-index` at wire time, so hydrating afterwards left every one
    // of them holding the answer for the global's declared default (3, Tracks)
    // rather than the section actually being restored. They then depended on
    // `SectionActiveGate`'s `changed` firing to correct themselves, and that is
    // not something it can be relied on for: its `ChangeTracker` is evaluated
    // inside `AppWindow::new()` and adopts that first reading *silently*, so it
    // becomes the baseline rather than an edge. The restored section recovered
    // (its gate still had a false→true to deliver); `TracksUi` did not, and sat
    // marked active for a hidden view all session — see the `SectionActiveGate`
    // bullet in `.claude/rules/ui-patterns.md` for the cost that carries.
    //
    // The Favorites *tab* still seeds down at `seed_tab` beside the detail
    // views: it needs the `favorites_ui` handle, which doesn't exist yet here.
    //
    // **My Library's tab seeds right here instead**, for exactly the reason the nav
    // index does: its five sub-views each seed `section_active` from
    // `Nav.selected-index == 3 && MyLibrary.tab-idx == <its tab>`, so a seed running
    // after `wire_all` leaves all five answering for the global's declared `0` — Songs
    // wrongly active, the restored tab wrongly inactive, and one wasted full-library
    // query per launch. It needs no handle, so nothing holds it back.
    if let Some(vs) = startup_view_state {
        // 4–7 were Albums / Artists / Genres / Playlists; a `views.json` written by a
        // released build still holds them, and they route nowhere now.
        let idx = ui::my_library::fold_retired_nav_index(vs.last_nav_index);
        if (0..=9).contains(&idx) {
            app.global::<Nav>().set_selected_index(idx);
        }
        ui::my_library::seed_tab(app, vs.my_library_tab);
    }

    ui::callbacks::wire_all(app, state);
    // The page's own three callbacks — the tab pick, the shared filter, the back
    // arrow. None takes a view handle, so it wires here rather than after the five.
    ui::my_library::install(app, state);

    // The shared row tier, and the one lazy callback every track table in the
    // app resolves its thumbnails through — both ahead of the first `install`,
    // since every slice clones the cache into its handle.
    let cover_thumbs = Arc::new(media::cover_thumbs::CoverThumbs::new());
    install_row_covers(app, &cover_thumbs);

    let cx = ui::view_ctx::ViewCtx {
        app,
        state,
        cover_thumbs: &cover_thumbs,
        view_state: startup_view_state,
    };

    // Each `install` is models + handle + wiring, in that order, inside the
    // slice. The peer parameters below are the ordering: `artists::install`
    // cannot be written above `albums_ui`.

    // 5b. Tracks view.
    let tracks_ui = ui::tracks::install(cx);

    // 5c. Browse view.
    let browse_ui = ui::browse::install(cx);

    // 5c2a. Albums view.
    let albums_ui = ui::albums::install(cx);

    // 5c2b. Artists view. Installed after Albums so the cross-tab "open
    // album from Artist Detail" hand-off has a live `AlbumsUi` to call
    // into — the peer parameter is what makes that a compile error to
    // get wrong rather than a comment to read.
    let artists_ui = ui::artists::install(cx, &albums_ui);

    // 5c2c. Genres view. Self-contained: no cross-tab origin, no
    // artwork — just the shared row-tier `cover_thumbs` for the
    // detail track-list's small artwork column.
    let genres_ui = ui::genres::install(cx);

    // 5c2d. Playlists view. CRUD entry points route through the
    // shared `Dialog` global; drag-reorder lives in the sibling
    // `DraggableTrackList` component (the shared `TrackList` stays
    // drag-free); the "Add to Playlist" entry on every track row's
    // overflow opens the multi-select picker (`Playlists.request-add-to-playlist`
    // → `add-tracks-to-selected`); and OS file drops on
    // the detail view route through `CURRENT_PLAYLIST_ID` in
    // `ui::window_chrome::drop_coalescer` (set/cleared by
    // `playlists::detail::open_playlist` / `close_detail`). Queue
    // Sheet always wins when both targets are open.
    let playlists_ui = ui::playlists::install(cx);

    // 5c2e. Favorites view. Installed after Artists so the cross-tab
    // open-artist hand-off has a live `ArtistsUi` to call into.
    let favorites_ui = ui::favorites::install(cx, &artists_ui);

    // 5c2e-bis. Recently-Played view (sidebar index 8). A trimmed Favorites —
    // the shared row-tier `cover_thumbs` serves the Songs list; the handle
    // allocates its own mosaic and Most Played LRUs (the latter released on
    // tab-leave as well as on section-leave). Its row-menu "Go to …" entries are
    // wired centrally by `wire_cross_tab_nav` below.
    let recently_played_ui = ui::recently_played::install(cx);

    // 5c2f. Search view (sidebar index 0). Installed after both Albums +
    // Artists so the cross-tab open-album / open-artist hand-offs have
    // live UI handles to call into. The shared row-tier `cover_thumbs`
    // serves the Songs `TrackList`; the SearchUi allocates its own
    // 180 px + 200 px LRUs for the Albums + Artists strips (released
    // on tab-leave). No initial fetch — Search is query-driven, so
    // the page paints empty until the user types.
    //
    // Bound under an underscore rather than dropped at the semicolon: nothing
    // downstream reads it, and the binding is where the keepalive note on
    // `UiHandles` has something to attach to. Every wired closure holds its own
    // strong `Arc`, so the handle outlives this scope regardless.
    let _search_ui = ui::search::install(cx, &albums_ui, &artists_ui);

    // 5c2g. Cross-tab navigation for every track-list view's right-
    // click "Go to Album/Artist/Genre" entries. Must run *after* every
    // per-view `wire_*` above so all three target UI handles
    // (`albums_ui`, `artists_ui`, `genres_ui`) exist.
    ui::callbacks::wire_cross_tab_nav(app, state, &albums_ui, &artists_ui, &genres_ui);

    // Publish the per-section `*Ui` handles into the nav-history
    // registry so Mouse-4/5 replay can dispatch `open_*` futures
    // without the handles having to be threaded through `winit_filter`.
    // See `src/ui/nav_history.rs`.
    *state.ui_handles.albums.lock() = Some(albums_ui.clone());
    *state.ui_handles.artists.lock() = Some(artists_ui.clone());
    *state.ui_handles.genres.lock() = Some(genres_ui.clone());
    *state.ui_handles.playlists.lock() = Some(playlists_ui.clone());

    // 5c2h. The four persisted-detail reopens, then the history seed that
    // depends on none of them having landed yet — which is why they stay four
    // adjacent lines here rather than folding into each slice's `install`. The
    // two tabbed pages' own tab seeds *did* fold in, their handle being the
    // receiver; see `ui::favorites::install`.
    ui::albums::seed_detail_from_settings(app, state, &albums_ui);
    ui::artists::seed_detail_from_settings(app, state, &artists_ui);
    ui::genres::seed_detail_from_settings(app, state, &genres_ui);
    ui::playlists::seed_detail_from_settings(app, state, &playlists_ui);

    // Seed the nav-history with the boot view so Mouse-4 has a target
    // after the user's first sidebar click. Without this, history starts
    // empty on a boot that lands on a section with no persisted detail
    // (Tracks / Browse / Favorites / Search / fresh install), so the
    // first sidebar nav records only the destination and `back()` returns
    // `None`. Reads `Nav.selected-index` (hydrated in 5a, at the top of this
    // function) and the section detail global (still `-1` — the async
    // `seed_detail_from_settings` futures haven't run yet); the async
    // future's own `record_current` appends a `{section, Some(id)}` entry on
    // top once it lands.
    ui::nav_history::record_current(state, app);

    // 5c3. Now-Playing favourite heart + star rating fan into every per-row cache.
    ui::callbacks::wire_now_playing_favorite(
        app, state, &tracks_ui, &browse_ui, &albums_ui, &artists_ui, &genres_ui,
    );
    ui::callbacks::wire_now_playing_rating(
        app, state, &tracks_ui, &browse_ui, &albums_ui, &artists_ui, &genres_ui,
    );
    // Retune every grid-tier cover LRU to the real display — same band for
    // all of them, since they all draw the same card at the same size (see
    // `ui::grid_prewarm::cover_cap`). Genres has no cover cache, so no
    // tuning step.
    ui::albums::tune_cache_for_display(app, &albums_ui);
    ui::artists::tune_cache_for_display(app, &artists_ui);
    ui::playlists::tune_cache_for_display(app, &playlists_ui);
    ui::favorites::tune_cache_for_display(app, &favorites_ui);
    ui::recently_played::tune_cache_for_display(app, &recently_played_ui);
    ui::browse::tune_cache_for_display(app, &browse_ui);

    // `browse_ui` / `favorites_ui` / `recently_played_ui` / `_search_ui` are
    // deliberately dropped here: their wired closures each hold a strong `Arc`
    // clone, so the objects outlive this scope regardless. See `UiHandles`.
    UiHandles {
        cover_thumbs,
        tracks_ui,
        albums_ui,
        artists_ui,
        genres_ui,
        playlists_ui,
    }
}

/// Library settings + playback toggles + notifications stack + file-watcher
/// toggle (sections 5d, 5d2, 5d3, 5d4 in the original `main`). The
/// auto-updater's UI state seed (Updater global) lands here too —
/// the daily-check task + Updater callbacks are wired separately from
/// `main()` once the `AppState` clones + tokio handle are in scope.
pub fn install_library_settings_and_friends(
    app: &AppWindow,
    state: &AppState,
) -> Result<std::rc::Rc<ui::shell::notifications::NotificationsUi>, melodia::error::AppError> {
    ui::settings::library_settings::install(app, state)
        .map_err(|e| melodia::error::AppError::Window(format!("library_settings install: {e}")))?;
    ui::callbacks::wire_library_settings(app, state);
    ui::settings::playback_settings::install_playback_settings(app, state);
    ui::equalizer::install_equalizer(app, state);
    ui::replaygain::install_replaygain(app, state);
    ui::visualizer::install_visualizer(app, state);
    ui::sleep_timer::install_sleep_timer(app, state);
    ui::settings::scrobbling_settings::install_scrobbling(app, state);
    ui::settings::discord_settings::install_discord(app, state);
    let notifications = ui::shell::notifications::install(app);
    ui::settings::file_watching::install(app, state, &notifications);
    ui::settings::updater_settings::install(app, state);
    ui::settings::about::install(app, state);
    // Takes the stack because it both toasts and, on the launch after a panic,
    // pushes the "crashed last time" notice itself.
    ui::settings::diagnostics::install(app, state, &notifications);
    ui::settings::settings_page::install(app, state);
    ui::hero_chips::install(app);
    Ok(notifications)
}

/// Push the current `PlayerState` into `Player.vm` / `Player.queue` so the
/// now-playing bar shows the persisted last-played track on launch.
pub fn seed_initial_view_model(
    app: &AppWindow,
    state: &AppState,
    cover_thumbs: &media::cover_thumbs::CoverThumbs,
) {
    use melodia::player::state::lock_state;

    let s = lock_state(&state.player_state);
    let light = s.to_view_model_light();
    let qvm = s.to_queue_view_model();
    drop(s);

    let player = app.global::<Player>();
    player.set_vm(ui::shell::bridge::to_slint_player_vm(&light, cover_thumbs));
    // Seed the position scalars from the snapshot so a freshly-restored
    // session shows the saved position immediately, before the first
    // playback-monitor tick lands. Position scalars live outside `vm` —
    // see `melodia-ui/ui/models.slint` for the rationale.
    let pos = i32::try_from(light.position_ms).unwrap_or(i32::MAX);
    let dur = i32::try_from(light.duration_ms).unwrap_or(i32::MAX);
    player.set_position_ms(pos);
    player.set_duration_ms(dur);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "u64 → f64 → f32: ms positions stay below f53 mantissa range, f32 progress is for UI display only"
    )]
    let progress = if light.duration_ms > 0 {
        (light.position_ms as f64 / light.duration_ms as f64) as f32
    } else {
        0.0
    };
    player.set_progress(progress);
    player.set_queue(ui::shell::bridge::to_slint_queue_vm(&qvm));
}

/// Apply every UI-visible persisted section to the Slint globals: sidebar
/// width + collapsed state (from `settings.json`) and per-view column
/// visibility / widths plus the section-collapse toggles (from
/// `views.json`). Missing entries leave the Slint defaults in place —
/// matches first-launch behaviour. When a cached snapshot is `None`,
/// falls back to reading that file from disk; `main()` passes the
/// once-read snapshots to avoid a redundant parse.
pub fn hydrate_ui_from_settings(
    app: &AppWindow,
    state: &AppState,
    cached_settings: Option<&services::settings::SettingsData>,
    cached_view_state: Option<&services::view_state::ViewStateData>,
) {
    let owned_settings;
    let settings: &services::settings::SettingsData = match cached_settings {
        Some(s) => s,
        None => match services::settings::read_settings(&state.paths) {
            Ok(s) => {
                owned_settings = s;
                &owned_settings
            }
            Err(e) => {
                log::warn!("hydrate_ui_from_settings: read settings failed: {e}");
                return;
            }
        },
    };
    let owned_view_state;
    let vs: &services::view_state::ViewStateData = match cached_view_state {
        Some(v) => v,
        None => match services::view_state::read_view_state(&state.paths) {
            Ok(v) => {
                owned_view_state = v;
                &owned_view_state
            }
            Err(e) => {
                log::warn!("hydrate_ui_from_settings: read view state failed: {e}");
                return;
            }
        },
    };
    apply_sidebar_width(app, settings);
    apply_sidebar_collapsed(app, settings);
    ui::track_list_view::hydrate_tracks_view(app, vs);
    ui::track_list_view::hydrate_browse_view(app, vs);
    ui::track_list_view::hydrate_album_detail_view(app, vs);
    ui::track_list_view::hydrate_artist_detail_view(app, vs);
    ui::track_list_view::hydrate_genre_detail_view(app, vs);
    ui::track_list_view::hydrate_playlist_detail_view(app, vs);
    ui::track_list_view::hydrate_favorites_view(app, vs);
    ui::track_list_view::hydrate_recently_played_view(app, vs);
    ui::track_list_view::hydrate_search_view(app, vs);
    app.global::<ArtistDetail>()
        .set_albums_collapsed(vs.artist_albums_collapsed);
    ui::settings::settings_page::seed_tab(app, vs.settings_tab);
}

/// The Slint side already clamps `Nav.sidebar-width` to
/// `[Theme.sidebar-min-w, Theme.sidebar-max-w]` at the use site
/// (`melodia-ui/ui/layout/sidebar.slint`), so no Rust-side clamp is needed.
fn apply_sidebar_width(app: &AppWindow, settings: &services::settings::SettingsData) {
    // Persisted sidebar width is f64 (settings.json) — Slint uses f32. Widths
    // are tens-to-hundreds of pixels, well within f32 precision.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "sidebar widths are small pixel counts; f32 precision is sufficient"
    )]
    let width = settings.sidebar_width as f32;
    app.global::<Nav>().set_sidebar_width(width);
}

fn apply_sidebar_collapsed(app: &AppWindow, settings: &services::settings::SettingsData) {
    app.global::<Nav>()
        .set_sidebar_collapsed(settings.layout.sidebar_collapsed);
}

/// Kick off initial Tracks fetch so the list is populated by the time the
/// user navigates to it. The sort comes from the persisted
/// `view_sort["tracks"]` (so a relaunch restores it); a fresh install
/// falls back to title ascending, matching the `Tracks` global default
/// and the header seeded by `ui::tracks::install`.
pub fn spawn_initial_tracks_fetch(
    state: &AppState,
    tracks_ui: &Arc<ui::tracks::TracksUi>,
    weak: slint::Weak<AppWindow>,
) {
    let (sort_field, sort_dir) =
        ui::detail_view::resolve_view_sort(state, ui::track_list_view::view_id::TRACKS, "title");
    let s = state.clone();
    let tu = tracks_ui.clone();
    state.runtime.spawn(async move {
        if let Err(e) =
            ui::tracks::fetch_and_apply(&s, &tu, weak, sort_field, sort_dir, String::new()).await
        {
            log::warn!("initial tracks fetch: {e}");
        }
    });
}

/// Kick off an entity grid's initial fetch so its cards are populated by the time
/// the user navigates to it.
///
/// One generator for the four grids rather than four bodies differing only in a
/// module path and a log tag. A macro rather than a generic `fn` for the reason
/// `impl_mosaic_hero!` and `impl_detail_view_helpers` are macros: `AlbumsUi` /
/// `ArtistsUi` / `GenresUi` / `PlaylistsUi` are distinct types with no trait between
/// them, and each `fetch_grid` is a free function in its own module.
///
/// Tracks is deliberately not among them — it resolves a persisted sort first and
/// calls `fetch_and_apply`, not `fetch_grid`.
macro_rules! initial_grid_fetch {
    ($(#[$doc:meta])* $name:ident, $module:ident, $handle:ty, $label:literal) => {
        $(#[$doc])*
        pub fn $name(state: &AppState, handle: &Arc<$handle>, weak: slint::Weak<AppWindow>) {
            let s = state.clone();
            let h = handle.clone();
            state.runtime.spawn(async move {
                if let Err(e) = ui::$module::fetch_grid(&s, &h, weak).await {
                    log::warn!("initial {} fetch: {e}", $label);
                }
            });
        }
    };
}

initial_grid_fetch!(
    /// Kick off the initial Albums grid fetch.
    spawn_initial_albums_fetch,
    albums,
    ui::albums::AlbumsUi,
    "albums"
);
initial_grid_fetch!(
    /// Kick off the initial Artists grid fetch.
    spawn_initial_artists_fetch,
    artists,
    ui::artists::ArtistsUi,
    "artists"
);
initial_grid_fetch!(
    /// Kick off the initial Genres grid fetch.
    spawn_initial_genres_fetch,
    genres,
    ui::genres::GenresUi,
    "genres"
);
initial_grid_fetch!(
    /// Kick off the initial Playlists grid fetch.
    spawn_initial_playlists_fetch,
    playlists,
    ui::playlists::PlaylistsUi,
    "playlists"
);

/// Subscribe to `library_changed_tx` and bump `Tracks.invoke_request_refresh`
/// on every mutation so the Tracks view stays in sync with scans / watcher
/// batches. The initial `0` is not observed — `changed()` only resolves on
/// a real `send_modify`, so this does not race the explicit initial fetch.
///
/// Gated on section visibility: play-count flushes bump this channel after
/// every track completion, so an ungated refresh would re-fetch the whole
/// library (full 19-col SELECT + search-key rebuild + re-sort) per song
/// during plain listening, even with the view hidden. While hidden the bump
/// is folded into the `TracksUi` dirty flag; `Tracks.section-active-changed`
/// runs one deferred refresh on re-enter.
pub fn install_library_changed_refresher(
    state: &AppState,
    tracks_ui: &Arc<ui::tracks::TracksUi>,
    weak: slint::Weak<AppWindow>,
) -> Result<(), melodia::error::AppError> {
    let mut rx = state.library_changed_tx.subscribe();
    let tu = tracks_ui.clone();
    slint::spawn_local(async_compat::Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let _ = rx.borrow_and_update();
            if !tu.section_active() {
                tu.mark_dirty();
                continue;
            }
            let Some(ui) = weak.upgrade() else { break };
            ui.global::<melodia::Tracks>().invoke_request_refresh();
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia::error::AppError::Window(format!("library-changed subscriber: {e}")))
}

/// Subscribe to `rescan_notice_tx` and push a transient toast through the
/// notifications stack on every kernel-overflow rescan. Lives on the UI
/// thread so it can hold the `Rc<NotificationsUi>` (not `Send`) and
/// resolve translation strings through `Settings.invoke_library_resyncing_*`
/// at push time — the message lands in whichever locale was active when
/// the rescan fired. Coalesced upstream by `watch`'s slot semantics and
/// by `RECONCILE_IN_FLIGHT`, so a burst of overflows still paints at most
/// one toast per reconcile cycle.
pub fn install_rescan_notice_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia::error::AppError> {
    use melodia::Settings;
    use ui::shell::notifications::NotificationParams;

    let mut rx = state.rescan_notice_tx.subscribe();
    slint::spawn_local(async_compat::Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let _ = rx.borrow_and_update();
            let Some(ui) = weak.upgrade() else { break };
            let g = ui.global::<Settings>();
            notifications.show(NotificationParams {
                variant: "info".into(),
                title: g.invoke_library_resyncing_title(),
                message: g.invoke_library_resyncing_message(),
                action_label: slint::SharedString::default(),
                action_kind: "library-resyncing".into(),
            });
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia::error::AppError::Window(format!("rescan-notice subscriber: {e}")))
}

/// Subscribe to `audio_device_lost_tx` and push a sticky warning toast when the
/// output device goes away. Sticky for the crash notice's reason: nothing else
/// reports this and playback carries on regardless, so a notice the user looked
/// away for did nothing. No action button — there is no device picker to send
/// them to — so the kind only groups the row for `dismiss_by_kind`.
pub fn install_audio_device_lost_subscriber(
    state: &AppState,
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia::error::AppError> {
    use melodia::Settings;
    use ui::shell::notifications::NotificationParams;

    let mut rx = state.audio_device_lost_tx.subscribe();
    slint::spawn_local(async_compat::Compat::new(async move {
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let _ = rx.borrow_and_update();
            let Some(ui) = weak.upgrade() else { break };
            let g = ui.global::<Settings>();
            notifications.show(NotificationParams {
                variant: "warning".into(),
                title: g.invoke_audio_device_lost_title(),
                message: g.invoke_audio_device_lost_message(),
                action_label: slint::SharedString::default(),
                action_kind: "audio-device-lost".into(),
            });
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia::error::AppError::Window(format!("audio-device-lost subscriber: {e}")))
}

/// Drain the process-wide `services::toast` channel on the UI thread and render
/// each backend-failure as an error toast. Mirrors
/// [`install_rescan_notice_subscriber`] but consumes an `mpsc` (errors must not
/// coalesce like a `watch` slot would) and resolves the localized title by
/// toast kind at push time — so a failure that fires on a tokio worker still
/// paints in whichever locale is active when it surfaces. The dynamic detail
/// (a path or error message) is shown verbatim as the toast body — except for
/// `RestartRequired`, which carries none and takes its body from `Settings` too.
pub fn install_toast_bridge(
    weak: slint::Weak<AppWindow>,
    notifications: std::rc::Rc<ui::shell::notifications::NotificationsUi>,
) -> Result<(), melodia::error::AppError> {
    use melodia::Settings;
    use melodia::services::toast::{self, ToastKind, ToastRequest};
    use ui::shell::notifications::NotificationParams;

    // First installer owns delivery; a second call (shouldn't happen) is a no-op.
    let Some(mut rx) = toast::init() else {
        return Ok(());
    };
    slint::spawn_local(async_compat::Compat::new(async move {
        while let Some(ToastRequest { kind, detail }) = rx.recv().await {
            let Some(ui) = weak.upgrade() else { break };
            let g = ui.global::<Settings>();
            match kind {
                ToastKind::PlaybackFailed | ToastKind::OperationFailed => {
                    let title = match kind {
                        ToastKind::PlaybackFailed => g.invoke_toast_playback_error_title(),
                        _ => g.invoke_toast_operation_failed_title(),
                    };
                    notifications.show(NotificationParams {
                        variant: "error".into(),
                        title,
                        message: detail.into(),
                        action_label: slint::SharedString::default(),
                        action_kind: "error".into(),
                    });
                }
                // Informational result of a user-triggered MBID sweep — transient,
                // so it auto-dismisses rather than sticking like a failure.
                ToastKind::MbidTagging => {
                    notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: "info".into(),
                            title: g.invoke_toast_mbid_title(),
                            message: detail.into(),
                            action_label: slint::SharedString::default(),
                            action_kind: "info".into(),
                        },
                        6000,
                    );
                }
                // A restart that had nowhere to relaunch from. Both strings are
                // localized — there is no dynamic detail — and it sticks,
                // because it asks the user to do something rather than
                // reporting what happened.
                ToastKind::RestartRequired => {
                    notifications.show(NotificationParams {
                        variant: "warning".into(),
                        title: g.invoke_toast_restart_required_title(),
                        message: g.invoke_toast_restart_required_message(),
                        action_label: slint::SharedString::default(),
                        action_kind: "warning".into(),
                    });
                }
                // Informational result of a retroactive loved-tracks backfill.
                ToastKind::LoveSync => {
                    notifications.show_auto_dismiss(
                        NotificationParams {
                            variant: "info".into(),
                            title: g.invoke_toast_love_sync_title(),
                            message: detail.into(),
                            action_label: slint::SharedString::default(),
                            action_kind: "info".into(),
                        },
                        6000,
                    );
                }
            }
        }
    }))
    .map(|_| ())
    .map_err(|e| melodia::error::AppError::Window(format!("toast bridge: {e}")))
}

#[cfg(test)]
#[path = "tests/ui_setup_tests.rs"]
mod tests;
