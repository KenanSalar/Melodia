//! UI installation phase: locale, chrome, per-view installers, settings
//! hydration, initial fetches, and the library-changed refresher.

use std::sync::Arc;

use melodia::{
    AppWindow, ArtistDetail, HeroBackdrop, Nav, Player, Theme, media, services, state::AppState, ui,
};
use slint::ComponentHandle;

/// Hydrate Slint's bundled-translation runtime from the persisted
/// `settings.locale` *before* `app.run()`, so the very first frame's `@tr(...)`
/// resolutions land in the chosen language, then wire the Language section.
pub fn install_locale(
    app: &AppWindow,
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let persisted_locale = startup_settings.map_or_else(|| "en".to_owned(), |s| s.locale.clone());
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

/// Raise the persisted backdrop choice, and do it **before `install_views`**.
///
/// The flag has to be up before the first tier exists, not merely before `app.show()`:
/// `install_views` constructs the three `DetailArtwork` caches — whose blur half is built or
/// skipped on this answer — and then seeds all four detail views, whose fetches end in
/// `ui::backdrop::kind`. The two obvious homes, `ui::appearance::install` and
/// `hydrate_ui_from_settings`, both run after it. A failed settings read leaves the
/// Slint-declared default, which is the same value.
///
/// Returns whether the aurora arm is live, read back off the property so the failed-read arm
/// answers with what boot raised — the writer reporting what it wrote, not a second reader.
/// `#[must_use]` because dropping the answer is the live mutation: the dither install downstream is
/// the aurora's alone, and a bare call statement would compile clean past it.
#[must_use]
pub fn apply_backdrop_style(
    app: &AppWindow,
    startup_settings: Option<&services::settings::SettingsData>,
) -> bool {
    let theme = app.global::<Theme>();
    if let Some(settings) = startup_settings {
        theme.set_aurora_backdrop(settings.backdrop.aurora_backdrop);
    }
    theme.get_aurora_backdrop()
}

/// One tile for the process, shared by both aurora tiers: it answers to the renderer's lack of
/// gradient dithering rather than to any artwork, so nothing later rewrites it. The `Image` is
/// `Rc`-backed, so the second global clones a handle, not a buffer.
///
/// **The aurora arm's alone** — on the blur arm it is a generator run and a buffer nothing draws,
/// and skipping it leaves the unset `image` `AuroraBackdrop` already degrades to.
pub fn install_backdrop_dither(app: &AppWindow) {
    let tile = slint::Image::from_rgba8(ui::aurora::dither_tile());
    app.global::<Player>().set_np_dither(tile.clone());
    app.global::<HeroBackdrop>().set_dither(tile);
}

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
    pub cover_thumbs: Arc<media::cover_thumbs::CoverThumbs>,
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
fn install_row_covers(app: &AppWindow, cover_thumbs: &Arc<media::cover_thumbs::CoverThumbs>) {
    let ct = cover_thumbs.clone();
    // `generation` is read for its effect on the binding, never its value — see `RowCovers`.
    app.global::<melodia::RowCovers>().on_request(move |path, _generation| {
        ct.get_or_schedule_opt(ui::grid_prewarm::nonempty_artwork_path(path.as_str()))
    });

    ui::cover_generation::notify_on_decode(cover_thumbs, app, |app| {
        let covers = app.global::<melodia::RowCovers>();
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
            state.radio_enabled(),
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
    let cover_thumbs = Arc::new(media::cover_thumbs::CoverThumbs::new());
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
    *state.ui_handles.albums.lock() = Some(albums_ui.clone());
    *state.ui_handles.artists.lock() = Some(artists_ui.clone());
    *state.ui_handles.genres.lock() = Some(genres_ui.clone());
    *state.ui_handles.playlists.lock() = Some(playlists_ui.clone());

    // The four persisted-detail reopens stay adjacent here rather than folding
    // into each slice's `install`, because the history seed below depends on
    // none of them having landed yet. The tabbed pages' tab seeds *did* fold
    // in, their handle being the receiver.
    ui::albums::seed_detail_from_settings(app, state, &albums_ui);
    ui::artists::seed_detail_from_settings(app, state, &artists_ui);
    ui::genres::seed_detail_from_settings(app, state, &genres_ui);
    ui::playlists::seed_detail_from_settings(app, state, &playlists_ui);

    // Seed the nav-history with the boot view, so Mouse-4 has a target after the
    // first sidebar click — otherwise a boot landing on a section with no
    // persisted detail records only the destination and `back()` returns `None`.
    // Reads the detail global while it is still `-1`; the async
    // `seed_detail_from_settings` future appends its own entry on top once it
    // lands.
    ui::nav_history::record_current(state, app);

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
        tune_rows.set_thumb_size(media::cover_thumbs::row_cover_size(f64::from(
            app.window().scale_factor(),
        )));
    };
    // **And again on every resize.** Both answers move with the window, and a cap read once at
    // launch is the cap a later maximize overruns — the cards past it can only paint
    // placeholders, the lookup behind them scheduling against a tier that cannot hold them.
    // Setting the cap exactly rather than growing it: a smaller window really does draw fewer
    // cards, and a drag that oscillates re-warms through the model rebuild it triggers anyway.
    app.global::<melodia::WindowChrome>().on_display_changed(retune.clone());
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
    ui::settings::radio_settings::install_radio(app, state);
    let notifications = ui::shell::notifications::install(app);
    ui::settings::file_watching::install(app, state, &notifications);
    ui::settings::updater_settings::install(app, state);
    ui::settings::motion::install(app, state);
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
    cover_thumbs: &Arc<media::cover_thumbs::CoverThumbs>,
) {
    use melodia::player::state::lock_state;

    let s = lock_state(&state.player_state);
    let light = s.to_view_model_light();
    let qvm = s.to_queue_view_model();
    drop(s);

    let player = app.global::<Player>();
    player.set_vm(ui::shell::bridge::to_slint_player_vm(&light, cover_thumbs));
    // Seed the position scalars too, so a restored session shows its saved
    // position before the first playback-monitor tick lands.
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

    // The row tier is empty this early, so the seed above is guaranteed to be the cache-only
    // lookup's miss — and no `view_model` push is owed on a restored-but-paused session, so
    // nothing else would ever fill it.
    ui::shell::bridge::warm_vm_cover(
        app.as_weak(),
        &state.runtime,
        cover_thumbs,
        light.current_track.as_ref().and_then(|t| t.artwork_path.clone()).unwrap_or_default(),
    );
}

/// Apply every UI-visible persisted section to the Slint globals — sidebar
/// geometry from `settings.json`, per-view columns and collapse state from
/// `views.json`. Missing entries leave the Slint defaults, which is first-launch
/// behaviour. A `None` snapshot re-reads from disk; `main()` passes what it
/// already read to avoid a second parse.
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
    apply_startup_animation_suppression(app, settings);
    ui::track_list_view::hydrate_tracks_view(app, vs);
    ui::track_list_view::hydrate_browse_view(app, vs);
    ui::track_list_view::hydrate_album_detail_view(app, vs);
    ui::track_list_view::hydrate_artist_detail_view(app, vs);
    ui::track_list_view::hydrate_genre_detail_view(app, vs);
    ui::track_list_view::hydrate_playlist_detail_view(app, vs);
    ui::track_list_view::hydrate_favorites_view(app, vs);
    ui::track_list_view::hydrate_recently_played_view(app, vs);
    ui::track_list_view::hydrate_search_view(app, vs);
    app.global::<ArtistDetail>().set_albums_collapsed(vs.artist_albums_collapsed);
    ui::settings::settings_page::seed_tab(app, vs.settings_tab);
}

/// `sidebar.slint` already clamps `Nav.sidebar-width` at the use site, so no
/// Rust-side clamp is needed.
fn apply_sidebar_width(app: &AppWindow, settings: &services::settings::SettingsData) {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "sidebar widths are small pixel counts; f32 precision is sufficient"
    )]
    let width = settings.sidebar_width as f32;
    app.global::<Nav>().set_sidebar_width(width);
}

fn apply_sidebar_collapsed(app: &AppWindow, settings: &services::settings::SettingsData) {
    app.global::<Nav>().set_sidebar_collapsed(settings.layout.sidebar_collapsed);
}

/// Raise the entrance-animation suppression for the launch mount. The flag only
/// has to be up before the first painted frame, and `ViewTransition` reads it
/// live and hands it back once settled — so this reaches exactly the view the
/// window opens on. A failed settings read leaves it down and the animation plays.
fn apply_startup_animation_suppression(
    app: &AppWindow,
    settings: &services::settings::SettingsData,
) {
    app.global::<Nav>().set_suppress_enter_animation(settings.motion.skip_startup_animation);
}

/// Kick off the initial Tracks fetch so the list is populated by the time the
/// user navigates to it. A fresh install falls back to title ascending, matching
/// the `Tracks` global default and the header `ui::tracks::install` seeds.
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

/// Kick off an entity grid's initial fetch so its cards are populated by the
/// time the user navigates to it.
///
/// A macro rather than a generic `fn` for the reason `impl_detail_view_helpers!`
/// is one: the four `*Ui` types share no trait, and each `fetch_grid` is a free
/// function in its own module. Tracks is deliberately not among them, resolving a
/// persisted sort and calling `fetch_and_apply` instead.
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

/// Keep the Tracks view in sync with scans and watcher batches. The initial `0`
/// isn't observed — `changed()` only resolves on a real `send_modify` — so this
/// doesn't race the explicit initial fetch.
///
/// Gated on section visibility, because play-count flushes bump this channel
/// after every track completion: ungated, plain listening re-fetches the whole
/// library once per song with the view hidden. While hidden the bump folds into
/// the `TracksUi` dirty flag and the section gate runs one refresh on re-enter.
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

/// Toast on every kernel-overflow rescan. On the UI thread so it can hold the
/// non-`Send` `Rc<NotificationsUi>` and resolve its strings at push time, in
/// whichever locale was active when the rescan fired. Coalesced upstream by the
/// `watch` slot and by `RECONCILE_IN_FLIGHT`, so a burst of overflows still
/// paints at most one toast per reconcile cycle.
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

/// Sticky warning toast when the output device goes away — sticky for the crash
/// notice's reason: nothing else reports this and playback carries on
/// regardless, so a notice the user looked away for did nothing. No action
/// button, there being no device picker to send them to, so the kind only groups
/// the row for `dismiss_by_kind`.
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

/// Drain the process-wide `services::toast` channel on the UI thread.
///
/// [`install_rescan_notice_subscriber`]'s shape over an `mpsc` rather than a
/// `watch` — errors must not coalesce — resolving the localized title by kind at
/// push time so a failure raised on a tokio worker paints in the locale that is
/// active when it surfaces. The dynamic detail is shown verbatim.
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
                // The result of a user-triggered sweep, so it auto-dismisses
                // rather than sticking like a failure.
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
                // A restart that had nowhere to relaunch from: no dynamic
                // detail, and it sticks because it asks the user to do
                // something rather than reporting what happened.
                ToastKind::RestartRequired => {
                    notifications.show(NotificationParams {
                        variant: "warning".into(),
                        title: g.invoke_toast_restart_required_title(),
                        message: g.invoke_toast_restart_required_message(),
                        action_label: slint::SharedString::default(),
                        action_kind: "warning".into(),
                    });
                }
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
