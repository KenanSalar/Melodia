// Without this, rustc defaults to the `console` subsystem on Windows: the OS
// allocates a console beside the GUI window and ties the app's lifetime to it.
// Gated on `debug_assertions` so `cargo run` keeps a console for `RUST_LOG` /
// `MELODIA_RSS_SAMPLE`. A parent's `Stdio::piped()` still captures stdout either
// way, which is what the updater's smoke test relies on.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod boot;
mod shutdown;

use melodia_app::services::settings::SettingsData;
use melodia_app::state::AppState;
use melodia_app::{library, services, tasks};
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_core::utils;
use melodia_platform::services::platform;
use melodia_ui::AppWindow;
use melodia_views::ui;
use slint::ComponentHandle;

/// Rayon's global pool, where jpeg-decoder runs its passes for every JPEG decoded outside a pool.
/// Two keeps a cover decode parallel without a worker per core idling from the first decode to
/// quit; a pass over library files brings a `ScanPool` of its own rather than landing here.
const GLOBAL_RAYON_THREADS: usize = 2;

fn main() -> AppResult<()> {
    // The updater's post-swap smoke test spawns the freshly renamed binary with
    // this and asserts exit 0 plus a `Melodia ` prefix carrying the expected
    // version. It must **stay here forever**: removing the branch or the prefix
    // breaks in-place updates for every older client. Ahead of `mallopt`, which
    // shapes a long-lived process's steady state and would only cost the
    // verifier latency.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        use std::io::Write;
        // Locked handle to dodge `clippy::print_stdout`, which guards against
        // accidental GUI-app stdout where this is a deliberate CLI contract. A
        // write failure is swallowed — the binary still works, and the
        // verifier's prefix check fails on the empty stdout anyway.
        let _ = writeln!(std::io::stdout().lock(), "Melodia {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Answers what the rest of the diagnostics feature can't: that surface sits
    // behind Settings → About, unreachable when the thing being reported is that
    // Melodia won't open. Touches neither the database nor Slint, and stays
    // beside the branch above — both must precede anything that can fail.
    //
    // Linux and macOS only, not by choice: a release build has no console under
    // `windows_subsystem = "windows"`, so this is swallowed there. Debug builds
    // are console-subsystem, making the gap invisible locally — `README.md` and
    // the issue template point Windows at `%APPDATA%\Melodia\logs\`.
    if std::env::args().nth(1).as_deref() == Some("--logs") {
        use std::io::Write;
        let paths = Paths::resolve()?;
        let _ = writeln!(std::io::stdout().lock(), "{}", paths.logs_dir.display());
        return Ok(());
    }

    prepare_process();

    // Ahead of the logger, whose file sink needs somewhere to write. A failure
    // here has no logger to reach and never will — it means no data directory.
    let paths = Paths::resolve()?;

    // Claim the right to be the only Melodia over this data directory, or hand
    // what we were asked to open to the one that already is. Ahead of the logger
    // so a forwarding launch never opens the shared file, and ahead of
    // everything expensive so it costs a socket write and a return.
    let startup_files = platform::single_instance::audio_files_from_argv();
    let mut unenforced_reason = None;
    let file_open_listener = match platform::single_instance::claim(&paths.data_dir, &startup_files)
    {
        platform::single_instance::Claim::Secondary => return Ok(()),
        platform::single_instance::Claim::Primary(listener) => Some(listener),
        platform::single_instance::Claim::Unenforced(e) => {
            unenforced_reason = Some(e);
            None
        }
    };

    install_diagnostics(&paths, unenforced_reason);
    let runtime = build_runtime()?;

    // Slint's a11y/D-Bus thread looks up a tokio reactor from UI-thread tasks,
    // so the guard has to stay alive for the entire `app.run()` window.
    let runtime_guard = runtime.enter();

    let (state, channels) = runtime.block_on(AppState::init(paths, runtime.handle().clone()))?;

    log::info!(
        "AppState initialized: db ok, watcher built, media controls = {}",
        if state.media_controls.is_some() { "ready" } else { "no-op" }
    );

    let spawner = tasks::TaskSpawner::from_state(&state);
    boot::tasks::spawn_background_tasks(&spawner, &state, channels);
    boot::tasks::restore_persisted_playback(&runtime, &state);

    // Read `settings.json` and `views.json` once and reuse them everywhere.
    let startup_settings: Option<services::settings::SettingsData> =
        library::settings::get_settings(&state.paths).ok();
    let startup_view_state: Option<services::view_state::ViewStateData> =
        library::settings::get_view_state(&state.paths).ok();

    // Files on the command line replace the restored queue and play, so resume
    // would only be visible for the moment it takes them to land.
    if startup_files.is_empty() {
        boot::tasks::maybe_resume_on_startup(&state, startup_settings.as_ref());
    } else {
        boot::tasks::open_startup_files(&runtime, &state, &startup_files);
    }

    boot::tasks::spawn_resume_watching(&spawner, &state);

    let app = open_window(startup_settings.as_ref())?;
    boot::ui_setup::install_ui(
        &app,
        &state,
        &spawner,
        startup_settings.as_ref(),
        startup_view_state.as_ref(),
        file_open_listener,
    )?;

    // Not `app.run()`: its loop terminates as soon as the last window is
    // *hidden*, which close-to-tray does. This is Slint's documented tray
    // pattern, returning only on `quit_event_loop()`.
    app.show().map_err(|e| AppError::io("show window", e))?;
    slint::run_event_loop_until_quit().map_err(|e| AppError::io("event loop", e))?;

    log::info!("Melodia shutting down — flushing player state");
    shutdown::save_state_on_exit(&app, &state, &runtime);

    // Before `process::exit(0)` skips destructors: an exclusive claim hands the device back with
    // the volume it had, rather than leaving it at Melodia's.
    state.engine.close_output();

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
    platform::logging::flush();

    shutdown::respawn_if_requested();

    // Returning normally would linger until every non-daemon thread exits, and
    // two never do: accesskit's a11y thread and any tokio worker parked on a
    // blocking call. State is flushed and the rest is OS-managed.
    std::process::exit(0);
}

/// What has to happen before the first thread starts or the first arena is created.
fn prepare_process() {
    // Reap the rollback snapshot `swap_in_place` keeps on AppImage / tarball
    // installs — reaching this launch proves the new binary works. Linux-only;
    // msiexec never leaves an `.old` at the install target.
    #[cfg(target_os = "linux")]
    {
        if let Ok(stale) = services::updater::install_target_old() {
            let _ = std::fs::remove_file(stale);
        }
    }
    // Ahead of the logger and the runtime builder, both of which allocate; the
    // module argues the numbers.
    platform::allocator::pin_arenas_and_thresholds();

    // Give PipeWire's ALSA-compat layer a clean stream name: CPAL opens the
    // default ALSA PCM, which PipeWire turns into a node auto-named
    // `alsa_playback.<prgname>` and EasyEffects and pavucontrol show verbatim.
    // pipewire-alsa reads this when the PCM opens; ignored on bare ALSA. Before
    // any thread spawns, so `AppState::init`'s device inherits it.
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
}

/// The rolling log and the crash hook, then the startup facts only this launch knows.
fn install_diagnostics(paths: &Paths, unenforced_reason: Option<std::io::Error>) {
    // Infallible: a log file that can't be opened degrades to stderr rather
    // than stopping the boot. See `platform::logging::install`.
    // An unparseable settings file is no reason to start louder; it surfaces later through
    // `AppState::init`'s own read.
    let verbose_logging = services::settings::read_settings(paths)
        .is_ok_and(|settings| settings.diagnostics.verbose_logging);
    platform::logging::install(paths, verbose_logging);
    // Before the runtime and before Slint, so boot panics are covered too.
    platform::crash_report::install_hook(&paths.logs_dir);
    log::info!("Melodia starting");
    // Which root this boot landed on is the one startup fact nothing downstream can infer: a dev
    // build and `MELODIA_DATA_DIR` both move it. The diagnostics bundle carries it as a field of
    // its own; this is the copy a live tail has.
    log::info!("data directory: {}", utils::redact::redact_home(&paths.data_dir.to_string_lossy()));
    // The claim happened before there was anywhere to say this.
    if let Some(e) = unenforced_reason {
        log::warn!(
            "single_instance: not enforced ({}); a second launch will open a second window",
            describe(&e)
        );
    }
}

/// The tokio runtime, and rayon's global pool beside it.
fn build_runtime() -> AppResult<tokio::runtime::Runtime> {
    // Two workers: the async work is event-driven (queries, watch publishes,
    // position ticks, media-control events) and CPU-bound work goes to Rayon
    // or `spawn_blocking`, neither of which draws on this pool. The `num_cpus`
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
        .map_err(|e| AppError::io("tokio runtime", e))?;

    // Ahead of the first decode: rayon fixes the global pool's shape at first use.
    if let Err(e) = rayon::ThreadPoolBuilder::new()
        .num_threads(GLOBAL_RAYON_THREADS)
        .thread_name(|i| format!("rayon-{i}"))
        .build_global()
    {
        log::warn!("rayon global pool: {}; rayon will size its own", describe(&e));
    }
    Ok(runtime)
}

/// The main window on the winit backend, at the persisted geometry and not yet shown.
fn open_window(settings: Option<&SettingsData>) -> AppResult<AppWindow> {
    // Maximized rides the winit `WindowAttributes` hook — Slint exposes no API
    // for it, and the hook creates the window already-maximized with no flash.
    // Size and position come after `AppWindow::new()`, via `geometry::restore`.
    let geometry = settings.map_or_else(
        ui::window_chrome::geometry::PersistedGeometry::fallback,
        ui::window_chrome::geometry::PersistedGeometry::from_settings,
    );
    select_backend(geometry.maximized)?;

    let app = AppWindow::new().map_err(|e| AppError::io("main window", e))?;

    // After `AppWindow::new()` (the adapter must exist) and before `app.run()`
    // (the window must not be shown). `set_size` sets winit's
    // `has_explicit_size`, which stops Slint snapping the window to its
    // content-preferred size on first show.
    ui::window_chrome::geometry::restore(
        &app,
        geometry,
        settings.and_then(|s| s.full_player_geometry),
    );
    Ok(app)
}

fn select_backend(restore_maximized: bool) -> AppResult<()> {
    let backend = slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(move |attrs| {
            let attrs = if restore_maximized { attrs.with_maximized(true) } else { attrs };
            // Pin the window identity so the compositor resolves our icon and
            // label. X11 matches `WM_CLASS` against the desktop file's
            // `StartupWMClass=Melodia`, falling back to the binary basename when
            // empty. Wayland clients can't set an icon at all — the compositor
            // matches `app_id` to a desktop file of the *same basename* and
            // reads its `Icon=` — so that half must be the reverse-DNS id we
            // install under, or KWin shows the generic placeholder.
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
        });
    // Counts the loop's `NewEvents`, which is how the pump tells a Win32 drag's modal loop apart.
    #[cfg(target_os = "windows")]
    let backend =
        backend.with_winit_custom_application_handler(ui::window_chrome::parked_loop::LoopTicks);
    backend.select().map_err(|e| AppError::io("backend selector", e))
}

#[cfg(test)]
#[path = "tests/main_order_tests.rs"]
mod tests;
