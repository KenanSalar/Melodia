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
    // `--version` is the updater's post-swap smoke test
    // (`updater::install::verify_swapped_binary`), which spawns the freshly
    // renamed binary to confirm it boots before offering Restart. It asserts
    // exit 0 within 5 s and stdout starting `Melodia ` and carrying the expected
    // version, so this must print exactly that and return promptly — and must
    // **stay here forever**: removing it, or the prefix, breaks in-place updates
    // for every older client that smoke-tests against it.
    //
    // Ahead of `mallopt`, which only shapes steady-state RSS of a long-lived
    // process and would cost the verifier latency for nothing.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        use std::io::Write;
        // Locked handle to dodge `clippy::print_stdout`, which guards against
        // accidental GUI-app stdout; this is a deliberate CLI contract. A write
        // failure is swallowed because the binary still *works* — and the
        // verifier's prefix check fails on the empty stdout regardless.
        let _ = writeln!(std::io::stdout().lock(), "Melodia {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // `--logs` answers what the rest of the diagnostics feature can't: it sits
    // behind Settings → About, unreachable when the thing being reported is that
    // Melodia won't open. Touches neither the database nor Slint, and stays
    // beside the branch above — both are contracts that must precede anything
    // that can fail.
    //
    // Linux and macOS only, not by choice: `windows_subsystem = "windows"`
    // leaves a release build no console, so `GetStdHandle` hands back nothing
    // and this is swallowed (the branch above escapes only via the updater's
    // `Stdio::piped()`). Debug builds are console-subsystem, so the gap is
    // invisible locally — `README.md` and the issue template point Windows at
    // `%APPDATA%\Melodia\logs\`.
    if std::env::args().nth(1).as_deref() == Some("--logs") {
        use std::io::Write;
        let paths = Paths::resolve()?;
        let _ = writeln!(std::io::stdout().lock(), "{}", paths.logs_dir.display());
        return Ok(());
    }

    // Reap the previous-binary snapshot `updater::install::swap_in_place` keeps
    // for rollback on AppImage / tarball installs: reaching this launch proves
    // the new binary works. Linux-only — Windows upgrades through msiexec, which
    // never leaves an `.old` at the install target.
    #[cfg(target_os = "linux")]
    {
        if let Ok(stale) = melodia::services::updater::install_target_old() {
            let _ = std::fs::remove_file(stale);
        }
    }
    // Cap the arenas at 2. glibc's default `8 × num_cpus` gives every long-lived
    // thread its own 64 MiB arena, and this process runs enough of them (art
    // prewarm, queue restore, Slint, souvlaki, SQLx) that the committed slack is
    // pure per-thread free-list overhead. Capping trades it for malloc
    // contention under heavy parallel allocation, which an idle-most-of-the-time
    // player doesn't have. **Must precede the first malloc on any thread** — the
    // logger and the runtime builder both allocate, so staying first covers it.
    //
    // The other two freeze the mmap and trim thresholds, which glibc otherwise
    // ratchets: freeing an mmap'd block raises the mmap threshold to that size
    // and trim to twice it, so one full-resolution cover decode leaves every
    // later allocation coming off the arena free list, where freeing hands
    // nothing back to the kernel. Setting either explicitly disables the
    // adjustment, and these *are* glibc's initial values — pinning where the
    // process starts, not tuning. Trade: more minor faults for less resident
    // anonymous memory.
    //
    // `M_TRIM_THRESHOLD = -1`, `M_MMAP_THRESHOLD = -3`, `M_ARENA_MAX = -8` per
    // glibc's `malloc.h`.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[allow(unsafe_code, reason = "FFI to glibc mallopt with constant args")]
    // SAFETY: no pointers cross the boundary — `int` in, `int` out. `mallopt` is
    // MT-Unsafe during init and nothing has spawned a thread yet, so no
    // concurrent allocation can observe a half-applied set.
    unsafe {
        libc::mallopt(-8, 2);
        libc::mallopt(-3, 128 * 1024);
        libc::mallopt(-1, 128 * 1024);
    }

    // Give PipeWire's ALSA-compat layer a clean stream name. CPAL opens the
    // default ALSA PCM, which PipeWire turns into a graph node auto-named
    // `alsa_playback.<prgname>` — what EasyEffects and pavucontrol show
    // verbatim. pipewire-alsa reads `PIPEWIRE_ALSA` (SPA-JSON) when the PCM
    // opens; `node.name` overrides the auto-name and `application.name` fills
    // those mixers' app column, so the stream reads simply "Melodia". Ignored on
    // bare ALSA and non-PipeWire systems. Before any thread spawns, so
    // `AppState::init`'s device inherits it.
    #[cfg(target_os = "linux")]
    #[allow(unsafe_code, reason = "env::set_var is unsafe in Rust 2024")]
    // SAFETY: `set_var` requires that no other thread is reading or writing the
    // environment. Nothing has spawned one yet — the logger, the runtime and
    // Slint all come later.
    unsafe {
        std::env::set_var(
            "PIPEWIRE_ALSA",
            "{ application.name = \"Melodia\" node.name = \"Melodia\" }",
        );
    }

    // Ahead of the logger, whose file sink needs somewhere to write. A failure
    // here has no logger to reach and never will — it means no data directory.
    let paths = Paths::resolve()?;

    // Claim the right to be the only Melodia over this data directory, or hand
    // what we were asked to open to the one that already is. Ahead of the logger
    // so a forwarding launch never opens the shared file, and ahead of
    // everything expensive so it costs a socket write and a return.
    let startup_files = services::single_instance::audio_files_from_argv();
    let mut unenforced_reason = None;
    let file_open_listener = match services::single_instance::claim(&paths.data_dir, &startup_files)
    {
        services::single_instance::Claim::Secondary => return Ok(()),
        services::single_instance::Claim::Primary(listener) => Some(listener),
        services::single_instance::Claim::Unenforced(e) => {
            unenforced_reason = Some(e);
            None
        }
    };

    // Infallible: a log file that can't be opened degrades to stderr rather
    // than stopping the boot. See `services::logging::install`.
    services::logging::install(&paths);
    // Before the runtime and before Slint, so boot panics are covered too.
    services::crash_report::install_hook(&paths.logs_dir);
    log::info!("Melodia starting");
    // The claim happened before there was anywhere to say this.
    if let Some(e) = unenforced_reason {
        log::warn!(
            "single_instance: not enforced ({e}); a second launch will open a second window"
        );
    }

    // Two workers: the async work is event-driven (queries, watch publishes,
    // position ticks, souvlaki) and CPU-bound work goes to Rayon or
    // `spawn_blocking`, neither of which draws on this pool. The `num_cpus`
    // default leaves 6+ idle threads on a desktop, each with a 2 MB stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        // A burst ceiling, not an idle-cost one — blocking threads spawn on
        // demand and get reaped, so tokio's 512 default never sits resident.
        // `system_theme::spawn_color_watcher` is the one permanent tenant.
        .max_blocking_threads(32)
        .enable_all()
        .thread_name("melodia-bg")
        .build()
        .map_err(|e| AppError::Settings(format!("tokio runtime: {e}")))?;

    // Slint's a11y/D-Bus thread looks up a tokio reactor from UI-thread tasks,
    // so the guard has to stay alive for the entire `app.run()` window.
    let runtime_guard = runtime.enter();

    let (state, channels) = runtime.block_on(AppState::init(paths, runtime.handle().clone()))?;

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

    // 3a. Files on the command line replace the restored queue and play, so
    // resume would only be visible for the moment it takes them to land.
    if startup_files.is_empty() {
        boot::tasks::maybe_resume_on_startup(&state, startup_settings.as_ref());
    } else {
        boot::tasks::open_startup_files(&runtime, &state, &startup_files);
    }

    // 4. First-launch auto-add + folder-watcher restart.
    boot::tasks::spawn_first_launch(&spawner, &state);

    // 4-pre. Persisted window geometry. Maximized rides the winit
    // `WindowAttributes` hook — Slint exposes no API for it, and the hook
    // creates the window already-maximized with no flash. Size and position come
    // after `AppWindow::new()` via `geometry::restore`. `fallback()` covers a
    // first launch with no `settings.json`.
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
            // Pin the window identity so the compositor resolves our icon and
            // label. The two protocols match differently:
            //
            //   * X11 matches `WM_CLASS` res_class against the desktop file's
            //     `StartupWMClass=Melodia`. Left empty, the compositor falls
            //     back to the binary basename — lowercase for `/usr/bin/melodia`.
            //   * Wayland clients cannot set an icon at all: the compositor
            //     matches `app_id` to a desktop file of the *same basename* and
            //     reads its `Icon=`, ignoring `StartupWMClass`. Ours installs as
            //     `com.github.kenansalar.melodia.desktop`, so the `app_id` must
            //     be that reverse-DNS id or KWin shows the generic placeholder.
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

    // After `AppWindow::new()` (the adapter must exist) and before `app.run()`
    // (the window must not be shown). `set_size` sets winit's
    // `has_explicit_size`, suppressing Slint's content-preferred resize on first
    // show — without it the window snaps to the ~640×420 layout minimum.
    ui::window_chrome::geometry::restore(&app, geometry);

    // 4a. Locale.
    boot::ui_setup::install_locale(&app, &state, startup_settings.as_ref());

    // 4b. Window chrome (no-frame / drag region / native-frame hydrate).
    boot::ui_setup::install_app_chrome(&app, &state);

    // 5–5c3. Tracks / Browse / Albums views + their callbacks + the
    // now-playing-favorite fan-out + album cover-cache tune. Returns the
    // per-view handles for downstream wiring.
    let views = boot::ui_setup::install_views(&app, &state, startup_view_state.as_ref());

    // 5d–5d4. Library Settings + playback toggle + notifications stack +
    // file-watcher toggle.
    let notifications = boot::ui_setup::install_library_settings_and_friends(&app, &state)?;

    // 5d5. Playlist import/export (M3U8) action pills. Wired here — after
    // both the playlists UI handle and the notifications stack exist —
    // because the completion toasts need the `Rc<NotificationsUi>`.
    ui::playlists::wire_files(&app, &state, &views.playlists_ui, &notifications);

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

    // 5f. Material You coordinator — after `appearance::install`, whose kick
    // channel it subscribes to.
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
    ui::shell::bridge::spawn_view_model_subscriber(
        weak.clone(),
        &state.sinks,
        views.cover_thumbs.clone(),
    )
    .map_err(|e| AppError::Window(format!("view-model subscriber: {e}")))?;
    ui::shell::bridge::spawn_queue_subscriber(weak.clone(), &state.sinks)
        .map_err(|e| AppError::Window(format!("queue subscriber: {e}")))?;
    ui::shell::bridge::spawn_position_subscriber(weak.clone(), &state.position_tx)
        .map_err(|e| AppError::Window(format!("position subscriber: {e}")))?;

    // 6b. Queue bottom-sheet.
    match ui::queue_sheet::install(&app, &state) {
        Ok(h) => ui::window_chrome::set_queue_sheet_open(h.is_open),
        Err(e) => log::warn!("queue_sheet::install: {e}"),
    }

    // 6c. Full-screen Now Playing view (owns its own small `(cover, blur)`
    // LRU separate from `cover_thumbs`).
    let np_artwork = Arc::new(ui::now_playing_artwork::NowPlayingArtwork::new());
    let np_state = match ui::now_playing::install(&app, &state, &views.cover_thumbs, &np_artwork) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("now_playing::install: {e}");
            None
        }
    };
    // Needs `np_state` for the up-next subscriber gate; without it the gate
    // would flip nothing visible.
    if let Some(ref np_state) = np_state
        && let Err(e) = ui::shell::mini_player::install(&app, &state, &np_artwork, np_state)
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

    // 9b-ii. Toast when the audio output device goes away mid-session.
    boot::ui_setup::install_audio_device_lost_subscriber(
        &state,
        weak.clone(),
        notifications.clone(),
    )?;

    // 9b-iii. The Ko-fi link, plus the one-time support toast a few minutes into
    // whichever early launch is the fifth. Counts this launch either way.
    ui::support::install(&app, &state, notifications.clone())?;

    // 9c. Surface backend failures (playback decode errors, failed scans /
    // imports / saves) pushed through the `services::toast` bridge as toasts.
    boot::ui_setup::install_toast_bridge(weak.clone(), notifications.clone())?;

    // 10. Opt-in memory sampler (`MELODIA_RSS_SAMPLE=1`), no-op when unset. On
    // the UI thread so it can read the Nav / *Detail globals for its view tag
    // without an atomic-shadow plumbing pass.
    tasks::rss_sampler::install(&weak);

    // 11. Auto-updater. One `watch` carries backend events from the daily task
    // and the `Updater.*` callbacks to the UI-thread subscriber that toasts
    // them. The daily task spawns only when auto-check is on *and* the install
    // path is user-writable — system-managed installs go through the OS package
    // manager instead.
    let (updater_event_tx, updater_event_rx) =
        watch::channel::<Option<services::updater::UpdaterEvent>>(None);
    ui::settings::updater_settings::install_event_subscriber(
        weak.clone(),
        notifications.clone(),
        updater_event_rx,
    );
    ui::callbacks::wire_updater(&app, &state, &notifications, &updater_event_tx);

    let updater_settings_snapshot =
        startup_settings.as_ref().map(|s| s.updates.clone()).unwrap_or_default();
    if updater_settings_snapshot.auto_check_enabled && !services::updater::is_system_install() {
        tasks::updater_daily::spawn(&spawner, state.clone(), weak.clone(), updater_event_tx);
    } else {
        log::info!(
            "updater_daily: not spawning (auto_check_enabled={}, system_managed={})",
            updater_settings_snapshot.auto_check_enabled,
            services::updater::is_system_install()
        );
    }

    // Independent of the daily check: the in-attempt prune only fires on the
    // next install click, so a cancelled or failed install leaves a verified
    // package in the staging dir forever for a user who never clicks again. The
    // 30 s grace matches `updater_daily::STARTUP_DELAY`, giving the first-launch
    // scan and DB pre-fetch first claim on the disk.
    runtime.spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        services::updater::prune_stale_staging().await;
    });

    // Tarball installs only — RPM/DEB own those files through their manifests
    // and AppImage bundles them. The BLAKE3 compare means the common case is a
    // stat plus a 3 KB hash and no write. Gating rationale in the module.
    #[cfg(target_os = "linux")]
    runtime.spawn_blocking(|| {
        if let Err(e) = services::desktop_integration::refresh_user_install() {
            log::warn!("desktop_integration: refresh failed: {e}");
        }
    });

    // Opt-in and off by default; when off, no D-Bus connection, no ksni thread,
    // no receiver tasks. Restart-gated through the `restart-tray` Dialog, so
    // this startup read is the single gate. `install` is cross-platform: Linux
    // creates the StatusNotifierItem eagerly, Win/mac defer `tray-icon` onto the
    // event loop, which has to be running first.
    if startup_settings.as_ref().is_some_and(|s| s.tray.tray_enabled) {
        ui::shell::tray_bridge::install(&spawner, &state, &app);
    }

    // Answer the launches queued on the socket claimed back at boot — here
    // rather than beside the claim, a forwarded track needing a window to raise.
    if let Some(listener) = file_open_listener {
        boot::tasks::serve_file_opens(&state, &app, listener);
    }

    // SMTC needs a real `HWND`, which exists only once the window is shown, so
    // `AppState::init` left the handle inert (souvlaki panics on a null one).
    // Posting to the event loop runs this on the first iteration, past the show.
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
                        // `sync()` no-op'd while the controls were inert, so the
                        // OS panel is still empty — push current state through
                        // the canonical path.
                        melodia::player::state::with_state_emit(&player_state, &sinks, |_| {});
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

    // Native-titlebar polish: boot's `themes::apply` ran inside
    // `appearance::install`, but its DWM hook no-op'd with no HWND yet. By the
    // time this one-shot fires the window is up and `Theme.mantle` carries the
    // resolved palette, so the caption picks up dark/light and its mantle
    // colour; every later palette change drives DWM from `write_palette`.
    //
    // The same closure pushes the embedded icon onto the winit window: the
    // taskbar reads `assets/melodia.ico` out of the EXE directly, but the
    // *caption* icon comes from the WNDCLASS, which winit registers with
    // `hIcon: 0` — without `set_window_icon` the titlebar stays generic.
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

    // Not `app.run()`: its `run_event_loop` terminates as soon as the last
    // window is *hidden*, which close-to-tray does. `run_event_loop_until_quit`
    // is Slint's documented tray pattern — it survives having no visible window
    // and returns only on `quit_event_loop()`, which every quit path already
    // calls.
    app.show().map_err(|e| AppError::Window(e.to_string()))?;
    slint::run_event_loop_until_quit().map_err(|e| AppError::Window(e.to_string()))?;

    log::info!("Melodia shutting down — flushing player state");
    shutdown::save_state_on_exit(&app, &state, &runtime);

    // Before `process::exit(0)` skips destructors — a leaked `tray-icon` ghosts
    // in the Windows notification area. No-op on Linux, whose ksni handle its
    // subscriber drops during `flush_tasks_and_db`. Main thread, as the `!Send`
    // Win/mac handle requires.
    ui::shell::tray_bridge::shutdown();

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

    // `!Send`, so it can't ride into the background drop thread.
    drop(runtime_guard);

    shutdown::drop_runtime_in_background(runtime);

    // Before `respawn_if_requested`, which `exec`s and never returns on Unix,
    // and before the `process::exit(0)` below — neither runs a destructor.
    services::logging::flush();

    shutdown::respawn_if_requested();

    // Returning normally would linger until every non-daemon thread exits, and
    // four of them never do: the leaked `MixerDeviceSink`'s rodio output thread,
    // souvlaki's MPRIS thread, accesskit's a11y thread, and any tokio worker
    // parked on a blocking call. State is flushed and the rest is OS-managed.
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

/// Push the embedded EXE icon (ordinal 1, from `build.rs`'s `winresource`) onto
/// the winit window. Windows auto-binds that icon to the taskbar but not the
/// caption, whose WNDCLASS winit registers with `hIcon: 0`.
///
/// Failures are logged and dropped — the titlebar falls back to the generic icon.
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
