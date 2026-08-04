// Suppress the auto-allocated console window on Windows release builds.
// Without this attribute, rustc defaults to the `console` subsystem on
// Windows: the OS allocates a console alongside the GUI window and ties
// the app's lifetime to it (closing the console terminates Melodia). The
// `cfg_attr(not(debug_assertions))` form keeps the console available
// during `cargo run` so `RUST_LOG` / `MELODIA_RSS_SAMPLE` output stays
// visible for development; only release builds become true GUI apps.
//
// Doesn't affect stdout capture by parent processes — the updater's
// post-install smoke test spawns this binary with `Stdio::piped()`,
// which works for GUI-subsystem children just as it does for console
// ones (the pipe handle is inherited regardless of subsystem).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod boot;
mod shutdown;

use std::sync::Arc;

use melodia::{
    AppWindow,
    config::Paths,
    error::{AppError, AppResult},
    library, services,
    state::AppState,
    tasks, ui,
};
use slint::ComponentHandle;
use tokio::sync::watch;

fn main() -> AppResult<()> {
    // `--version` smoke-test fast path. The in-app updater's post-swap
    // verifier (`services::updater::install::verify_swapped_binary`)
    // spawns `<new_binary> --version` after a successful rename to
    // confirm the freshly-installed binary actually boots before the
    // user clicks Restart. This must:
    //
    //   * Print *something* to stdout (the verifier asserts non-empty
    //     stdout to defend against a degenerate "exits 0 but prints
    //     nothing" binary).
    //   * Return promptly — the verifier's timeout is 3 s.
    //   * Stay in this function forever as a forward-compatibility
    //     contract. Removing or breaking this branch in a future
    //     release would break in-place updates for every older client
    //     that runs the smoke test against it.
    //
    // Runs before `mallopt` because (a) the path exits in microseconds
    // so arena overhead is irrelevant, (b) we want minimum latency for
    // the verifier, and (c) `mallopt` only matters for steady-state
    // resident memory of long-lived processes.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        use std::io::Write;
        // `writeln!` on a locked stdout handle dodges
        // `clippy::print_stdout` — the lint guards against accidental
        // stdout from a GUI app; this branch is a deliberate CLI
        // contract. Errors here are swallowed: a broken stdout
        // (pipe closed before we could write) still means we should
        // exit 0 — the *binary works*, which is the smoke test's only
        // question. The verifier checks both `status.success()` AND
        // non-empty stdout, so a write failure here would correctly
        // be reported as a smoke-test failure (empty stdout → fail).
        let _ = writeln!(
            std::io::stdout().lock(),
            "Melodia {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    // Reap `<install_target>.old` — the previous-binary snapshot left by
    // [`services::updater::install::swap_in_place`] on AppImage / tarball
    // installs (`Melodia.old` / `<AppImage-basename>.old`). The
    // atomic-swap path keeps `.old` around so a failed post-swap smoke
    // test can roll back; on the next successful launch we know the new
    // binary works and the `.old` copy is dead weight. Linux-only:
    // Windows installs flow through an MSI which is replaced by msiexec
    // major-upgrade, not an in-process binary swap, so no `.old` ever
    // exists at the install target there.
    #[cfg(target_os = "linux")]
    {
        if let Ok(stale) = melodia::services::updater::install_target_old() {
            let _ = std::fs::remove_file(stale);
        }
    }
    // Cap glibc's per-thread malloc arenas to 2. The default `8 × num_cpus`
    // gives every long-lived thread its own 64 MiB virtual arena, and this
    // process runs enough of them (album-art prewarm, queue restore, Slint,
    // souvlaki, SQLx) that the committed slack across those arenas is pure
    // per-thread free-list overhead. Capping forces threads to share, trading
    // it for malloc contention under heavy parallel allocation — which an
    // idle-most-of-the-time desktop player doesn't have. Must run before any
    // thread does its first malloc; `env_logger::init()` and the tokio
    // runtime builder both allocate, so staying first in `main()` covers it.
    //
    // The other two calls freeze the mmap and trim thresholds, which glibc
    // otherwise moves on its own: freeing an mmap'd block raises the mmap
    // threshold to that block's size and the trim threshold to twice it. One
    // large short-lived allocation — a full-resolution cover decode, say — is
    // enough to leave the threshold above every later one, and those then come
    // off the arena free list instead of mmap, where freeing them hands nothing
    // back to the kernel. Setting either parameter explicitly disables the
    // adjustment; the values below are glibc's own initial ones, so this pins
    // the behaviour the process starts with rather than tuning it. The trade is
    // more minor faults (every allocation past the threshold is a fresh mmap)
    // for less resident anonymous memory, which is the direction this app wants.
    //
    // `M_TRIM_THRESHOLD = -1`, `M_MMAP_THRESHOLD = -3` and `M_ARENA_MAX = -8`
    // per glibc's `malloc.h`.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[allow(
        unsafe_code,
        reason = "FFI to glibc mallopt with constant args; no thread safety concerns before runtime spawn"
    )]
    unsafe {
        libc::mallopt(-8, 2);
        libc::mallopt(-3, 128 * 1024);
        libc::mallopt(-1, 128 * 1024);
    }

    // Give PipeWire's ALSA-compat layer a clean stream name. CPAL (via
    // Rodio) opens the default ALSA device; under PipeWire that PCM
    // becomes a graph node auto-named `alsa_playback.<prgname>`, which
    // EasyEffects / pavucontrol show verbatim. `PIPEWIRE_ALSA` is read by
    // pipewire-alsa when the PCM is opened (it accepts SPA-JSON
    // `alsa.properties` / `stream.properties`); setting `node.name`
    // overrides the auto-name and `application.name` fills the app-name
    // column those mixers display, so the stream reads simply "Melodia".
    // No-op on bare ALSA (the real plugin ignores it) and on non-PipeWire
    // systems. Must be set before any thread spawns — both for the unsafe
    // `set_var` soundness and so the audio device (opened later in
    // `AppState::init`) inherits it.
    #[cfg(target_os = "linux")]
    #[allow(
        unsafe_code,
        reason = "set_var before any thread spawns; main() is single-threaded here"
    )]
    unsafe {
        std::env::set_var(
            "PIPEWIRE_ALSA",
            "{ application.name = \"Melodia\" node.name = \"Melodia\" }",
        );
    }

    env_logger::init();
    log::info!("Melodia starting");

    // Cap worker threads at 2. Melodia's async work is event-driven (DB
    // queries, watch publishes, position ticks, souvlaki events) and rarely
    // CPU-bound — CPU-bound work goes through Rayon (file scan, cover thumb
    // prewarm) or `spawn_blocking` (Material You), neither of which counts
    // against the worker pool. Default `num_cpus` workers leaves 6+ idle
    // threads on a typical desktop, each with a 2 MB stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("melodia-bg")
        .build()
        .map_err(|e| AppError::Settings(format!("tokio runtime: {e}")))?;

    // Slint's a11y/D-Bus thread looks up a tokio reactor from UI-thread tasks,
    // so the guard has to stay alive for the entire `app.run()` window.
    let runtime_guard = runtime.enter();

    let paths = Paths::resolve()?;
    let (state, channels) =
        runtime.block_on(AppState::init(paths, runtime.handle().clone()))?;

    log::info!(
        "AppState initialized: db ok, watcher built, media controls = {}",
        if state.media_controls.is_some() {
            "ready"
        } else {
            "no-op"
        }
    );

    // Unified task lifecycle handle.
    let spawner = tasks::TaskSpawner::from_state(&state);

    // 1, 2. Always-running background tasks + souvlaki receiver.
    boot::tasks::spawn_background_tasks(&spawner, &state, channels);

    // 3. Restore persisted queue from disk.
    boot::tasks::restore_persisted_queue(&runtime, &state);

    // Read `settings.json` and `views.json` once and reuse them everywhere.
    let startup_settings: Option<services::settings::SettingsData> =
        library::settings::get_settings(&state).ok();
    let startup_view_state: Option<services::view_state::ViewStateData> =
        library::settings::get_view_state(&state).ok();

    // 3a. Resume on Startup (iff the flag is on AND a current track exists).
    boot::tasks::maybe_resume_on_startup(&state, startup_settings.as_ref());

    // 4. First-launch auto-add + folder-watcher restart.
    boot::tasks::spawn_first_launch(&spawner, &state);

    // 4-pre. Persisted window geometry. Maximized state is restored via
    // the winit `WindowAttributes` hook (applied during
    // `AppWindow::new()`) — there is no `slint::Window` API for it, and
    // the hook creates the window already-maximized with no visible
    // flash. Size + position are restored *after* `AppWindow::new()`
    // via `geometry::restore` — see that module for why `set_size` is
    // required instead of the hook. `fallback()` covers a first launch
    // where `settings.json` doesn't exist yet.
    let geometry = startup_settings.as_ref().map_or_else(
        ui::window_chrome::geometry::PersistedGeometry::fallback,
        ui::window_chrome::geometry::PersistedGeometry::from_settings,
    );
    let restore_maximized = geometry.maximized;
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(move |attrs| {
            let attrs = if restore_maximized {
                attrs.with_maximized(true)
            } else {
                attrs
            };
            // Pin the window identity so the compositor resolves our
            // icon and label. The two protocols match differently:
            //
            //   * X11: `WM_CLASS` res_class "Melodia" is matched against
            //     the `.desktop` file's `StartupWMClass=Melodia`. Without
            //     it winit leaves the class empty and the compositor falls
            //     back to the binary basename (lowercase `melodia` for the
            //     RPM/DEB `/usr/bin/melodia`, `Melodia` for `cargo run`).
            //   * Wayland: clients cannot set a window icon at all — the
            //     compositor finds it by matching the `app_id` to a
            //     `.desktop` file of the *same basename*, then reads its
            //     `Icon=`. `StartupWMClass` is not consulted on Wayland.
            //     Our desktop file installs as
            //     `com.github.kenansalar.melodia.desktop`
            //     (see `services::desktop_integration`), so the `app_id`
            //     must be that reverse-DNS id — not "Melodia" — or KWin
            //     shows the generic Wayland placeholder in Alt+Tab.
            #[cfg(target_os = "linux")]
            let attrs = {
                use slint::winit_030::winit::platform::wayland::WindowAttributesExtWayland;
                use slint::winit_030::winit::platform::x11::WindowAttributesExtX11;
                let attrs = WindowAttributesExtX11::with_name(attrs, "Melodia", "Melodia");
                WindowAttributesExtWayland::with_name(
                    attrs,
                    "com.github.kenansalar.melodia",
                    "com.github.kenansalar.melodia",
                )
            };
            attrs
        })
        .select()
        .map_err(|e| AppError::Window(format!("backend selector: {e}")))?;

    let app = AppWindow::new().map_err(|e| AppError::Window(e.to_string()))?;

    // Restore window size + position. Must run after `AppWindow::new()`
    // (window adapter exists) and before `app.run()` (window not yet
    // shown). `set_size` sets the winit backend's `has_explicit_size`
    // flag, which suppresses Slint's content-preferred-size resize on
    // first show — without it the window snaps to the ~640×420 layout
    // minimum regardless of what the WM placed it at.
    ui::window_chrome::geometry::restore(&app, geometry);

    // 4a. Locale.
    boot::ui_setup::install_locale(&app, &state, startup_settings.as_ref());

    // 4b. Window chrome (no-frame / drag region / native-frame hydrate).
    boot::ui_setup::install_app_chrome(&app, &state);

    // 5–5c3. Tracks / Browse / Albums views + their callbacks + the
    // now-playing-favorite fan-out + album cover-cache tune. Returns the
    // per-view handles for downstream wiring.
    let views =
        boot::ui_setup::install_views(&app, &state, startup_view_state.as_ref());

    // 5d–5d4. Library Settings + playback toggle + notifications stack +
    // file-watcher toggle.
    let notifications = boot::ui_setup::install_library_settings_and_friends(&app, &state)?;

    // 5d5. Playlist import/export (M3U8) header pills. Wired here — after
    // both the playlists UI handle and the notifications stack exist —
    // because the completion toasts need the `Rc<NotificationsUi>`.
    ui::callbacks::wire_playlist_files(&app, &state, &views.playlists_ui, &notifications);

    // 5d6. Edit-Track-Information dialog callbacks. Wired here for the same
    // reason as the playlist pills — the Save completion toast needs the
    // `Rc<NotificationsUi>`.
    ui::callbacks::wire_tags(&app, &state, &notifications);

    // 5e. Appearance.
    let appearance_handles = match ui::appearance::install(&app, &state) {
        Ok(h) => Some(h),
        Err(e) => {
            log::warn!("appearance::install: {e}");
            None
        }
    };

    // 5f. Material You coordinator (subscribes to the player view-model +
    // the appearance kick channel). Spawned after `appearance::install`
    // so the kick channel exists.
    if let Some(h) = &appearance_handles {
        tasks::material_you::spawn(
            &spawner,
            state.clone(),
            h.os_state.clone(),
            state.sinks.view_model.subscribe(),
            h.kick_tx.subscribe(),
            h.repaint_tx.clone(),
            views.cover_thumbs.clone(),
        );
    }

    // 5c–5e. Apply persisted UI state (column visibility, sidebar width,
    // column widths).
    boot::ui_setup::hydrate_ui_from_settings(
        &app,
        &state,
        startup_settings.as_ref(),
        startup_view_state.as_ref(),
    );

    // 6. Bridge subscribers: ViewModel / queue / position channels.
    let weak = app.as_weak();
    ui::bridge::spawn_view_model_subscriber(
        weak.clone(),
        &state.sinks,
        views.cover_thumbs.clone(),
    )
    .map_err(|e| AppError::Window(format!("view-model subscriber: {e}")))?;
    ui::bridge::spawn_queue_subscriber(weak.clone(), &state.sinks)
        .map_err(|e| AppError::Window(format!("queue subscriber: {e}")))?;
    ui::bridge::spawn_position_subscriber(weak.clone(), &state.position_tx)
        .map_err(|e| AppError::Window(format!("position subscriber: {e}")))?;

    // 6b. Queue bottom-sheet.
    match ui::queue_sheet::install(&app, &state) {
        Ok(h) => ui::window_chrome::set_queue_sheet_open(h.is_open),
        Err(e) => log::warn!("queue_sheet::install: {e}"),
    }

    // 6c. Full-screen Now Playing view (owns its own small `(cover, blur)`
    // LRU separate from `cover_thumbs`).
    let np_artwork = Arc::new(ui::now_playing_artwork::NowPlayingArtwork::new());
    let np_state = match ui::now_playing::install(&app, &state, &views.cover_thumbs, &np_artwork)
    {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("now_playing::install: {e}");
            None
        }
    };
    // Miniplayer wiring depends on `np_state` so the up-next subscriber
    // gate can flip on/off as the responsive mini state changes. Skip
    // if `now_playing::install` failed — without it the gate would
    // never affect anything visible.
    if let Some(ref np_state) = np_state
        && let Err(e) = ui::mini_player::install(&app, &state, &np_artwork, np_state)
    {
        log::warn!("mini_player::install: {e}");
    }

    // 7. Seed `Player.vm` / `Player.queue` once with the current state.
    boot::ui_setup::seed_initial_view_model(&app, &state, &views.cover_thumbs);

    // 8, 8b, 8c, 8d, 8e. Initial Tracks + Albums + Artists + Genres +
    // Playlists fetches.
    boot::ui_setup::spawn_initial_tracks_fetch(&state, &views.tracks_ui, weak.clone());
    boot::ui_setup::spawn_initial_albums_fetch(&state, &views.albums_ui, weak.clone());
    boot::ui_setup::spawn_initial_artists_fetch(&state, &views.artists_ui, weak.clone());
    boot::ui_setup::spawn_initial_genres_fetch(&state, &views.genres_ui, weak.clone());
    boot::ui_setup::spawn_initial_playlists_fetch(&state, &views.playlists_ui, weak.clone());

    // 9. Re-fetch Tracks whenever the library mutates (deferred to the next
    // section-enter while the view is hidden).
    boot::ui_setup::install_library_changed_refresher(&state, &views.tracks_ui, weak.clone())?;

    // 9b. Toast on watcher-overflow rescan (kernel queue dropped events).
    boot::ui_setup::install_rescan_notice_subscriber(&state, weak.clone(), notifications.clone())?;

    // 9c. Surface backend failures (playback decode errors, failed scans /
    // imports / saves) pushed through the `services::toast` bridge as toasts.
    boot::ui_setup::install_toast_bridge(weak.clone(), notifications.clone())?;

    // 10. Opt-in memory sampler (`MELODIA_RSS_SAMPLE=1`). No-op when unset.
    // Lives on the UI thread so it can read the Nav / *Detail globals for
    // the view-tag annotation without an atomic-shadow plumbing pass.
    tasks::rss_sampler::install(&weak);

    // 11. Auto-updater wiring. One `watch` channel carries backend
    // events (Available / Installed / Failed) from the daily-check task
    // + Updater.* callbacks to the UI-thread subscriber that pushes
    // notifications. The daily task only spawns when
    // `auto_check_enabled` is on AND the install path is user-writable
    // (system-managed installs flow through the OS package manager).
    let (updater_event_tx, updater_event_rx) =
        watch::channel::<Option<services::updater::UpdaterEvent>>(None);
    ui::updater_settings::install_event_subscriber(
        weak.clone(),
        notifications.clone(),
        updater_event_rx,
    );
    ui::callbacks::wire_updater(&app, &state, &notifications, &updater_event_tx);

    let updater_settings_snapshot = startup_settings
        .as_ref()
        .map(|s| s.updates.clone())
        .unwrap_or_default();
    if updater_settings_snapshot.auto_check_enabled
        && !services::updater::is_system_install()
    {
        tasks::updater_daily::spawn(
            &spawner,
            state.clone(),
            weak.clone(),
            updater_event_tx,
        );
    } else {
        log::info!(
            "updater_daily: not spawning (auto_check_enabled={}, system_managed={})",
            updater_settings_snapshot.auto_check_enabled,
            services::updater::is_system_install()
        );
    }

    // Independent of the daily-check task: reap stale staged update
    // artifacts that a user-cancelled / failed install left behind in
    // `~/.cache/Melodia/update-staging/`. Without this one-shot at
    // boot, a user who never clicks "Install" again would let a
    // verified .rpm/.deb sit on disk forever (the in-attempt prune
    // only fires on the next install click).
    //
    // 30 s startup grace matches `updater_daily::STARTUP_DELAY` — the
    // first-launch folder scan + DB pre-fetch should have first claim
    // on the disk before we add staging-dir I/O.
    runtime.spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        services::updater::prune_stale_staging().await;
    });

    // Tarball installs only: re-deploy `.desktop` + icon if our
    // compiled-in payloads differ from what's on disk. RPM/DEB
    // packages own those files via their `%files` manifest; AppImage
    // bundles them inside. Cheap BLAKE3 compare per file means the
    // common case (already up to date) is one stat + 3 KB hash and
    // exits without touching the disk. See `desktop_integration.rs`
    // for the full gating rationale.
    #[cfg(target_os = "linux")]
    runtime.spawn_blocking(|| {
        if let Err(e) = services::desktop_integration::refresh_user_install() {
            log::warn!("desktop_integration: refresh failed: {e}");
        }
    });

    // System tray. Opt-in via `settings.tray.tray_enabled` (off by default) —
    // when off, none of the tray code runs: no D-Bus connection, no ksni
    // service thread, no action-receiver/subscriber tasks. The setting is
    // restart-gated (the `restart-tray` Dialog flow), so this startup read
    // is the single gate. `install` itself is cross-platform — on Linux
    // ksni's StatusNotifierItem is created eagerly (its own D-Bus thread);
    // on Windows / macOS `tray-icon` creation is deferred onto the event
    // loop (it needs the loop running).
    if startup_settings.as_ref().is_some_and(|s| s.tray.tray_enabled) {
        ui::tray_bridge::install(&spawner, &state, &app);
    }

    // Windows: SMTC needs a real `HWND`, which only exists once the OS
    // window is shown. `AppState::init` left the media-controls handle
    // inert (souvlaki panics on a null `HWND`) — attach it now. The
    // closure is posted to the event loop, so it runs on the first
    // iteration, after the window is created and shown.
    #[cfg(target_os = "windows")]
    if let Some(mc) = state.media_controls.clone() {
        let weak = app.as_weak();
        let player_state = state.player_state.clone();
        let sinks = state.sinks.clone();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            match win32_hwnd(&app) {
                Some(hwnd) => {
                    if mc.attach_smtc(hwnd) {
                        // `sync()` no-op'd every call while the controls were
                        // inert, so the OS panel is still empty. Run a no-op
                        // `with_state_emit` to push the current playback state
                        // through the canonical sync path now.
                        melodia::player::state::with_state_emit(
                            &player_state,
                            &sinks,
                            |_| {},
                        );
                    }
                }
                None => {
                    log::warn!("Win32 HWND unavailable after window show; SMTC disabled");
                }
            }
        }) {
            log::warn!("Failed to schedule Windows SMTC attach: {e}");
        }
    }

    // Windows native-titlebar polish: the boot-time `themes::apply` already
    // ran (inside `ui::appearance::install`) but its DWM hook no-op'd
    // because the HWND only exists after `app.show()`. Schedule a one-shot
    // re-apply from the event loop — by the time it fires the window is up,
    // `Theme.mantle` carries the resolved palette, and the caption picks up
    // dark/light + matching mantle colour. Every subsequent palette change
    // drives DWM directly from `themes::apply::write_palette`.
    //
    // Same closure also pushes the embedded EXE icon onto the winit window.
    // `build.rs` (via `winresource`) compiles `assets/melodia.ico` into the
    // EXE under ordinal 1, which the taskbar reads directly — but the
    // *caption* icon comes from the window's WNDCLASS, and winit registers
    // that with `hIcon: 0` (`winit/src/platform_impl/windows/window.rs`).
    // Without an explicit `set_window_icon` the titlebar stays generic.
    #[cfg(target_os = "windows")]
    {
        let weak = app.as_weak();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            let Some(app) = weak.upgrade() else { return };
            install_window_icon(&app);
            melodia::services::dwm_titlebar::reapply_from_theme(&app);
        }) {
            log::warn!("Failed to schedule Windows titlebar polish: {e}");
        }
    }

    // `app.run()` is `show()` + `run_event_loop()`, and `run_event_loop`
    // terminates as soon as the last window is *hidden* — which would quit
    // the process the moment close-to-tray hides the only window. Use
    // `run_event_loop_until_quit` instead: it keeps the loop alive with no
    // visible windows (Slint's documented system-tray pattern) and only
    // returns on an explicit `slint::quit_event_loop()`. Every quit path
    // (titlebar close, native-titlebar `CloseRequested`, tray "Quit",
    // restart) already calls `quit_event_loop()`.
    app.show().map_err(|e| AppError::Window(e.to_string()))?;
    slint::run_event_loop_until_quit().map_err(|e| AppError::Window(e.to_string()))?;

    log::info!("Melodia shutting down — flushing player state");
    shutdown::save_state_on_exit(&app, &state, &runtime);

    // Tear the tray down before `process::exit(0)` skips destructors — a
    // leaked `tray-icon` lingers as a ghost in the Windows notification
    // area. No-op on Linux (the ksni handle is dropped by its subscriber
    // task during `flush_tasks_and_db`). Runs on the main/UI thread, which
    // the `!Send` Windows/macOS tray handle requires.
    ui::tray_bridge::shutdown();

    log::info!("Melodia shutting down — signalling tasks");
    let shutdown_completed = shutdown::flush_tasks_and_db(&runtime, state);
    if shutdown_completed {
        log::info!("All background tasks completed; exiting");
    } else {
        log::warn!(
            "Background shutdown did not finish within 3s — forcing exit. \
             Pending blocking work (scan, retroactive hash, …) is abandoned; \
             persisted state is already flushed by save_state_on_exit."
        );
    }

    // Release the tokio context guard on this thread (it's `!Send` so
    // it can't be moved into the background drop thread).
    drop(runtime_guard);

    shutdown::drop_runtime_in_background(runtime);
    shutdown::respawn_if_requested();

    // Force-terminate. Returning normally from `main()` would let the
    // process linger until every non-daemon thread exits — Rodio's audio
    // output thread (we `Box::leak` the `MixerDeviceSink` so it never
    // `Drop`s), souvlaki's MPRIS / D-Bus thread, accesskit's a11y thread,
    // and any tokio worker still parked on a blocking call all keep the
    // process alive. Persisted state was already flushed; everything left
    // is either OS-managed or ephemeral, so a hard exit is safe here.
    std::process::exit(0);
}

/// Extract the native Win32 `HWND` from the shown Slint window for souvlaki's
/// SMTC backend. Returns `None` if the winit window does not exist yet or the
/// platform handle is not `Win32` — `attach_smtc` then leaves controls inert.
#[cfg(target_os = "windows")]
fn win32_hwnd(app: &AppWindow) -> Option<*mut std::ffi::c_void> {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    app.window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
            _ => None,
        })
        .flatten()
}

/// Push the embedded EXE icon (ordinal 1, emitted by `winresource` in
/// `build.rs`) onto the winit window as `ICON_SMALL`. Without this call the
/// caption icon stays generic — winit's WNDCLASS registers `hIcon: 0` and
/// Windows only auto-binds the EXE icon to the taskbar, not the caption.
///
/// Failures are warn-logged and dropped: the titlebar just falls back to
/// the generic icon, no functional impact.
#[cfg(target_os = "windows")]
fn install_window_icon(app: &AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::platform::windows::IconExtWindows;
    use slint::winit_030::winit::window::Icon;

    match Icon::from_resource(1, None) {
        Ok(icon) => {
            app.window().with_winit_window(|w| {
                w.set_window_icon(Some(icon));
            });
        }
        Err(e) => {
            log::warn!("Failed to load Melodia icon from EXE resource (ordinal 1): {e}");
        }
    }
}
